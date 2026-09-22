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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[macro_use]
mod common;

use common::{FixtureTransport, TestContext, fixtures_root};
use serde_json::{Value, json};
use vivace::lock_merge::{Entry, Identity, Moved, Scope, escalate_resolve};
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

/// #296's escalation rungs, over a small hand-authored dependency chain
/// (`d/dep` -> `e/dependent` -> `f/chain`, `g/untouched` off to the side;
/// `tests/fixtures/packagist/repo.packagist.org/p2/{d,e,f,g}`), the same
/// idiom as `tests/update.rs`'s own `a/x`/`b/y`/`c/z`: the real
/// monolog/psr-log fixture has no version whose own metadata blocks a
/// resolve the way this needs.
fn pinned(name: &str, version: &str, require: &Value) -> Entry<Value> {
    Entry {
        identity: Identity {
            version: version.to_string(),
            source_ref: None,
            dev: false,
        },
        payload: json!({"name": name, "version": version, "require": require}),
    }
}

/// `d/dep` is the one divergent name (ours 2.0.0, theirs 2.0.1). Root wants
/// `d/dep` `^2.0`, but `e/dependent` is pinned at 1.0.0 on both sides, whose
/// own locked `require` is `d/dep ^1.0` — rung 1 (the divergent closure
/// alone) cannot satisfy both at once. `e/dependent` directly requires
/// `d/dep`, so rung 2 unlocks it too; its only registry version compatible
/// with `d/dep ^2.0` is 2.0.0, so it moves.
#[tokio::test]
async fn rung_2_resolves_when_a_pinned_direct_dependent_blocks_the_closure() {
    let root = json!({"require": {"d/dep": "^2.0", "e/dependent": "^1.0 || ^2.0"}});
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let cache = tempfile::tempdir().unwrap();
    let repo =
        Repository::from_composer_json_with_ttl(&root, cache.path(), &transport, Duration::ZERO)
            .await
            .unwrap();

    let e_dependent = pinned(
        "e/dependent",
        "1.0.0",
        &json!({"php": ">=7.4.0", "d/dep": "^1.0"}),
    );
    let mut merged = BTreeMap::new();
    merged.insert("e/dependent".to_string(), e_dependent.clone());

    let mut divergent = BTreeSet::new();
    divergent.insert("d/dep".to_string());

    let mut ours = BTreeMap::new();
    ours.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.0", &json!({"php": ">=7.4.0"})),
    );
    ours.insert("e/dependent".to_string(), e_dependent.clone());

    let mut theirs = BTreeMap::new();
    theirs.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.1", &json!({"php": ">=7.4.0"})),
    );
    theirs.insert("e/dependent".to_string(), e_dependent);

    let resolved = escalate_resolve(
        &repo,
        &root,
        false,
        &merged,
        &divergent,
        &ours,
        &theirs,
        None,
        Some(cache.path()),
        Scope::Seeded,
    )
    .await
    .unwrap();

    assert_eq!(
        resolved.scope,
        Scope::Dependents,
        "rung 1 alone cannot satisfy e/dependent's own pinned require"
    );
    assert_eq!(
        resolved.moved,
        vec![Moved {
            name: "e/dependent".to_string(),
            before: "1.0.0".to_string(),
            after: "2.0.0".to_string(),
        }],
        "e/dependent, a direct dependent of the divergent closure, must be reported as moved"
    );
}

/// `--max-scope closure` on the same case as above: rung 1 fails and the
/// cap forbids rung 2, so the caller (not exercised here — this is the
/// library seam) must fall back to markers. The returned error names the
/// cap so `merge_composer_lock`'s own fallback message can say why.
#[tokio::test]
async fn max_scope_closure_caps_before_rung_2_and_names_the_cap() {
    let root = json!({"require": {"d/dep": "^2.0", "e/dependent": "^1.0 || ^2.0"}});
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let cache = tempfile::tempdir().unwrap();
    let repo =
        Repository::from_composer_json_with_ttl(&root, cache.path(), &transport, Duration::ZERO)
            .await
            .unwrap();

    let e_dependent = pinned(
        "e/dependent",
        "1.0.0",
        &json!({"php": ">=7.4.0", "d/dep": "^1.0"}),
    );
    let mut merged = BTreeMap::new();
    merged.insert("e/dependent".to_string(), e_dependent.clone());

    let mut divergent = BTreeSet::new();
    divergent.insert("d/dep".to_string());

    let mut ours = BTreeMap::new();
    ours.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.0", &json!({"php": ">=7.4.0"})),
    );
    ours.insert("e/dependent".to_string(), e_dependent.clone());

    let mut theirs = BTreeMap::new();
    theirs.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.1", &json!({"php": ">=7.4.0"})),
    );
    theirs.insert("e/dependent".to_string(), e_dependent);

    let result = escalate_resolve(
        &repo,
        &root,
        false,
        &merged,
        &divergent,
        &ours,
        &theirs,
        None,
        Some(cache.path()),
        Scope::Closure,
    )
    .await;

    let Err(err) = result else {
        panic!("rung 1 alone must fail on this fixture, and --max-scope=closure must not escalate");
    };
    assert!(
        format!("{err:#}").contains("--max-scope=closure"),
        "error must name the cap: {err:#}"
    );
}

