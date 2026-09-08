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

use std::collections::HashMap;
use std::io::Write as _;
use std::path::Path;
use std::process::Command;

use common::{FixtureTransport, TestContext, fixtures_root};
use serde_json::Value;
use vivace::repository::Repository;
use vivace::solver;
use vivace::store::Store;

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

/// #117: `symfony/string` requires the four `symfony/polyfill-*` packages
/// this fixture's root `replace`s (mirroring `symfony/demo`'s own shape).
/// Composer's own lock (`tests/fixtures/root-replace/composer.lock`) never
/// lists them: the root package's `replace` satisfies `symfony/string`'s
/// requires directly (`Pool::whatProvides`), and
/// `PoolBuilder::buildPool`'s `getFixedOrLockedPackages` loop never even
/// fetches a replaced name's provider file. Before `pool_builder::root_package`/
/// `root_replaced_names`, viv fetched and locked all four anyway.
#[tokio::test]
async fn update_omits_polyfills_the_root_replaces() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/root-replace");
    let got = update_lock(&fixture).await;
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// #90: seeding the closure walk with names from the prior lock is a
/// prefetch, never a pool change. `acme/unreachable` doesn't exist anywhere
/// in the fixtures and is no longer (never was) required by the monolog
/// fixture's `composer.json`; `monolog/monolog` is both seeded and actually
/// reachable. Either way the resulting lock must be byte-identical to the
/// unseeded walk's.
#[tokio::test]
async fn seeding_with_an_unreachable_lock_name_does_not_change_the_lock() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");

    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();
    let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let seed = vec![
        "acme/unreachable".to_string(),
        "monolog/monolog".to_string(),
    ];
    let result = solver::solve_update_seeded(&repo, &root, false, false, &seed, HashMap::new())
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
    let got =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();

    let want = update_lock(&fixture).await;
    assert_eq!(
        got, want,
        "seeded and unseeded updates must match byte-for-byte"
    );
}

