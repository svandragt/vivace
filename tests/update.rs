//! Resolver stage 4 (`docs/resolver-design.md`): `viv update`'s lock writer,
//! byte-diffed against Composer 2.10.2's own `composer.lock` for the
//! monolog fixture (recorded Packagist metadata, hermetic and offline —
//! same `FixtureTransport` shape as `tests/solver.rs`). The
//! network/php-gated half (`composer validate`/`install --dry-run`/`update
//! --lock`) runs the real `viv` binary against real Packagist, like
//! `tests/install_e2e.rs`.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestContext;
use serde_json::Value;
use vivace::repository::{Repository, Transport};
use vivace::solver;

const FIXED_LAST_MODIFIED: &str = "Mon, 01 Jan 2024 00:00:00 GMT";

/// Same recursive copy as `tests/install_e2e.rs`'s helper of the same name:
/// the monolog fixture's `src`/`lib` autoload sources, needed for `composer
/// install` to generate an autoloader without erroring on a missing
/// directory.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/repo.packagist.org")
}

/// Same fixture-replaying transport as `tests/solver.rs`/`tests/repository.rs`.
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

/// Solves `fixture`'s `composer.json` against the recorded Packagist
/// fixtures and writes the lock exactly like `viv update` would, without
/// going through the CLI (which always hits the real network) or the
/// filesystem.
async fn update_lock(fixture: &Path) -> String {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let result = solver::solve_update(&repo, &root, false, false)
        .await
        .unwrap();

    let options = vivace::lock_writer::LockOptions {
        minimum_stability: result.minimum_stability,
        stability_flags: &result.stability_flags,
        prefer_stable: result.prefer_stable,
        prefer_lowest: result.prefer_lowest,
        platform_reqs: &result.platform_reqs,
        platform_dev_reqs: &result.platform_dev_reqs,
        platform_overrides: &result.platform_overrides,
        aliases: &result.aliases,
    };
    vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
        .unwrap()
}

fn assert_matches_expected(got: &str, expected_path: &Path) {
    let want = fs_err::read_to_string(expected_path).unwrap();
    if got == want {
        return;
    }
    let diff = similar_unified_diff(&want, got);
    panic!(
        "{} does not match viv update's output:\n{diff}",
        expected_path.display()
    );
}

/// A minimal unified-ish line diff: no external crate, just enough context
/// to see what changed when a golden test goes red.
fn similar_unified_diff(want: &str, got: &str) -> String {
    let want_lines: Vec<&str> = want.lines().collect();
    let got_lines: Vec<&str> = got.lines().collect();
    let mut out = String::new();
    let max = want_lines.len().max(got_lines.len());
    for i in 0..max {
        let w = want_lines.get(i).copied();
        let g = got_lines.get(i).copied();
        if w != g {
            use std::fmt::Write as _;
            if let Some(w) = w {
                let _ = writeln!(out, "-{i:>4}: {w}");
            }
            if let Some(g) = g {
                let _ = writeln!(out, "+{i:>4}: {g}");
            }
        }
    }
    out
}

#[tokio::test]
async fn update_reproduces_the_monolog_lock() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");
    let got = update_lock(&fixture).await;
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

#[tokio::test]
async fn update_reproduces_the_legacy_lock() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy");
    let got = update_lock(&fixture).await;
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// Resolver stage 5: a partial `viv update psr/log` against
/// `tests/fixtures/partial-update`'s `lock-before.json` (psr/log rolled
/// back to 3.0.0, monolog/monolog untouched) must keep monolog/monolog at
/// its locked version and refetch only psr/log, landing back on the same
/// `composer.lock` `tests/fixtures/monolog` already has (root
/// `composer.json` there requires psr/log `^3.0` directly, so the newest
/// match is deterministic and happens to be the version already committed).
#[tokio::test]
async fn partial_update_keeps_the_unlisted_package_locked() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/partial-update");
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();
    let lock_before: Value =
        serde_json::from_slice(&fs_err::read(fixture.join("lock-before.json")).unwrap()).unwrap();

    let mut locked_by_name = std::collections::HashMap::new();
    for key in ["packages", "packages-dev"] {
        for entry in lock_before[key].as_array().unwrap() {
            locked_by_name.insert(
                entry["name"].as_str().unwrap().to_ascii_lowercase(),
                entry.clone(),
            );
        }
    }

    let result = vivace::solver::solve_partial_update(
        &repo,
        &root,
        false,
        false,
        &locked_by_name,
        &["psr/log".to_string()],
        vivace::solver::pool_builder::UpdateAllowMode::OnlyListed,
    )
    .await
    .unwrap();

    // monolog/monolog was not in the allow list: it must come back exactly
    // as `lock-before.json` recorded it, not refetched.
    let monolog = result
        .non_dev
        .iter()
        .find(|p| p.name == "monolog/monolog")
        .unwrap();
    assert_eq!(monolog.pretty_version, "3.11.0");
    let psr_log = result.non_dev.iter().find(|p| p.name == "psr/log").unwrap();
    assert_eq!(psr_log.pretty_version, "3.0.2");

    let options = vivace::lock_writer::LockOptions {
        minimum_stability: result.minimum_stability,
        stability_flags: &result.stability_flags,
        prefer_stable: result.prefer_stable,
        prefer_lowest: result.prefer_lowest,
        platform_reqs: &result.platform_reqs,
        platform_dev_reqs: &result.platform_dev_reqs,
        platform_overrides: &result.platform_overrides,
        aliases: &result.aliases,
    };
    let got =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// End-to-end: the real `viv update` binary against real Packagist,
