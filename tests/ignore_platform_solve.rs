//! #242: `--ignore-platform-reqs`/`--ignore-platform-req` reach the solve
//! itself, not just `install`'s `autoload/platform_check.php` generation —
//! `RuleSetGenerator::addRulesForPackage`/`addRulesForRequest`'s own
//! `isIgnored($link->getTarget())` skip, ported as a pool-level filter in
//! `pool_builder::build_partial_seeded` (`src/solver/pool_builder.rs`'s
//! `strip_ignored_platform_links`).
//!
//! `tests/fixtures/ignore-platform-solve/` is `tests/fixtures/solver-problems/`
//! plus one more package (`acme/platform-dep`, added there rather than as
//! its own corpus, same as `tests/problem_messages.rs`'s own small
//! hand-authored packages) requiring `ext-viv-nonexistent`, an extension no
//! host has. `composer.lock` was captured verbatim from a real
//! `composer update --no-interaction --no-install --ignore-platform-reqs`
//! (Composer 2.10.2) against that corpus served over a local
//! `php -S 127.0.0.1:8987`, the exact URL this fixture's `composer.json`
//! names in its own `repositories` block (`content_hash_from_value`
//! includes that block, so the recorded `content-hash` only matches a
//! `root` value carrying the identical string — never actually dialled by
//! the hermetic tests below, which replay `tests/fixtures/solver-problems/`
//! through `FixtureTransport` instead).

mod common;

use std::collections::HashMap;
use std::path::Path;

use common::FixtureTransport;
use serde_json::Value;
use vivace::autoload::platform::IgnorePlatform;
use vivace::repository::Repository;
use vivace::solver::{self, problem::SolverError};

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/ignore-platform-solve")
}

fn composer_json() -> Vec<u8> {
    fs_err::read(fixture_dir().join("composer.json")).unwrap()
}

fn transport() -> FixtureTransport {
    FixtureTransport {
        root: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/solver-problems"),
    }
}

/// No `--ignore-platform-reqs`/`--ignore-platform-req`: `acme/platform-dep`'s
/// `ext-viv-nonexistent` requirement has no provider, same as today.
#[tokio::test]
async fn update_fails_without_ignoring_the_missing_extension() {
    let composer_json = composer_json();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();
    let transport = transport();
    let cache = tempfile::tempdir().unwrap();
    let repo = Repository::load("http://127.0.0.1:8987", cache.path(), &transport)
        .await
        .unwrap();

    let Err(err) = solver::solve_update_seeded(
        &repo,
        &root,
        false,
        false,
        &[],
        HashMap::new(),
        None::<vivace::solver::pool_builder::AdvisoryFilter<'_, vivace::audit::NoAdvisories>>,
        None,
        &IgnorePlatform::None,
    )
    .await
    else {
        panic!("expected the missing extension to still block the solve")
    };
    assert!(
        err.downcast_ref::<SolverError>().is_some(),
        "expected a SolverError, got: {err}"
    );
}

/// `--ignore-platform-reqs`: the pool never sees the `ext-viv-nonexistent`
/// link at all, so `acme/platform-dep` resolves — and the written lock
/// matches Composer's own `--ignore-platform-reqs` run byte for byte.
#[tokio::test]
async fn ignore_platform_reqs_resolves_and_matches_composers_lock() {
    let composer_json = composer_json();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();
    let transport = transport();
    let cache = tempfile::tempdir().unwrap();
    let repo = Repository::load("http://127.0.0.1:8987", cache.path(), &transport)
        .await
        .unwrap();

    let result = solver::solve_update_seeded(
        &repo,
        &root,
        false,
        false,
        &[],
        HashMap::new(),
        None::<vivace::solver::pool_builder::AdvisoryFilter<'_, vivace::audit::NoAdvisories>>,
        None,
        &IgnorePlatform::All,
    )
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
    let want = fs_err::read_to_string(fixture_dir().join("composer.lock")).unwrap();
    assert_eq!(
        got, want,
        "does not match a real --ignore-platform-reqs run"
    );
}

/// `--ignore-platform-req=ext-viv-*`: same result as `--ignore-platform-reqs`
/// here (the fixture has only the one platform requirement to ignore), a
/// glob rather than the blanket form.
#[tokio::test]
async fn ignore_platform_req_wildcard_resolves_and_matches_composers_lock() {
    let composer_json = composer_json();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();
    let transport = transport();
    let cache = tempfile::tempdir().unwrap();
    let repo = Repository::load("http://127.0.0.1:8987", cache.path(), &transport)
        .await
        .unwrap();
    let ignore = IgnorePlatform::List(vec!["ext-viv-*".to_string()]);

    let result = solver::solve_update_seeded(
        &repo,
        &root,
        false,
        false,
        &[],
        HashMap::new(),
        None::<vivace::solver::pool_builder::AdvisoryFilter<'_, vivace::audit::NoAdvisories>>,
        None,
        &ignore,
    )
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
    let want = fs_err::read_to_string(fixture_dir().join("composer.lock")).unwrap();
    assert_eq!(
        got, want,
        "does not match a real --ignore-platform-reqs run"
    );
}
