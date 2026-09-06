//! Resolver stage 3 (`docs/resolver-design.md`): a full `viv update`-style
//! solve against recorded Packagist fixtures, plus small hand-built pools
//! exercising each `DefaultPolicy` tie-break and a couple of solver-level
//! outcomes (a conflict that forces a backjump, an unsatisfiable root
//! require).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;
use vivace::repository::{Repository, Transport};
use vivace::semver;
use vivace::solver::policy::DefaultPolicy;
use vivace::solver::pool::{Link, Package, Pool};
use vivace::solver::request::Request;
use vivace::solver::solver;

const FIXED_LAST_MODIFIED: &str = "Mon, 01 Jan 2024 00:00:00 GMT";

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/repo.packagist.org")
}

/// Same fixture-replaying transport as `tests/repository.rs`.
struct FixtureTransport {
    root: PathBuf,
}

impl Transport for &FixtureTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &reqwest::Url,
        if_modified_since: Option<&str>,
    ) -> anyhow::Result<vivace::fetch::Conditional> {
        if if_modified_since == Some(FIXED_LAST_MODIFIED) {
            return Ok(vivace::fetch::Conditional::NotModified);
        }
        let path = self.root.join(url.path().trim_start_matches('/'));
        match fs_err::read(&path) {
            Ok(body) => Ok(vivace::fetch::Conditional::Fresh {
                body,
                last_modified: Some(FIXED_LAST_MODIFIED.to_string()),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(vivace::fetch::Conditional::NotFound)
            }
            Err(err) => Err(err.into()),
        }
    }
}

#[tokio::test]
async fn full_update_reproduces_the_monolog_lock() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let root: Value = serde_json::from_str(
        &fs_err::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.json"),
        )
        .unwrap(),
    )
    .unwrap();

    let resolved = vivace::solver::solve_full_update(&repo, &root, false, false)
        .await
        .unwrap();

    let got: BTreeMap<String, String> = resolved
        .into_iter()
        .map(|p| (p.name, p.pretty_version))
        .collect();

    // The full-update solve, before the dev split (stage 4): every package
    // in the fixture's committed lock, `packages` and `packages-dev`
    // combined, exactly like `Installer::doUpdate`'s first solve.
    let mut want = BTreeMap::new();
    want.insert("monolog/monolog".to_string(), "3.11.0".to_string());
    want.insert("psr/log".to_string(), "3.0.2".to_string());
    want.insert("psr/container".to_string(), "2.0.2".to_string());

    assert_eq!(got, want, "{got:?}");
}

#[tokio::test]
async fn unsatisfiable_root_require_names_the_package() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let root: Value = serde_json::json!({
        "name": "vivace/fixture-unsatisfiable",
        "require": { "monolog/this-package-does-not-exist": "^99.0" }
    });

    let err = vivace::solver::solve_full_update(&repo, &root, false, false)
        .await
        .unwrap_err();

    // The blunt `Problem` port names the package and prints *some*
    // rendering of its constraint; matching the exact original `^99.0`
    // syntax is `Problem.php`'s job (stage 5, composer/composer#42), not
    // this stage's.
    let message = format!("{err}");
    assert!(
        message.contains("monolog/this-package-does-not-exist"),
        "{message}"
    );
    assert!(message.contains("99.0"), "{message}");
}

// --- Hand-built pool tests: one per `DefaultPolicy` tie-break, plus a
// conflict that forces a solver backjump. Each builds a `Pool`/`Request`
// directly rather than through `pool_builder`, and uses an unconstrained
// (`None`) root require so `Pool::what_provides` returns every candidate
// regardless of version text, isolating the tie-break under test from
// unrelated constraint-matching behaviour.

fn version(v: &str) -> semver::NormalizedVersion {
    semver::normalize(v).unwrap()
}

fn link(target: &str) -> Link {
    Link {
        target: target.to_string(),
        constraint: None,
        pretty_constraint: None,
    }
}

fn package(name: &str, pretty_version: &str) -> Package {
    let normalized = version(pretty_version);
    Package {
        stability: semver::stability(normalized.as_str()),
        is_dev: semver::stability(normalized.as_str()) == "dev",
        name: name.to_string(),
        version: normalized,
        pretty_version: pretty_version.to_string(),
        requires: Vec::new(),
        conflicts: Vec::new(),
        provides: Vec::new(),
        replaces: Vec::new(),
        alias_of: None,
        is_root_package_alias: false,
        has_self_version_requires: false,
        raw: serde_json::json!({ "name": name, "version": pretty_version }),
    }
}

