//! `install`/`update` reading `viv.lock` (#297, #344): once every record
//! carries its own `raw` entry, `viv.lock` alone is enough to install from —
//! `install` generates `composer.lock` from it first (`native_lock::export`),
//! regenerates a stale one, and never touches one newer than `viv.lock`
//! (`docs/research.md` chapter 1's Format section). Both present and in
//! step, `reconcile` still refuses on a name/identity mismatch between the
//! two rather than picking a side. Offline cases reuse the `path` fixture
//! (`tests/install_e2e.rs`'s own `copy_path_sources`, duplicated here rather
//! than shared, same as that file does with `tests/update.rs`); the round
//! trip needs real dists, so it's gated on `VIVACE_TEST_NETWORK=1` like the
//! rest of the network-touching suite.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;
use predicates::prelude::*;

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

/// The reverse direction of the mismatch above: a package `composer.lock`
/// names that `viv.lock` lacks (a stale `viv.lock` after Composer's own
/// `require`) must refuse too, not silently drop the package from vendor/.
/// `composer.lock` is kept the newer file throughout (a real `composer
/// require` touches only it): #344's "regenerate a stale `composer.lock`"
/// rule is about `viv.lock` being the newer, authoritative side, and must
/// not paper over the opposite case, where `composer.lock` moved on and
/// `viv.lock` is the one that's behind.
#[test]
fn install_refuses_a_composer_lock_package_missing_from_viv_lock() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    ctx.viv().arg("lock").arg("convert").assert().success();
    let viv_lock = fs::read_to_string(project.join("viv.lock")).unwrap();
    let start = viv_lock.find("[[package]]\nname = \"acme/hello\"").unwrap();
    let end = viv_lock[start + 1..]
        .find("[[package]]")
        .map_or(viv_lock.len(), |offset| start + 1 + offset);
    let mut edited = viv_lock.clone();
    edited.replace_range(start..end, "");
    assert_ne!(
        edited, viv_lock,
        "the acme/hello record must have been removed"
    );
    fs::write(project.join("viv.lock"), edited).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    filetime::set_file_mtime(
        project.join("viv.lock"),
        filetime::FileTime::from_system_time(old),
    )
    .unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "acme/hello: composer.lock has version 1.0.0, dev=false, absent from viv.lock",
        ));
    assert!(
        !project.join("vendor").exists(),
        "vendor/ must not be written"
    );
}

/// #344: once every `viv.lock` record carries `raw`, the format alone is
/// enough to install from — no `composer.lock` at all generates one first,
/// through `native_lock::export` (the chunk 1 code path, in-process), matching
/// the fixture's own committed file byte for byte, `install` says so once,
/// and the install itself proceeds from it exactly as if it had been there
/// all along.
#[test]
fn install_generates_composer_lock_from_viv_lock_alone() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);
    let canonical_lock = fs::read_to_string(project.join("composer.lock")).unwrap();

    ctx.viv().arg("lock").arg("convert").assert().success();
    fs::remove_file(project.join("composer.lock")).unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "composer.lock generated from viv.lock",
        ));

    assert_eq!(
        fs::read_to_string(project.join("composer.lock")).unwrap(),
        canonical_lock,
        "the generated composer.lock must match the fixture byte for byte"
    );
    assert!(
        project.join("vendor/acme/hello").exists(),
        "install must actually have run from the generated lock"
    );
}

