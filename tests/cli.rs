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