fn require(name: &str) -> Request {
    Request {
        requires: vec![(name.to_string(), None)],
        fixed: Vec::new(),
    }
}

/// Solves and returns the installed, non-empty name -> pretty version map.
fn solve(
    pool: &Pool,
    request: &Request,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> BTreeMap<String, String> {
    let policy = DefaultPolicy::new(prefer_stable, prefer_lowest);
    let installed = solver::solve(&policy, pool, request).unwrap();
    installed
        .into_iter()
        .map(|id| {
            let p = pool.package_by_id(id);
            (p.name.clone(), p.pretty_version.clone())
        })
        .collect()
}

#[test]
fn prefers_a_branch_alias_over_the_branch_it_aliases() {
    // A branch-alias pool entry (`dev-main` aliased to `2.x-dev`) always
    // sorts before the plain branch in `DefaultPolicy::compareByPriority`'s
    // same-name check (`docs/resolver-design.md`: "Prefer alias over
    // aliased").
    let real = package("vendor/pkg", "dev-main");
    let mut alias = package("vendor/pkg", "2.x-dev");
    alias.alias_of = Some(0);
    let pool = Pool::new(vec![real, alias]);

    let got = solve(&pool, &require("vendor/pkg"), false, false);
    assert_eq!(got.get("vendor/pkg").map(String::as_str), Some("2.x-dev"));
}

#[test]
fn prefers_the_replaced_package_over_its_replacer() {
    let original = package("vendor/original", "1.0.0");
    let mut fork = package("vendor/fork", "1.0.0");
    fork.replaces = vec![link("vendor/original")];
    let pool = Pool::new(vec![original, fork]);

    let got = solve(&pool, &require("vendor/original"), false, false);
    assert_eq!(got.len(), 1);
    assert!(got.contains_key("vendor/original"), "{got:?}");
}

#[test]
fn prefers_a_replacer_from_the_same_vendor_as_the_requiring_package() {
    let mut acme = package("acme/pkg-a", "1.0.0");
    acme.replaces = vec![link("acme/pkg")];
    let mut other = package("other/pkg-b", "1.0.0");
    other.replaces = vec![link("acme/pkg")];
    let pool = Pool::new(vec![acme, other]);

    let got = solve(&pool, &require("acme/pkg"), false, false);
    assert_eq!(got.len(), 1);
    assert!(got.contains_key("acme/pkg-a"), "{got:?}");
}

#[test]
fn prefer_stable_picks_the_stable_version_over_a_higher_unstable_one() {
    let stable = package("vendor/pkg", "1.0.0");
    let unstable = package("vendor/pkg", "2.0.0-beta1");
    let pool = Pool::new(vec![stable, unstable]);

    let got = solve(&pool, &require("vendor/pkg"), true, false);
    assert_eq!(got.get("vendor/pkg").map(String::as_str), Some("1.0.0"));
}

#[test]
fn prefer_lowest_picks_the_lowest_version() {
    let low = package("vendor/pkg", "1.0.0");
    let high = package("vendor/pkg", "2.0.0");
    let pool = Pool::new(vec![low, high]);

    let got = solve(&pool, &require("vendor/pkg"), false, true);
    assert_eq!(got.get("vendor/pkg").map(String::as_str), Some("1.0.0"));
}

#[test]
fn a_conflict_forces_a_backjump_to_the_non_conflicting_alternative() {
    // root requires vendor/root, which requires both vendor/x and
    // vendor/y. vendor/x has two candidates: the higher version (the
    // default policy's first pick) conflicts with vendor/y, which is
    // otherwise a forced (single-candidate) dependency. The solver must
    // learn from that conflict and backjump to the lower vendor/x version.
    let mut root = package("vendor/root", "1.0.0");
    root.requires = vec![link("vendor/x"), link("vendor/y")];

    let x_low = package("vendor/x", "1.0.0");
    let mut x_high = package("vendor/x", "2.0.0");
    x_high.conflicts = vec![link("vendor/y")];

    let y = package("vendor/y", "1.0.0");

    let pool = Pool::new(vec![root, x_low, x_high, y]);
    let got = solve(&pool, &require("vendor/root"), false, false);

    assert_eq!(
        got.get("vendor/x").map(String::as_str),
        Some("1.0.0"),
        "{got:?}"
    );
    assert_eq!(
        got.get("vendor/y").map(String::as_str),
        Some("1.0.0"),
        "{got:?}"
    );
    assert!(got.contains_key("vendor/root"));
    assert_eq!(got.len(), 3, "{got:?}");
}