/// A `composer.lock` older than `viv.lock` is regenerated too, not only an
/// absent one — a hand edit, or a checkout that only restored
/// `composer.lock` — and `install` says so, the same one line.
#[test]
fn install_regenerates_a_stale_composer_lock_when_viv_lock_is_newer() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);
    let canonical_lock = fs::read_to_string(project.join("composer.lock")).unwrap();

    ctx.viv().arg("lock").arg("convert").assert().success();

    // Same packages/versions (so the actual install still succeeds), but
    // not the canonical bytes `export` would write.
    let stale = canonical_lock.replace(
        "\"content-hash\": \"73da8f91c87f59f6735580124d29207c\"",
        "\"content-hash\": \"00000000000000000000000000000000\"",
    );
    assert_ne!(
        stale, canonical_lock,
        "the content-hash must have been replaced"
    );
    fs::write(project.join("composer.lock"), &stale).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    filetime::set_file_mtime(
        project.join("composer.lock"),
        filetime::FileTime::from_system_time(old),
    )
    .unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "composer.lock generated from viv.lock",
        ));

    assert_eq!(
        fs::read_to_string(project.join("composer.lock")).unwrap(),
        canonical_lock,
        "a stale composer.lock must be regenerated back to viv.lock's own export"
    );
}

/// `install` must never touch a `composer.lock` newer than `viv.lock` — even
/// one whose own bytes differ from what `export` would write.
#[test]
fn install_leaves_a_composer_lock_newer_than_viv_lock_untouched() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);
    let canonical_lock = fs::read_to_string(project.join("composer.lock")).unwrap();

    ctx.viv().arg("lock").arg("convert").assert().success();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    filetime::set_file_mtime(
        project.join("viv.lock"),
        filetime::FileTime::from_system_time(old),
    )
    .unwrap();

    let edited = canonical_lock.replace(
        "\"content-hash\": \"73da8f91c87f59f6735580124d29207c\"",
        "\"content-hash\": \"00000000000000000000000000000000\"",
    );
    assert_ne!(
        edited, canonical_lock,
        "the content-hash must have been replaced"
    );
    fs::write(project.join("composer.lock"), &edited).unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("generated from viv.lock").not());

    assert_eq!(
        fs::read_to_string(project.join("composer.lock")).unwrap(),
        edited,
        "a composer.lock newer than viv.lock must be left untouched"
    );
}

/// A pre-#344 `viv.lock`, with no `raw` on any record, still can't be
/// exported: `install` refuses and names a package, pointing at `viv lock
/// convert` to bring the format up to date — `native_lock::export`'s own
/// error, propagated rather than reworded.
#[test]
fn install_refuses_a_pre_344_viv_lock_with_no_raw() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    ctx.viv().arg("lock").arg("convert").assert().success();
    fs::remove_file(project.join("composer.lock")).unwrap();

    let viv_lock = fs::read_to_string(project.join("viv.lock")).unwrap();
    let stripped = viv_lock
        .lines()
        .filter(|line| !line.starts_with("raw = "))
        .fold(String::new(), |mut acc, line| {
            acc.push_str(line);
            acc.push('\n');
            acc
        });
    assert!(
        stripped.len() < viv_lock.len(),
        "at least one `raw` line must have been removed"
    );
    fs::write(project.join("viv.lock"), stripped).unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains("acme/hello"))
        .stderr(predicates::str::contains(
            "viv.lock has no stored provider entry to export from",
        ))
        .stderr(predicates::str::contains("viv lock convert"));

    assert!(!project.join("vendor").exists());
}

/// `.gitattributes` gains `composer.lock linguist-generated=true` only once
/// a project has adopted `viv.lock` — never for a plain `composer.lock`-only
/// project, and not `export-ignore`, which would strip the file from
/// `git archive` and break a downstream tool that needs it there.
#[test]
fn install_marks_composer_lock_generated_only_with_viv_lock() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    ctx.viv().arg("install").assert().success();
    assert!(
        !fs::read_to_string(project.join(".gitattributes"))
            .unwrap_or_default()
            .contains("linguist-generated"),
        "a project without viv.lock must not gain the generated attribute"
    );

    ctx.viv().arg("lock").arg("convert").assert().success();
    ctx.viv().arg("install").assert().success();
    assert!(
        fs::read_to_string(project.join(".gitattributes"))
            .unwrap()
            .contains("composer.lock linguist-generated=true"),
        "a project with viv.lock must gain the generated attribute"
    );
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
