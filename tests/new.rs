//! `viv new`/`viv create-project` (#139): start a project in a directory
//! that doesn't exist yet.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/init")
        .join(name)
}

/// A `GIT_CONFIG_GLOBAL` file with a fixed `user.name`, so the inferred
/// `composer.json` name is deterministic. Duplicated from `tests/init.rs`'s
/// own private helper: each test binary is compiled separately, so there's
/// nowhere shared to hang this without widening `tests/common`.
fn fixed_git_identity(ctx: &TestContext, cmd: &mut assert_cmd::Command) -> tempfile::TempDir {
    let home = tempfile::tempdir().unwrap();
    fs::write(
        home.path().join(".gitconfig"),
        "[user]\n\tname = Test User\n",
    )
    .unwrap();
    cmd.current_dir(ctx.project.path())
        .env("GIT_CONFIG_GLOBAL", home.path().join(".gitconfig"))
        .env("GIT_CONFIG_NOSYSTEM", "1");
    home
}

/// `viv new demo` on a bare directory name runs `viv init`'s own defaults
/// inside it, so it writes the same golden `composer.json` `tests/init.rs`
/// checks for `viv init -d demo`.
#[test]
fn bare_directory_yields_the_init_golden() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    let _home = fixed_git_identity(&ctx, &mut cmd);
    cmd.args(["new", "demo"]).assert().success();

    let written = fs::read(ctx.project.path().join("demo/composer.json")).unwrap();
    let expected = fs::read(fixture("default.expected.json")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&written),
        String::from_utf8_lossy(&expected),
    );
}

#[test]
fn refuses_a_non_empty_directory() {
    let ctx = TestContext::new();
    let demo = ctx.project.path().join("demo");
    fs::create_dir(&demo).unwrap();
    fs::write(demo.join("keep.txt"), "already here").unwrap();

    ctx.viv()
        .args(["new", "demo"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"));

    assert_eq!(
        fs::read_to_string(demo.join("keep.txt")).unwrap(),
        "already here",
        "a refused new must not touch the existing directory"
    );
}

/// A bare directory name takes no second or third argument: those only mean
/// something for a `vendor/package` skeleton.
#[test]
fn bare_directory_rejects_extra_positionals() {
    let ctx = TestContext::new();
    ctx.viv()
        .args(["new", "demo", "extra"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("plain directory name"));
}

/// `vendor/package:constraint` and a third positional both naming a
/// constraint is ambiguous, not "last one wins".
#[test]
fn colon_and_positional_constraint_conflict() {
    let ctx = TestContext::new();
    ctx.viv()
        .args(["new", "acme/skeleton:^1.0", "demo", "^2.0"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("already names a constraint"));
}

/// `create-project` is `new`'s alias, including a bare directory name.
#[test]
fn create_project_alias_accepts_a_bare_directory() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    let _home = fixed_git_identity(&ctx, &mut cmd);
    cmd.args(["create-project", "demo"]).assert().success();

    assert!(ctx.project.path().join("demo/composer.json").is_file());
}

/// Composer's own `create-project vendor/package dir constraint` shape
/// (three positionals, no colon) must still parse under the `create-project`
/// alias: `--offline` turns the very first network call into a clean error
/// rather than a clap usage error, which is what a rejected third
/// positional would produce instead.
#[test]
fn create_project_accepts_the_positional_constraint_form() {
    let ctx = TestContext::new();
    let assert = ctx
        .viv()
        .args([
            "create-project",
            "acme/skeleton",
            "demo",
            "^1.0",
            "--offline",
        ])
        .assert()
        .failure();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).to_string();
    assert!(!stderr.contains("Usage:"), "a clap usage error: {stderr}");
    assert!(stderr.contains("Network disabled"), "{stderr}");
}

/// End-to-end: `viv new laravel/laravel demo --no-scripts` must match what
/// `composer create-project` writes for the same skeleton (#139's "done
/// when"). Gated on `VIVACE_TEST_NETWORK=1` so a bare `cargo nextest run`
/// stays offline; run explicitly with `VIVACE_TEST_NETWORK=1 devbox run --
/// cargo nextest run -E 'binary(new)'`.
#[test]
fn laravel_skeleton_matches_composer() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping laravel_skeleton_matches_composer: set VIVACE_TEST_NETWORK=1 to fetch \
             real dists over the network"
        );
        return;
    }
    if std::process::Command::new("composer")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!(
            "skipping laravel_skeleton_matches_composer: composer not on PATH (run through devbox)"
        );
        return;
    }

    let ctx = TestContext::new();
    let project = ctx.project.path();

    ctx.viv()
        .args(["new", "laravel/laravel", "demo", "--no-scripts"])
        .assert()
        .success();

    let composer_dir = project.join("demo-composer");
    let create = std::process::Command::new("composer")
        .args([
            "create-project",
            "laravel/laravel",
            "demo-composer",
            "--no-scripts",
            "--no-install",
            "--prefer-dist",
        ])
        .current_dir(project)
        .status()
        .expect("run composer create-project");
    assert!(create.success(), "composer create-project failed");

    let install = std::process::Command::new("composer")
        .args(["install", "--no-scripts"])
        .current_dir(&composer_dir)
        .status()
        .expect("run composer install");
    assert!(install.success(), "composer install failed");

    let viv_lock: serde_json::Value =
        serde_json::from_slice(&fs::read(project.join("demo/composer.lock")).unwrap()).unwrap();
    let composer_lock: serde_json::Value =
        serde_json::from_slice(&fs::read(composer_dir.join("composer.lock")).unwrap()).unwrap();
    assert_eq!(viv_lock, composer_lock, "composer.lock differs");

    let diff = std::process::Command::new("diff")
        .args(["-rq", "--exclude=.vivace-state"])
        .arg(project.join("demo/vendor"))
        .arg(composer_dir.join("vendor"))
        .output()
        .expect("diff vendor dirs");
    assert!(
        diff.status.success(),
        "vendor/ differs:\n{}",
        String::from_utf8_lossy(&diff.stdout)
    );
}
