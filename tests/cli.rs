//! CLI-surface snapshot tests: exit codes and stdout/stderr shape, not the
//! byte-exact `vendor/` contents (that is `install_e2e.rs`'s job).

#[macro_use]
mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog")
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

/// Copy just the source inputs a fresh `composer install` would see: no
/// `vendor/`, so the plan is always a clean install of every lock package.
fn copy_monolog_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture().join(name), project.join(name)).unwrap();
    }
    for dir in ["src", "lib"] {
        copy_tree(&fixture().join(dir), &project.join(dir));
    }
}

#[test]
fn update_is_not_implemented() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    cmd.arg("update");
    viv_snapshot!(ctx, cmd);
}

#[test]
fn install_without_a_lock_fails() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    cmd.arg("install");
    viv_snapshot!(ctx, cmd);
}

#[test]
fn install_dry_run_lists_the_plan() {
    let ctx = TestContext::new();
    copy_monolog_sources(ctx.project.path());
    let mut cmd = ctx.viv();
    cmd.args(["install", "--dry-run"]);
    viv_snapshot!(ctx, cmd);
}

#[test]
fn dump_autoload_without_a_lock_fails() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    cmd.arg("dump-autoload");
    viv_snapshot!(ctx, cmd);
}

/// The fixture's `vendor/` is gitignored and only populated locally by
/// `make fixtures` (needs devbox composer), so skip instead of panicking
/// when it's missing, e.g. on a fresh CI checkout.
#[allow(clippy::print_stderr, reason = "test skip notice, not app logging")]
fn fixture_vendor_missing() -> bool {
    if fixture()
        .join("vendor/composer/installed.json")
        .try_exists()
        .unwrap_or(false)
    {
        return false;
    }
    eprintln!(
        "skipping: run `make fixtures` (needs devbox composer) to populate tests/fixtures/monolog/vendor"
    );
    true
}

/// Byte-compare every file under `expected` against the same relative path
/// under `actual`, collecting mismatches instead of failing on the first.
fn compare_tree(actual: &Path, expected: &Path, rel: &Path, mismatches: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(expected.join(rel)).unwrap() {
        let entry = entry.unwrap();
        let rel = rel.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            compare_tree(actual, expected, &rel, mismatches);
        } else {
            let want = fs::read(expected.join(&rel)).unwrap();
            let got = fs::read(actual.join(&rel)).unwrap_or_default();
            if want != got {
                mismatches.push(rel);
            }
        }
    }
}

#[test]
fn dump_autoload_matches_composer() {
    if fixture_vendor_missing() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_monolog_sources(project);
    copy_tree(&fixture().join("vendor"), &project.join("vendor"));
    // Wipe what the fixture's own `composer install` produced, so a pass
    // proves `dump-autoload` regenerated it, not that it was already there.
    fs::remove_dir_all(project.join("vendor/composer")).unwrap();
    fs::remove_file(project.join("vendor/autoload.php")).unwrap();

    let mut cmd = ctx.viv();
    cmd.arg("dump-autoload");
    let output = cmd.output().expect("failed to run viv");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Generated autoload files\n"
    );
    let mut mismatches = Vec::new();
    compare_tree(
        &project.join("vendor"),
        &fixture().join("expected/dev"),
        Path::new(""),
        &mut mismatches,
    );
    assert!(
        mismatches.is_empty(),
        "files differing from composer (dev): {mismatches:?}"
    );

    let mut cmd = ctx.viv();
    cmd.args(["dump-autoload", "--no-dev"]);
    let output = cmd.output().expect("failed to run viv");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut mismatches = Vec::new();
    compare_tree(
        &project.join("vendor"),
        &fixture().join("expected/no-dev"),
        Path::new(""),
        &mut mismatches,
    );
    assert!(
        mismatches.is_empty(),
        "files differing from composer (no-dev): {mismatches:?}"
    );
}
