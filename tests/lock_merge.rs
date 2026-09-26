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

use common::{FixtureTransport, PanicTransport, TestContext, fixtures_root};
use serde_json::{Value, json};
use vivace::lock_merge::{Entry, Identity, Moved, Scope, escalate_resolve, try_offline_rung};
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
    let repo = Repository::from_composer_json_with_ttl(
        Path::new("."),
        &root,
        cache.path(),
        &transport,
        Duration::ZERO,
    )
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
    let repo = Repository::from_composer_json_with_ttl(
        Path::new("."),
        &root,
        cache.path(),
        &transport,
        Duration::ZERO,
    )
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
    let repo = Repository::from_composer_json_with_ttl(
        Path::new("."),
        &root,
        cache.path(),
        &transport,
        Duration::ZERO,
    )
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
    let repo = Repository::from_composer_json_with_ttl(
        Path::new("."),
        &root,
        cache.path(),
        &transport,
        Duration::ZERO,
    )
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
    let repo = Repository::from_composer_json_with_ttl(
        Path::new("."),
        &root,
        cache.path(),
        &transport,
        Duration::ZERO,
    )
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

// --- rung 0, --offline-rung (#314) --------------------------------------

/// A `Repository` over [`PanicTransport`], with every source disabled
/// (`lock_merge::try_offline_resolve_composer_lock`'s own
/// `neutered_repositories`, reproduced here rather than exposed, so this
/// test proves the guarantee independently of that helper): zero sources
/// means the pool-building walk below has nothing to fetch from even
/// before `PanicTransport` would refuse it, and a panic here would mean
/// the repository-construction step itself changed, not the function
/// under test.
async fn offline_repo(root: &Value, cache: &Path) -> Repository<&'static PanicTransport> {
    let mut neutered = root.clone();
    neutered["repositories"] = json!([{"packagist.org": false}]);
    Repository::from_composer_json_with_ttl(
        Path::new("."),
        &neutered,
        cache,
        &PanicTransport,
        Duration::ZERO,
    )
    .await
    .unwrap()
}

/// (a): `d/dep` is the one divergent name, ours pinned 1.0.0, theirs 2.0.0.
/// Root wants `^1.0 || ^2.0` (either satisfies) and nothing else pins a
/// `require` against it, so `ours` — tried first — already satisfies
/// everything the two locks record. Rung 0 must accept it without ever
/// calling `PanicTransport::get`.
#[tokio::test]
async fn offline_rung_accepts_ours_pin_when_it_already_satisfies_everything() {
    let root = json!({"require": {"d/dep": "^1.0 || ^2.0"}});
    let cache = tempfile::tempdir().unwrap();
    let repo = offline_repo(&root, cache.path()).await;

    let merged = BTreeMap::new();
    let mut divergent = BTreeSet::new();
    divergent.insert("d/dep".to_string());
    let mut ours = BTreeMap::new();
    ours.insert(
        "d/dep".to_string(),
        pinned("d/dep", "1.0.0", &json!({"php": ">=7.4.0"})),
    );
    let mut theirs = BTreeMap::new();
    theirs.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.0", &json!({"php": ">=7.4.0"})),
    );

    let result = try_offline_rung(&repo, &root, false, &merged, &divergent, &ours, &theirs)
        .await
        .expect("ours' pin satisfies the merged set; rung 0 must accept it");
    let d_dep = result
        .non_dev
        .iter()
        .find(|p| p.name == "d/dep")
        .expect("d/dep must be in the result");
    assert_eq!(d_dep.pretty_version, "1.0.0", "ours is tried first");
}