/// byte-diffed the same way, then validated with Composer itself.
/// Gated on `VIVACE_TEST_NETWORK=1` so a bare `cargo nextest run` stays
/// offline, like `tests/install_e2e.rs`.
#[test]
fn viv_update_matches_composer_and_validates() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv_update_matches_composer_and_validates: set VIVACE_TEST_NETWORK=1 to \
             resolve against real Packagist"
        );
        return;
    }
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!("skipping viv_update_matches_composer_and_validates: composer is not on PATH");
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs_err::copy(fixture.join("composer.json"), project.join("composer.json")).unwrap();
    for dir in ["src", "lib"] {
        copy_tree(&fixture.join(dir), &project.join(dir));
    }

    ctx.viv().arg("update").assert().success();

    let got = fs_err::read_to_string(project.join("composer.lock")).unwrap();
    let want = fs_err::read_to_string(fixture.join("composer.lock")).unwrap();
    assert_eq!(
        got, want,
        "viv update's lock differs from the committed one"
    );

    let validate = Command::new("composer")
        .args(["validate", "--strict", "--no-check-publish"])
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "composer validate --strict failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stderr)
    );

    // `install --dry-run` only reports "nothing to do" once something is
    // actually installed; real Composer, not `viv install`, so this is
    // Composer's own dist fetch (network, already gated above).
    let install = Command::new("composer")
        .args(["install", "--no-interaction"])
        .current_dir(project)
        .status()
        .unwrap();
    assert!(install.success(), "composer install failed");

    let dry_run = Command::new("composer")
        .args(["install", "--dry-run"])
        .current_dir(project)
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&dry_run.stdout),
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert!(
        dry_run.status.success() && combined.contains("Nothing to install, update or remove"),
        "composer install --dry-run reported changes: {combined}"
    );

    let before = fs_err::read(project.join("composer.lock")).unwrap();
    Command::new("composer")
        .args(["update", "--lock"])
        .current_dir(project)
        .status()
        .map(|s| assert!(s.success(), "composer update --lock failed"))
        .unwrap();
    let after = fs_err::read(project.join("composer.lock")).unwrap();
    assert_eq!(
        before, after,
        "composer update --lock changed the lock viv update wrote"
    );
}

/// `viv update --lock`: re-derives the lock from itself with no solving
/// (`update::lock_only`, never reaches `Repository::load`), so this needs
/// no network and no recorded Packagist fixture. The monolog fixture's
/// committed lock is already canonical, so round-tripping it must be a
/// byte-identical no-op.
#[test]
fn update_lock_only_reproduces_an_already_canonical_lock() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs_err::copy(fixture.join("composer.json"), project.join("composer.json")).unwrap();
    fs_err::copy(fixture.join("composer.lock"), project.join("composer.lock")).unwrap();

    ctx.viv().args(["update", "--lock"]).assert().success();

    let got = fs_err::read_to_string(project.join("composer.lock")).unwrap();
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// Resolver stage 5's `Problem`/`SolverProblemsException` port
/// (`src/solver/problem.rs`): a root require for a package that plain does
/// not exist matches Composer's exact wording (verified against a real,
/// live `composer update` on this exact requirement: a nonexistent name
/// stays nonexistent, so there is no drift risk in trusting the captured
/// text as a hermetic golden). `viv update`'s own stub composer.json here
/// never resolves anything real, so this needs no recorded Packagist
/// fixture: `monolog/this-package-does-not-exist-xyz` 404s against the
/// same `FixtureTransport` every other hermetic test in this file uses.
#[tokio::test]
async fn unsatisfiable_root_require_matches_composers_message() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let root: Value = serde_json::json!({
        "name": "vivace/fixture-unsatisfiable",
        "require": { "monolog/this-package-does-not-exist-xyz": "^1.0" }
    });

    let Err(err) = solver::solve_update(&repo, &root, false, false).await else {
        panic!("expected an unsatisfiable request to fail")
    };
    let solver_error = err
        .downcast_ref::<vivace::solver::problem::SolverError>()
        .expect("solve_update's error is a SolverError for an unsatisfiable request");

    // Captured verbatim (ANSI-free: `--no-ansi`) from `composer update
    // --no-ansi` on this exact `composer.json`, Composer 2.10.2.
    let want = "Your requirements could not be resolved to an installable set of packages.\n\n  \
                Problem 1\n    - Root composer.json requires \
                monolog/this-package-does-not-exist-xyz, it could not be found in any version, \
                there may be a typo in the package name.\n\nPotential causes:\n - A typo in the \
                package name\n - The package is not available in a stable-enough version \
                according to your minimum-stability setting\n   see \
                <https://getcomposer.org/doc/04-schema.md#minimum-stability> for more details.\n \
                - It's a private package and you forgot to add a custom repository to find \
                it\n\nRead <https://getcomposer.org/doc/articles/troubleshooting.md> for further \
                common problems.\n";
    assert_eq!(format!("{solver_error}"), want);
}
