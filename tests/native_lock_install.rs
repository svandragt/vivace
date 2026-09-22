//! `install`/`update` reading `viv.lock` (#297): the format stays a
//! companion to `composer.lock`, never a replacement
//! (`docs/research.md` chapter 1's Format section) — `composer.lock` still
//! supplies every package's full entry, `viv.lock` only decides the locked
//! set and each record's own identity. Offline cases reuse the `path`
//! fixture (`tests/install_e2e.rs`'s own `copy_path_sources`, duplicated
//! here rather than shared, same as that file does with `tests/update.rs`);
//! the round trip needs real dists, so it's gated on `VIVACE_TEST_NETWORK=1`
//! like the rest of the network-touching suite.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn path_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/path")
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// #13's offline path fixture: one prod, one dev-only path package, no
/// network and no registry fetch, so it doubles as the fast fixture for
/// #297's reconciler.
fn copy_path_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(path_fixture().join(name), project.join(name)).unwrap();
    }
    copy_tree(&path_fixture().join("packages"), &project.join("packages"));
}

/// A record whose version disagrees with `composer.lock`'s own entry for
/// that package must refuse the whole install rather than pick a side.
#[test]
fn install_refuses_a_viv_lock_version_mismatch_and_writes_nothing() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    ctx.viv().arg("lock").arg("convert").assert().success();
    let viv_lock = fs::read_to_string(project.join("viv.lock")).unwrap();
    let edited = viv_lock.replacen(
        "name = \"acme/hello\"\nversion = \"1.0.0\"",
        "name = \"acme/hello\"\nversion = \"9.9.9\"",
        1,
    );
    assert_ne!(edited, viv_lock, "the version line must have been replaced");
    fs::write(project.join("viv.lock"), edited).unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains("acme/hello"))
        .stderr(predicates::str::contains(
            "viv.lock has version 9.9.9, dev=false",
        ))
        .stderr(predicates::str::contains(
            "composer.lock has version 1.0.0, dev=false",
        ))
        .stderr(predicates::str::contains(
            "run `viv update --lock native` to bring them back in step",
        ));

    assert!(
        !project.join("vendor").exists(),
        "a refused install must not create vendor/"
    );
}

/// `viv.lock` with no `composer.lock` beside it is not a standalone format
/// (`docs/research.md` chapter 1): `install` refuses and names the fix,
/// never falling back to a registry fetch.
#[test]
fn install_refuses_a_viv_lock_with_no_composer_lock() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    ctx.viv().arg("lock").arg("convert").assert().success();
    fs::remove_file(project.join("composer.lock")).unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains("companion to composer.lock"))
        .stderr(predicates::str::contains("viv update --lock native"));

    assert!(!project.join("vendor").exists());
}

/// Done-when (#297): `viv update --lock native`, delete `vendor/`, `viv
/// install` from the companion pair reproduces the same `vendor/` an
/// install from `composer.lock` alone would. Real dists, gated like
/// `tests/install_e2e.rs`.
#[test]
fn install_from_the_companion_pair_matches_composer_lock_alone() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_from_the_companion_pair_matches_composer_lock_alone: set \
             VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("bench/laravel");
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::copy(fixture.join("composer.json"), project.join("composer.json")).unwrap();
    fs::copy(fixture.join("composer.lock"), project.join("composer.lock")).unwrap();

    // Re-solves and chain-installs with `viv.lock` already on disk, so the
    // chained install already exercises the companion path this test means
    // to check.
    ctx.viv()
        .args(["update", "--lock", "native"])
        .assert()
        .success();
    assert!(project.join("viv.lock").is_file());

    let companion_vendor = ctx.project.path().join("vendor-companion");
    copy_tree(&project.join("vendor"), &companion_vendor);

    fs::remove_dir_all(project.join("vendor")).unwrap();
    fs::remove_file(project.join("viv.lock")).unwrap();

    ctx.viv().arg("install").assert().success();

    let diff = std::process::Command::new("diff")
        .args(["-rq", "--exclude=.vivace-state"])
        .arg(project.join("vendor"))
        .arg(&companion_vendor)
        .output()
        .unwrap();
    assert!(
        diff.status.success(),
        "vendor/ from composer.lock alone differs from the companion-pair install:\n{}",
        String::from_utf8_lossy(&diff.stdout)
    );
}