/// (b): `e/dependent` (non-divergent, pinned both sides) locks its own
/// `require` to `d/dep ^2.0`. Ours' pin (1.0.0) violates that sibling's
/// require, so the all-`ours` attempt fails; theirs' pin (2.0.0) satisfies
/// it, so the all-`theirs` attempt must be the one rung 0 accepts.
#[tokio::test]
async fn offline_rung_falls_back_to_theirs_when_ours_violates_a_siblings_require() {
    let root = json!({"require": {"d/dep": "^1.0 || ^2.0", "e/dependent": "^1.0"}});
    let cache = tempfile::tempdir().unwrap();
    let repo = offline_repo(&root, cache.path()).await;

    let e_dependent = pinned(
        "e/dependent",
        "1.0.0",
        &json!({"php": ">=7.4.0", "d/dep": "^2.0"}),
    );
    let mut merged = BTreeMap::new();
    merged.insert("e/dependent".to_string(), e_dependent);

    let mut divergent = BTreeSet::new();
    divergent.insert("d/dep".to_string());
    let mut ours = BTreeMap::new();
    ours.insert(
        "d/dep".to_string(),
        pinned("d/dep", "1.0.0", &json!({"php": ">=7.4.0"})),
    );
    let mut theirs = BTreeMap::new();
    theirs.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.0", &json!({"php": ">=7.4.0"})),
    );

    let result = try_offline_rung(&repo, &root, false, &merged, &divergent, &ours, &theirs)
        .await
        .expect("theirs' pin satisfies e/dependent's own require; rung 0 must accept it");
    let d_dep = result
        .non_dev
        .iter()
        .find(|p| p.name == "d/dep")
        .expect("d/dep must be in the result");
    assert_eq!(d_dep.pretty_version, "2.0.0", "theirs, since ours failed");
}

/// (c): `e/dependent` pins `d/dep` to exactly `2.5.0`; neither ours' 1.0.0
/// nor theirs' 3.0.0 satisfies that sibling's own require, so both
/// attempts fail and rung 0 must fall through (`None`) with no fetch.
#[tokio::test]
async fn offline_rung_falls_through_when_neither_pin_satisfies_a_siblings_require() {
    let root = json!({"require": {"d/dep": "^1.0 || ^2.0 || ^3.0", "e/dependent": "^1.0"}});
    let cache = tempfile::tempdir().unwrap();
    let repo = offline_repo(&root, cache.path()).await;

    let e_dependent = pinned(
        "e/dependent",
        "1.0.0",
        &json!({"php": ">=7.4.0", "d/dep": "2.5.0"}),
    );
    let mut merged = BTreeMap::new();
    merged.insert("e/dependent".to_string(), e_dependent);

    let mut divergent = BTreeSet::new();
    divergent.insert("d/dep".to_string());
    let mut ours = BTreeMap::new();
    ours.insert(
        "d/dep".to_string(),
        pinned("d/dep", "1.0.0", &json!({"php": ">=7.4.0"})),
    );
    let mut theirs = BTreeMap::new();
    theirs.insert(
        "d/dep".to_string(),
        pinned("d/dep", "3.0.0", &json!({"php": ">=7.4.0"})),
    );

    let result = try_offline_rung(&repo, &root, false, &merged, &divergent, &ours, &theirs).await;
    assert!(
        result.is_none(),
        "neither pin satisfies e/dependent's own require; rung 0 must fall through"
    );
}

/// (d): the merged root's own `require` (`^4.0`) excludes both ours'
/// 1.0.0 and theirs' 2.0.0 outright, with no sibling involved at all.
/// Rung 0 must fall through with no fetch, same as (c) but from the root
/// requirement rather than a locked package's own.
#[tokio::test]
async fn offline_rung_falls_through_when_the_root_requirement_excludes_both_pins() {
    let root = json!({"require": {"d/dep": "^4.0"}});
    let cache = tempfile::tempdir().unwrap();
    let repo = offline_repo(&root, cache.path()).await;

    let merged = BTreeMap::new();
    let mut divergent = BTreeSet::new();
    divergent.insert("d/dep".to_string());
    let mut ours = BTreeMap::new();
    ours.insert(
        "d/dep".to_string(),
        pinned("d/dep", "1.0.0", &json!({"php": ">=7.4.0"})),
    );
    let mut theirs = BTreeMap::new();
    theirs.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.0", &json!({"php": ">=7.4.0"})),
    );

    let result = try_offline_rung(&repo, &root, false, &merged, &divergent, &ours, &theirs).await;
    assert!(
        result.is_none(),
        "the root's own requirement excludes both pins; rung 0 must fall through"
    );
}