#[tokio::test]
async fn update_reproduces_the_legacy_lock() {
    if Command::new("php").arg("--version").output().is_err() {
        eprintln!(
            "skipping update_reproduces_the_legacy_lock: php is not on PATH (platform \
             extensions come from the host php)"
        );
        return;
    }

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
    let locked_by_name = common::locked_by_name(&fixture.join("lock-before.json"));

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

/// #79: `psr/log` is only a transitive requirement, reached solely through
/// `monolog/monolog` (root `composer.json` there never mentions `psr/log`
/// directly, unlike `tests/fixtures/partial-update`). Before the fix,
/// `build_partial`'s closure walk only seeded from root `require`, so an
/// allow-listed name reachable only through a locked-out parent's own
/// `require` was never fetched (`PoolBuilder::loadPackage` still marks a
/// locked package's requires for loading regardless, `PoolBuilder.php:520-551`).
/// `psr/log` has no locked requires of its own, so `-w`/`-W`
/// (`UpdateAllowMode::With*`) land on the exact same lock as a plain
/// listed-only update; all three are asserted here against the same
/// recorded `composer update psr/log --no-install` output.
#[tokio::test]
async fn partial_update_finds_a_transitive_allow_listed_package() {
    for mode in [
        vivace::solver::pool_builder::UpdateAllowMode::OnlyListed,
        vivace::solver::pool_builder::UpdateAllowMode::WithTransitiveDepsNoRootRequire,
        vivace::solver::pool_builder::UpdateAllowMode::WithTransitiveDeps,
    ] {
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/partial-update-transitive");
        let cache = tempfile::tempdir().unwrap();
        let transport = FixtureTransport {
            root: fixtures_root(),
        };
        let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
            .await
            .unwrap();

        let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
        let root: Value = serde_json::from_slice(&composer_json).unwrap();
        let locked_by_name = common::locked_by_name(&fixture.join("lock-before.json"));

        let result = vivace::solver::solve_partial_update(
            &repo,
            &root,
            false,
            false,
            &locked_by_name,
            &["psr/log".to_string()],
            mode,
        )
        .await
        .unwrap();

        let monolog = result
            .non_dev
            .iter()
            .find(|p| p.name == "monolog/monolog")
            .unwrap();
        assert_eq!(monolog.pretty_version, "3.11.0", "{mode:?}");
        let psr_log = result.non_dev.iter().find(|p| p.name == "psr/log").unwrap();
        assert_eq!(psr_log.pretty_version, "3.0.2", "{mode:?}");

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
        let got = vivace::lock_writer::write(
            &result.non_dev,
            Some(&result.dev),
            &options,
            &composer_json,
        )
        .unwrap();
        assert_matches_expected(&got, &fixture.join("composer.lock"));
    }
}

/// #61 (`--minimal-changes`): `lock-before.json` locks `psr/log` at 3.0.0,
/// still allowed by root's `psr/log: ^3.0` — a plain full update bumps it
/// to 3.0.2 (the newest match, exactly what `tests/fixtures/monolog`'s own
/// committed lock already proves for this same closure). With
/// `Installer::createPolicy`'s `$preferredVersions` pin
/// (`DefaultPolicy::with_preferred_versions`), it must stay locked instead.
/// `composer.lock` here was generated by the real Composer 2.10.2 against
/// this same recorded closure (a `file://` `composer`-type repository
/// pointed at `tests/fixtures/packagist/repo.packagist.org`, then
/// `composer update --lock` to re-hash against the committed
/// `composer.json`): only `psr/log`'s version/reference/time differ from
/// `tests/fixtures/partial-update/composer.lock`.
#[tokio::test]
async fn minimal_changes_keeps_the_locked_version() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/minimal-changes");
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();
    let locked_by_name = common::locked_by_name(&fixture.join("lock-before.json"));
    let seed: Vec<String> = locked_by_name.keys().cloned().collect();
    let preferred: HashMap<String, vivace::semver::NormalizedVersion> = locked_by_name
        .iter()
        .map(|(name, entry)| {
            let version = vivace::semver::normalize(entry["version"].as_str().unwrap()).unwrap();
            (name.clone(), version)
        })
        .collect();

    let result = solver::solve_update_seeded(&repo, &root, false, false, &seed, preferred)
        .await
        .unwrap();

    let psr_log = result.non_dev.iter().find(|p| p.name == "psr/log").unwrap();
    assert_eq!(
        psr_log.pretty_version, "3.0.0",
        "--minimal-changes must keep psr/log locked at its already-satisfying version"
    );

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

/// #23/#104: a single-file zip, in memory, for pre-populating the store
/// without ever touching the network. Duplicated from
/// `tests/install_e2e.rs`'s copy of the same name (private there, and small
/// enough not to widen for one more caller).
fn zip_of_one_file(name: &str, content: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(content).unwrap();
    writer.finish().unwrap().into_inner()
}

/// #104: a `tests/fixtures/partial-update` project (`lock-before.json` as
/// `composer.lock`) with its on-disk repository cache pre-warmed from the
/// recorded Packagist fixtures (same `FixtureTransport`/`solve_partial_update`
/// call `partial_update_keeps_the_unlisted_package_locked` already makes,
/// run here only for its side effect of writing `packages.json`/provider
/// caches to `ctx.cache`) and its store pre-populated with a synthetic
/// archive for every package `tests/fixtures/monolog/composer.lock` already
/// proves this partial update converges to — so the real `viv` binary can
/// run `update psr/log --offline` (solve *and* the chained install) with no
/// network at all.
async fn offline_partial_update_context() -> TestContext {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/partial-update");
    let monolog_fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");

    fs_err::copy(fixture.join("composer.json"), project.join("composer.json")).unwrap();
    fs_err::copy(
        fixture.join("lock-before.json"),
        project.join("composer.lock"),
    )
    .unwrap();
    for dir in ["src", "lib"] {
        copy_tree(&monolog_fixture.join(dir), &project.join(dir));
    }

    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", ctx.cache.path(), &transport)
        .await
        .unwrap();
    let root: Value =
        serde_json::from_slice(&fs_err::read(project.join("composer.json")).unwrap()).unwrap();
    let locked_by_name = common::locked_by_name(&fixture.join("lock-before.json"));
    vivace::solver::solve_partial_update(
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

    let store = Store::open(ctx.cache.path()).unwrap();
    let lock = vivace::lock::read_lock(&monolog_fixture.join("composer.lock")).unwrap();
    for package in lock.packages(true) {
        store
            .add_zip(
                package,
                &zip_of_one_file("marker.txt", package.name.as_bytes()),
            )
            .unwrap();
    }
    drop(store);

    ctx
}

/// #104: `viv update psr/log` chains into `install` once `composer.lock` is
/// written (`--no-install` opts out, asserted separately below), the way
/// `composer update` chains into `Installer::run()` with `update` set.
#[tokio::test]
async fn update_chains_into_install() {
    let ctx = offline_partial_update_context().await;
    let project = ctx.project.path();

    ctx.viv()
        .args(["update", "psr/log", "--offline"])
        .assert()
        .success();

    for name in ["monolog/monolog", "psr/log", "psr/container"] {
        assert!(
            project
                .join("vendor")
                .join(name)
                .join("marker.txt")
                .is_file(),
            "{name} should be installed under vendor/ from the chained install"
        );
    }

    let installed: Value = serde_json::from_slice(
        &fs_err::read(project.join("vendor/composer/installed.json")).unwrap(),
    )
    .unwrap();
    let lock: Value =
        serde_json::from_slice(&fs_err::read(project.join("composer.lock")).unwrap()).unwrap();
    let mut installed_names: Vec<String> = installed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    installed_names.sort();
    let mut locked_names: Vec<String> = lock["packages"]
        .as_array()
        .unwrap()
        .iter()
        .chain(lock["packages-dev"].as_array().unwrap())
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    locked_names.sort();
    assert_eq!(
        installed_names, locked_names,
        "vendor/composer/installed.json should match the freshly written lock"
    );
}

/// #155: `composer.lock`'s `content-hash` must describe the
/// `composer.json` bytes actually left on disk, not the unnormalised bytes
/// `viv update` read before normalising. A deliberately unnormalised
/// `require` (reverse key order, so the normaliser's platform-first sort
/// actually moves something) must still leave a fresh lock behind, so the
/// chained install prints no stale-lock warning.
#[tokio::test]
async fn update_normalizes_composer_json_before_computing_the_content_hash() {
    let ctx = offline_partial_update_context().await;
    let project = ctx.project.path();
    let composer_json_path = project.join("composer.json");

    let original: Value =
        serde_json::from_slice(&fs_err::read(&composer_json_path).unwrap()).unwrap();
    let mut require_reversed = serde_json::Map::new();
    for (name, constraint) in original["require"].as_object().unwrap().iter().rev() {
        require_reversed.insert(name.clone(), constraint.clone());
    }
    let unnormalized = serde_json::json!({
        "require": require_reversed,
        "name": original["name"],
        "license": original["license"],
        "type": original["type"],
        "require-dev": original["require-dev"],
        "autoload": original["autoload"],
        "config": original["config"],
    });
    fs_err::write(
        &composer_json_path,
        serde_json::to_vec(&unnormalized).unwrap(),
    )
    .unwrap();

    let output = ctx
        .viv()
        .args(["update", "psr/log", "--offline"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "viv update failed:\n{combined}");
    assert!(
        !combined.contains("is not up to date"),
        "the chained install must not see a stale lock: {combined}"
    );

    let lock = vivace::lock::read_lock(&project.join("composer.lock")).unwrap();
    let on_disk_composer_json = fs_err::read(&composer_json_path).unwrap();
    assert!(
        vivace::lock::is_fresh(&lock, &on_disk_composer_json).unwrap(),
        "content-hash should describe the normalized composer.json actually on disk"
    );
}

/// #104: `--no-install` is today's `viv update` behaviour, kept as an
/// explicit opt-out now that installing is the default.
#[tokio::test]
async fn update_no_install_leaves_vendor_absent() {
    let ctx = offline_partial_update_context().await;
    let project = ctx.project.path();

    ctx.viv()
        .args(["update", "psr/log", "--offline", "--no-install"])
        .assert()
        .success();

    assert!(
        !project.join("vendor").exists(),
        "vendor/ must stay absent under --no-install"
    );
}

/// #104: `pre-update-cmd`/`post-update-cmd` fire around the whole
/// resolve-then-install run, in place of the chained install's own
/// `pre-install-cmd`/`post-install-cmd` (`Installer::run`'s own event-name
/// switch on its `update` flag: Composer never fires both pairs for one
/// invocation).
#[tokio::test]
async fn update_dispatches_update_scripts_not_install_scripts() {
    let ctx = offline_partial_update_context().await;
    let project = ctx.project.path();
    let composer_json_path = project.join("composer.json");
    let mut root: Value =
        serde_json::from_slice(&fs_err::read(&composer_json_path).unwrap()).unwrap();
    root["scripts"] = serde_json::json!({
        "pre-update-cmd": "echo PRE-UPDATE-CMD-RAN",
        "post-update-cmd": "echo POST-UPDATE-CMD-RAN",
        "pre-install-cmd": "echo PRE-INSTALL-CMD-MUST-NOT-RUN",
        "post-install-cmd": "echo POST-INSTALL-CMD-MUST-NOT-RUN",
    });
    fs_err::write(
        &composer_json_path,
        serde_json::to_vec_pretty(&root).unwrap(),
    )
    .unwrap();

    let output = ctx
        .viv()
        .args(["update", "psr/log", "--offline"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        !stdout.contains("PRE-INSTALL-CMD-MUST-NOT-RUN"),
        "install's own pre-install-cmd must not fire during an update: {stdout}"
    );
    assert!(
        !stdout.contains("POST-INSTALL-CMD-MUST-NOT-RUN"),
        "install's own post-install-cmd must not fire during an update: {stdout}"
    );
    let pre = stdout
        .find("PRE-UPDATE-CMD-RAN")
        .expect("pre-update-cmd should fire before resolving");
    let post = stdout
        .find("POST-UPDATE-CMD-RAN")
        .expect("post-update-cmd should fire after the install step");
    assert!(
        pre < post,
        "pre-update-cmd must fire before post-update-cmd: {stdout}"
    );
}

/// #105: wpackagist.org is a real v1 repository whose provider files are
/// non-minified (object-keyed by version label, `tests/repository.rs`'s
/// `v1_provider_file_object_keyed_by_version_label` fixture covers that
/// shape hermetically). This is the live-network half: `viv update` against
/// the real wpackagist.org must byte-match real Composer's own lock, the
/// same "record once, gate on network" split as `tests/install_e2e.rs`.
#[tokio::test]
async fn viv_update_reproduces_composers_lock_against_real_wpackagist() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv_update_reproduces_composers_lock_against_real_wpackagist: set \
             VIVACE_TEST_NETWORK=1 to hit the real wpackagist.org"
        );
        return;
    }
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!(
            "skipping viv_update_reproduces_composers_lock_against_real_wpackagist: composer \
             is not on PATH"
        );
        return;
    }

    let root = serde_json::json!({
        "name": "vivace/wpackagist-fixture",
        "repositories": [
            {"type": "composer", "url": "https://wpackagist.org"}
        ],
        "require": {"wpackagist-plugin/akismet": "*"}
    });
    let composer_json = serde_json::to_vec_pretty(&root).unwrap();

    let project_dir = tempfile::tempdir().unwrap();
    fs_err::write(project_dir.path().join("composer.json"), &composer_json).unwrap();
    let composer_home = tempfile::tempdir().unwrap();

    let status = Command::new("composer")
        .args(["update", "--no-install", "--no-plugins"])
        .current_dir(project_dir.path())
        .env("COMPOSER_HOME", composer_home.path())
        .env("COMPOSER_CACHE_DIR", composer_home.path().join("cache"))
        .status()
        .unwrap();
    assert!(status.success(), "composer update failed");
    let want = fs_err::read_to_string(project_dir.path().join("composer.lock")).unwrap();

    let auth = vivace::auth::Auth::load(project_dir.path()).unwrap();
    let fetcher = vivace::fetch::Fetcher::new(auth).unwrap();
    let transport = vivace::repository::HttpTransport { fetcher: &fetcher };
    let cache = tempfile::tempdir().unwrap();
    let repo = Repository::from_composer_json(&root, cache.path(), transport)
        .await
        .unwrap();
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
    let got =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();

    assert_eq!(
        got, want,
        "viv update's lock does not byte-match Composer's against real wpackagist.org"
    );
}
