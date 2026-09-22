//! `viv lock merge` (#275 chunk 1): the golden case where one package
//! (`psr/log`) changed on both sides to a different version -- everything
//! else in `tests/fixtures/lock-merge/` merges clean, so the output must be
//! exactly one marker block around `psr/log`, byte-for-byte canonical
//! everywhere else.

use std::fs;
use std::path::{Path, PathBuf};

#[macro_use]
mod common;

use common::TestContext;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/lock-merge")
}

#[test]
fn a_divergent_package_gets_one_marker_block_and_exit_1() {
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