/// The CLI wiring end to end: `psr/log` diverges (ours 3.0.99, theirs
/// 3.0.50), root wants `^3.0`, and `monolog/monolog`'s own locked
/// `require` (`^2.0 || ^3.0`) accepts either — so `ours`, tried first,
/// already satisfies everything the two locks record, and `--offline-rung`
/// must accept it without building a real `HttpTransport` at all (the CLI
/// process has no network access in this test environment; a rung-1
/// escalation attempt here would hang or fail, not silently pass).
#[test]
fn offline_rung_resolves_the_cli_fixture_with_no_network_transport_built() {
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
        .arg("--offline-rung")
        .output()
        .expect("failed to run viv");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("resolved via rung 0 (offline): psr/log"),
        "stderr: {stderr}"
    );
    let got = fs::read_to_string(dir.join("ours.lock")).unwrap();
    assert!(!got.contains("<<<<<<<"));
    assert!(
        got.contains("\"version\": \"3.0.99\""),
        "ours' pin, tried first, already satisfies everything"
    );
}

/// #295's own seam, `lock_merge::viv_lock_payloads`: `d/dep` is the one
/// divergent name (ours 2.0.0, theirs 2.0.1, `d/dep.json`'s own available
/// versions), `g/untouched` is the pinned non-divergent set whose `require`
/// a `viv.lock` record never carries — sourced instead from a hand-built
/// `composer.lock` `Value`, exactly as the sibling file supplies it in
/// production. Since nothing pins a require against `d/dep`, rung 1 alone
/// must resolve it, and the write-back through `native_lock::write` /
/// `lock_writer::write` must produce a pair `native_lock::reconcile`
/// accepts.
#[tokio::test]
async fn viv_lock_rung_1_resolve_writes_a_reconcilable_viv_lock_and_composer_lock() {
    let root = json!({"require": {"d/dep": "^2.0", "g/untouched": "^1.0"}});
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let cache = tempfile::tempdir().unwrap();
    let repo = Repository::from_composer_json_with_ttl(
        Path::new("."),
        &root,
        cache.path(),
        &transport,
        Duration::ZERO,
    )
    .await
    .unwrap();

    let composer_lock = json!({
        "packages": [{"name": "g/untouched", "version": "1.0.0", "require": {"php": ">=7.4.0"}}],
        "packages-dev": [],
    });
    let mut merged_identities = BTreeMap::new();
    merged_identities.insert(
        "g/untouched".to_string(),
        Identity {
            version: "1.0.0".to_string(),
            source_ref: None,
            dev: false,
        },
    );
    let merged = vivace::lock_merge::viv_lock_payloads(&merged_identities, &composer_lock).unwrap();

    let g_untouched = pinned("g/untouched", "1.0.0", &json!({"php": ">=7.4.0"}));
    let mut ours = BTreeMap::new();
    ours.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.0", &json!({"php": ">=7.4.0"})),
    );
    ours.insert("g/untouched".to_string(), g_untouched.clone());
    let mut theirs = BTreeMap::new();
    theirs.insert(
        "d/dep".to_string(),
        pinned("d/dep", "2.0.1", &json!({"php": ">=7.4.0"})),
    );
    theirs.insert("g/untouched".to_string(), g_untouched);

    let mut divergent = BTreeSet::new();
    divergent.insert("d/dep".to_string());

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
        Scope::Closure,
        "d/dep alone, with no pinned require blocking it, resolves at rung 1"
    );
    assert!(
        resolved.moved.is_empty(),
        "rung 1 must not move anything outside the divergent name: {:?}",
        resolved.moved
    );
    let untouched = resolved
        .result
        .non_dev
        .iter()
        .find(|p| p.name == "g/untouched")
        .unwrap();
    assert_eq!(
        untouched.pretty_version, "1.0.0",
        "g/untouched's payload, sourced from composer.lock via viv_lock_payloads, must pin it"
    );

    let viv_text =
        vivace::native_lock::write(&resolved.result.non_dev, &resolved.result.dev, &root).unwrap();
    let options = vivace::lock_writer::LockOptions {
        minimum_stability: resolved.result.minimum_stability,
        stability_flags: &resolved.result.stability_flags,
        prefer_stable: resolved.result.prefer_stable,
        prefer_lowest: resolved.result.prefer_lowest,
        platform_reqs: &resolved.result.platform_reqs,
        platform_dev_reqs: &resolved.result.platform_dev_reqs,
        platform_overrides: &resolved.result.platform_overrides,
        aliases: &resolved.result.aliases,
    };
    let composer_json = serde_json::to_vec(&root).unwrap();
    let composer_text = vivace::lock_writer::write(
        &resolved.result.non_dev,
        Some(&resolved.result.dev),
        &options,
        &composer_json,
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let viv_lock_path = dir.path().join("viv.lock");
    let composer_lock_path = dir.path().join("composer.lock");
    fs::write(&viv_lock_path, viv_text).unwrap();
    fs::write(&composer_lock_path, composer_text).unwrap();

    let mut lock = vivace::lock::read_lock(&composer_lock_path).unwrap();
    vivace::native_lock::reconcile(&mut lock, &viv_lock_path)
        .expect("the re-solved viv.lock and composer.lock must carry matching identities");
}