/// A second hop: `f/chain` (pinned 1.0.0, requires `e/dependent ^1.0`) is
/// not a *direct* dependent of `d/dep`, so rung 2's one-hop expansion never
/// unlocks it, and rung 2 fails exactly as rung 1 did. Only rung 3 (a full
/// solve, nothing pinned hard) can move `f/chain` and `e/dependent`
/// together. `g/untouched`, pinned 1.0.0 with a newer 1.5.0 available and
/// no connection to the conflict at all, must stay at 1.0.0: that is
/// `preferred` at work, and the fact that it does not move is what proves
/// rung 3 is not a plain update.
#[tokio::test]
async fn rung_3_moves_a_two_hop_dependent_and_leaves_an_unrelated_package_pinned() {
    let root = json!({
        "require": {"d/dep": "^2.0", "f/chain": "^1.0 || ^2.0", "g/untouched": "^1.0"},
    });
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let cache = tempfile::tempdir().unwrap();
    let repo =
        Repository::from_composer_json_with_ttl(&root, cache.path(), &transport, Duration::ZERO)
            .await
            .unwrap();

    let e_dependent = pinned(
        "e/dependent",
        "1.0.0",
        &json!({"php": ">=7.4.0", "d/dep": "^1.0"}),
    );
    let f_chain = pinned(
        "f/chain",
        "1.0.0",
        &json!({"php": ">=7.4.0", "e/dependent": "^1.0"}),
    );
    let g_untouched = pinned("g/untouched", "1.0.0", &json!({"php": ">=7.4.0"}));

    let mut merged = BTreeMap::new();
    merged.insert("e/dependent".to_string(), e_dependent.clone());
    merged.insert("f/chain".to_string(), f_chain.clone());
    merged.insert("g/untouched".to_string(), g_untouched.clone());

    let mut divergent = BTreeSet::new();
    divergent.insert("d/dep".to_string());

    let mut ours = BTreeMap::new();
    ours.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.0", &json!({"php": ">=7.4.0"})),
    );
    ours.insert("e/dependent".to_string(), e_dependent.clone());
    ours.insert("f/chain".to_string(), f_chain.clone());
    ours.insert("g/untouched".to_string(), g_untouched.clone());

    let mut theirs = BTreeMap::new();
    theirs.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.1", &json!({"php": ">=7.4.0"})),
    );
    theirs.insert("e/dependent".to_string(), e_dependent);
    theirs.insert("f/chain".to_string(), f_chain);
    theirs.insert("g/untouched".to_string(), g_untouched);

    let resolved = escalate_resolve(
        &repo,
        &root,
        false,
        &merged,
        &divergent,
        &ours,
        &theirs,
        None,
        Some(cache.path()),
        Scope::Seeded,
    )
    .await
    .unwrap();

    assert_eq!(
        resolved.scope,
        Scope::Seeded,
        "a two-hop dependent is outside rung 2's one-hop expansion"
    );
    let mut moved_names: Vec<&str> = resolved.moved.iter().map(|m| m.name.as_str()).collect();
    moved_names.sort_unstable();
    assert_eq!(
        moved_names,
        vec!["e/dependent", "f/chain"],
        "only the packages the conflict actually forces should move: {:?}",
        resolved.moved
    );

    let untouched = resolved
        .result
        .non_dev
        .iter()
        .find(|p| p.name == "g/untouched")
        .expect("g/untouched must still be in the result");
    assert_eq!(
        untouched.pretty_version, "1.0.0",
        "preferred must keep an unrelated package at its locked version, not update it to 1.5.0"
    );
}
