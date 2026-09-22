//! `viv lock merge`: chunk 1's marker output (#275) and chunk 2's re-solve
//! of the divergent closure. `psr/log` changed on both sides to a
//! different version in `tests/fixtures/lock-merge/`; everything else
//! merges clean.
//!
//! `--no-resolve` keeps chunk 1's exact golden (byte-for-byte canonical
//! except the one marker block) hermetic and network-free. The re-solve
//! itself is exercised at the library level
//! (`lock_merge_resolves_the_divergent_closure...`, below), against a
//! `FixtureTransport` the same way `tests/update.rs` does, since the CLI
//! path always builds a real `HttpTransport`.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[macro_use]
mod common;

use common::{FixtureTransport, TestContext, fixtures_root};
use serde_json::Value;
use vivace::repository::Repository;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lock-merge")
}

#[test]
fn a_divergent_package_gets_one_marker_block_and_exit_1_with_no_resolve() {
    let ctx = TestContext::new();
    let dir = ctx.project.path();
    for name in ["composer.json", "base.lock", "ours.lock", "theirs.lock"] {
        fs::copy(fixtures().join(name), dir.join(name)).unwrap();
    }

    let output = ctx
        .viv()
        .args(["lock", "merge"])
        .arg(dir.join("base.lock"))
        .arg(dir.join("ours.lock"))
        .arg(dir.join("theirs.lock"))
        .arg("-d")
        .arg(dir)
        .arg("--no-resolve")
        .output()
        .expect("failed to run viv");

    assert_eq!(
        output.status.code(),
        Some(1),
        "a divergent package must exit 1: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let got = fs::read_to_string(dir.join("ours.lock")).unwrap();
    let expected = fs::read_to_string(fixtures().join("expected.lock")).unwrap();
    assert_eq!(got, expected);

    assert_eq!(
        got.matches("<<<<<<< ours").count(),
        1,
        "exactly one marker block"
    );
}

/// Chunk 2's seam, `lock_merge::resolve_divergent_closure`, driven directly
/// with a `FixtureTransport` (`tests/update.rs`'s own pattern) instead of
/// the CLI path's real `HttpTransport`: `psr/log` is the one divergent
/// name, `locked_by_name` is everything else from the merge's own base
/// fixture, and the resolved version must satisfy the fixture
/// `composer.json`'s own `"psr/log": "^2.0 || ^3.0"`.
#[tokio::test]
async fn lock_merge_resolves_the_divergent_closure_against_a_fixture_transport() {
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let cache = tempfile::tempdir().unwrap();
    let composer_json = fs_err::read(fixtures().join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();
    let repo =
        Repository::from_composer_json_with_ttl(&root, cache.path(), &transport, Duration::ZERO)
            .await
            .unwrap();

    let mut locked_by_name = common::locked_by_name(&fixtures().join("base.lock"));
    locked_by_name.remove("psr/log");
    let allow_list = vec!["psr/log".to_string()];

    let result = vivace::lock_merge::resolve_divergent_closure(
        &repo,
        &root,
        false,
        &locked_by_name,
        &allow_list,
        None,
    )
    .await
    .unwrap();

    let psr_log = result
        .non_dev
        .iter()
        .chain(result.dev.iter())
        .find(|p| p.name == "psr/log")
        .expect("psr/log must be in the re-solved set");
    // The fixture's own require is "psr/log": "^2.0 || ^3.0"; every other
    // locked package (monolog/monolog, psr/container) must survive
    // untouched since neither is in the allow list or psr/log's closure.
    assert!(
        psr_log.pretty_version.starts_with("2.") || psr_log.pretty_version.starts_with("3."),
        "psr/log resolved to {}, outside ^2.0 || ^3.0",
        psr_log.pretty_version
    );
    assert!(
        result
            .non_dev
            .iter()
            .any(|p| p.name == "monolog/monolog" && p.pretty_version == "3.11.0"),
        "monolog/monolog must survive untouched, outside the allow list"
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
    let lock_text =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();
    assert!(
        !lock_text.contains("<<<<<<<"),
        "a re-solved lock has no markers"
    );
}

/// `--as-of` (#275): `psr/log` 3.0.2 released 2024-09-11T13:17:53Z, 3.0.1
/// released 2024-08-21T13:31:24Z (`tests/fixtures/packagist/.../p2/psr/log.json`).
/// An `as_of` between the two must resolve the older one; without it (or
/// with a cutoff after both) the newer one wins, same as any other solve.
#[tokio::test]
async fn resolve_divergent_closure_declines_a_version_released_after_as_of() {
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let cache = tempfile::tempdir().unwrap();
    let root: Value = serde_json::json!({"require": {"psr/log": "^3.0"}});
    let repo =
        Repository::from_composer_json_with_ttl(&root, cache.path(), &transport, Duration::ZERO)
            .await
            .unwrap();
    let locked_by_name = std::collections::HashMap::new();
    let allow_list = vec!["psr/log".to_string()];

    let latest = vivace::lock_merge::resolve_divergent_closure(
        &repo,
        &root,
        false,
        &locked_by_name,
        &allow_list,
        None,
    )
    .await
    .unwrap();
    assert_eq!(
        latest
            .non_dev
            .iter()
            .find(|p| p.name == "psr/log")
            .unwrap()
            .pretty_version,
        "3.0.2",
        "no cutoff: today's latest matching version"
    );

    // 2024-09-01T00:00:00Z, between 3.0.1's and 3.0.2's release: computed
    // once (`date -u -d @1725148800` -> that instant) rather than adding a
    // date-parsing call to this test just to produce its own fixture input.
    let as_of = 1_725_148_800;
    let older = vivace::lock_merge::resolve_divergent_closure(
        &repo,
        &root,
        false,
        &locked_by_name,
        &allow_list,
        Some(as_of),
    )
    .await
    .unwrap();
    assert_eq!(
        older
            .non_dev
            .iter()
            .find(|p| p.name == "psr/log")
            .unwrap()
            .pretty_version,
        "3.0.1",
        "as_of before 3.0.2's release: the last version that existed then"
    );
}

/// `lock_merge::run`'s own `--as-of` parse, via the CLI so the error path
/// (`lock_writer::parse_time_to_epoch` returning `None`) is exercised the
/// way a real invocation hits it, before any lock or format is even read.
#[test]
fn an_unparseable_as_of_is_a_clear_error() {
    let ctx = TestContext::new();
    let dir = ctx.project.path();
    for name in ["composer.json", "base.lock", "ours.lock", "theirs.lock"] {
        fs::copy(fixtures().join(name), dir.join(name)).unwrap();
    }

    let output = ctx
        .viv()
        .args(["lock", "merge"])
        .arg(dir.join("base.lock"))
        .arg(dir.join("ours.lock"))
        .arg(dir.join("theirs.lock"))
        .arg("-d")
        .arg(dir)
        .arg("--as-of")
        .arg("not-a-timestamp")
        .output()
        .expect("failed to run viv");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not-a-timestamp") && stderr.contains("not a recognised timestamp"),
        "stderr: {stderr}"
    );
}

#[test]
fn a_one_sided_change_merges_clean_and_exits_0() {
    let ctx = TestContext::new();
    let dir = ctx.project.path();
    fs::copy(fixtures().join("composer.json"), dir.join("composer.json")).unwrap();
    fs::copy(fixtures().join("base.lock"), dir.join("base.lock")).unwrap();
    fs::copy(fixtures().join("ours.lock"), dir.join("ours.lock")).unwrap();
    // theirs == base: only ours changed psr/log, so this must merge clean.
    fs::copy(fixtures().join("base.lock"), dir.join("theirs.lock")).unwrap();

    let output = ctx
        .viv()
        .args(["lock", "merge"])
        .arg(dir.join("base.lock"))
        .arg(dir.join("ours.lock"))
        .arg(dir.join("theirs.lock"))
        .arg("-d")
        .arg(dir)
        .output()
        .expect("failed to run viv");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let got = fs::read_to_string(dir.join("ours.lock")).unwrap();
    assert!(!got.contains("<<<<<<<"));
    assert!(got.contains("\"version\": \"3.0.99\""), "ours' change wins");
}