/// A minimal `viv.lock` body of one or more records, built through
/// `native_lock::write` itself (the same writer `viv lock convert` uses)
/// rather than hand-typed TOML, so a malformed fixture can't hide a real
/// bug.
fn viv_lock_body(records: &[(&str, &str)]) -> String {
    let packages: Vec<vivace::solver::transaction::ResolvedPackage> = records
        .iter()
        .map(
            |&(name, version)| vivace::solver::transaction::ResolvedPackage {
                name: name.to_string(),
                pretty_version: version.to_string(),
                raw: json!({}),
            },
        )
        .collect();
    vivace::native_lock::write(&packages, &[], &json!({})).unwrap()
}

/// `--max-scope closure` (#296) threaded through the `viv.lock` path
/// (#295), over the CLI's own real `HttpTransport` path: a `"package"`-type
/// `repositories[]` entry (`tests/package_repository.rs`'s own #294
/// pattern) needs no network at all, so the fixture below reuses
/// `d/dep`/`e/dependent`'s exact shape from
/// [`rung_2_resolves_when_a_pinned_direct_dependent_blocks_the_closure`]:
/// `d/dep` diverges (ours 2.0.0, theirs 2.0.1), `e/dependent` is pinned at
/// 1.0.0 requiring `d/dep ^1.0` (from the sibling `composer.lock`), so rung
/// 1 alone cannot satisfy root's `d/dep ^2.0` — only rung 2 could, and
/// `--max-scope closure` forbids it. `ours.lock` must end up with a marker
/// block, not a half-written lock.
#[test]
fn max_scope_closure_caps_and_falls_back_to_markers_in_viv_lock() {
    let ctx = TestContext::new();
    let dir = ctx.project.path();
    fs::write(
        dir.join("composer.json"),
        serde_json::to_vec(&json!({
            "name": "vivace/fixture-viv-lock-cap",
            "repositories": [
                {"packagist.org": false},
                {"type": "package", "package": [
                    {"name": "d/dep", "version": "1.0.0",
                     "dist": {"type": "zip", "url": "https://example.invalid/d-dep-1.0.0.zip"}},
                    {"name": "d/dep", "version": "2.0.0",
                     "dist": {"type": "zip", "url": "https://example.invalid/d-dep-2.0.0.zip"}},
                    {"name": "d/dep", "version": "2.0.1",
                     "dist": {"type": "zip", "url": "https://example.invalid/d-dep-2.0.1.zip"}},
                    {"name": "e/dependent", "version": "1.0.0", "require": {"d/dep": "^1.0"},
                     "dist": {"type": "zip", "url": "https://example.invalid/e-dependent-1.0.0.zip"}},
                    {"name": "e/dependent", "version": "2.0.0", "require": {"d/dep": "^2.0"},
                     "dist": {"type": "zip", "url": "https://example.invalid/e-dependent-2.0.0.zip"}},
                ]},
            ],
            "require": {"d/dep": "^2.0", "e/dependent": "^1.0 || ^2.0"},
        }))
        .unwrap(),
    )
    .unwrap();
    fs::write(
        dir.join("composer.lock"),
        serde_json::to_vec(&json!({
            "packages": [{"name": "e/dependent", "version": "1.0.0", "require": {"d/dep": "^1.0"}}],
            "packages-dev": [],
        }))
        .unwrap(),
    )
    .unwrap();
    let e_dependent = ("e/dependent", "1.0.0");
    fs::write(
        dir.join("base.lock"),
        viv_lock_body(&[("d/dep", "1.0.0"), e_dependent]),
    )
    .unwrap();
    fs::write(
        dir.join("ours.lock"),
        viv_lock_body(&[("d/dep", "2.0.0"), e_dependent]),
    )
    .unwrap();
    fs::write(
        dir.join("theirs.lock"),
        viv_lock_body(&[("d/dep", "2.0.1"), e_dependent]),
    )
    .unwrap();

    let output = ctx
        .viv()
        .args(["lock", "merge"])
        .arg(dir.join("base.lock"))
        .arg(dir.join("ours.lock"))
        .arg(dir.join("theirs.lock"))
        .arg("-d")
        .arg(dir)
        .arg("--max-scope")
        .arg("closure")
        .output()
        .expect("failed to run viv");

    assert_eq!(
        output.status.code(),
        Some(1),
        "a capped re-solve must still exit 1 like any other unresolved divergence: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let got = fs::read_to_string(dir.join("ours.lock")).unwrap();
    assert!(
        got.contains("<<<<<<< ours"),
        "viv.lock must carry a marker block, not a half-written re-solve: {got}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--max-scope=closure")
            && stderr.contains("falling back to conflict markers"),
        "stderr must name the cap and the fallback: {stderr}"
    );
}

/// #295's other fallback: no sibling `composer.lock` at all means no
/// `require` to re-solve the pinned set with, so `viv lock merge` must go
/// straight to markers and say why, naming the file it looked for, rather
/// than trying a solve it cannot pin and reporting an obscure lookup error.
#[test]
fn missing_sibling_composer_lock_falls_back_to_markers_with_a_reason() {
    let ctx = TestContext::new();
    let dir = ctx.project.path();
    fs::write(
        dir.join("composer.json"),
        serde_json::to_vec(&json!({"require": {"d/dep": "^2.0"}})).unwrap(),
    )
    .unwrap();
    fs::write(dir.join("base.lock"), viv_lock_body(&[("d/dep", "1.0.0")])).unwrap();
    fs::write(dir.join("ours.lock"), viv_lock_body(&[("d/dep", "2.0.0")])).unwrap();
    fs::write(
        dir.join("theirs.lock"),
        viv_lock_body(&[("d/dep", "2.0.1")]),
    )
    .unwrap();

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

    assert_eq!(
        output.status.code(),
        Some(1),
        "no sibling composer.lock must still fall back to markers, not error out: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let got = fs::read_to_string(dir.join("ours.lock")).unwrap();
    assert!(
        got.contains("<<<<<<< ours"),
        "viv.lock must carry a marker block: {got}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let composer_lock_path = dir.join("composer.lock");
    assert!(
        stderr.contains(&composer_lock_path.display().to_string()),
        "stderr must name the composer.lock it looked for: {stderr}"
    );
}
