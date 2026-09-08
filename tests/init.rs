//! `viv init` (#143): a composer.json for a new project, no prompts.
//!
//! `default_file_matches_the_golden` fixes the two inputs the default name
//! depends on (`git config user.name` via `GIT_CONFIG_GLOBAL`, and the
//! project directory name via `--project-dir`) so the file it writes is
//! deterministic.
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

/// A `GIT_CONFIG_GLOBAL` file with a fixed `user.name`, so `viv init`'s
/// inferred name is deterministic regardless of the machine running the
/// test. `GIT_CONFIG_NOSYSTEM` keeps a CI runner's own `/etc/gitconfig`
/// (if any) out of the picture too.
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

#[test]
fn default_file_matches_the_golden() {
    let ctx = TestContext::new();
    fs::create_dir(ctx.project.path().join("demo")).unwrap();
    let mut cmd = ctx.viv();
    let _home = fixed_git_identity(&ctx, &mut cmd);
    cmd.args(["init", "-d", "demo"]).assert().success();

    let written = fs::read(ctx.project.path().join("demo/composer.json")).unwrap();
    let expected = fs::read(fixture("default.expected.json")).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&written),
        String::from_utf8_lossy(&expected),
    );
}

#[test]
fn refuses_to_overwrite_without_force() {
    let ctx = TestContext::new();
    fs::write(ctx.project.path().join("composer.json"), "{}").unwrap();

    ctx.viv()
        .arg("init")
        .assert()
        .failure()
        .stderr(predicates::str::contains("already exists"));

    assert_eq!(
        fs::read_to_string(ctx.project.path().join("composer.json")).unwrap(),
        "{}",
        "a refused init must not touch the existing file"
    );
}

#[test]
fn force_overwrites_an_existing_file() {
    let ctx = TestContext::new();
    fs::write(ctx.project.path().join("composer.json"), "{}").unwrap();

    ctx.viv()
        .args(["init", "--force", "--name", "acme/widgets"])
        .assert()
        .success();

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(ctx.project.path().join("composer.json")).unwrap())
            .unwrap();
    assert_eq!(written["name"], "acme/widgets");
}

/// `src/` already present: the default `autoload.psr-4` entry appears with
/// no `--autoload` flag, namespaced from the project half of the name.
#[test]
fn src_directory_is_autoloaded_by_default() {
    let ctx = TestContext::new();
    fs::create_dir(ctx.project.path().join("src")).unwrap();

    ctx.viv()
        .args(["init", "--name", "acme/my-app"])
        .assert()
        .success();

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(ctx.project.path().join("composer.json")).unwrap())
            .unwrap();
    assert_eq!(written["autoload"]["psr-4"]["MyApp\\"], "src/");
}

/// `--autoload` forces the entry even without a matching directory on disk,
/// and picks the directory named.
#[test]
fn autoload_flag_forces_the_entry() {
    let ctx = TestContext::new();

    ctx.viv()
        .args(["init", "--name", "acme/my-app", "--autoload", "lib"])
        .assert()
        .success();

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(ctx.project.path().join("composer.json")).unwrap())
            .unwrap();
    assert_eq!(written["autoload"]["psr-4"]["MyApp\\"], "lib/");
}

/// No `src/` and no `--autoload`: no `autoload` key at all.
#[test]
fn no_autoload_entry_without_src_or_the_flag() {
    let ctx = TestContext::new();

    ctx.viv()
        .args(["init", "--name", "acme/my-app"])
        .assert()
        .success();

    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(ctx.project.path().join("composer.json")).unwrap())
            .unwrap();
    assert!(written.get("autoload").is_none());
}

/// End-to-end: `viv init --require psr/log` produces a composer.json,
/// composer.lock and vendor/ Composer itself accepts (#143's "done when").
/// Gated on `VIVACE_TEST_NETWORK=1` so a bare `cargo nextest run` stays
/// offline; run explicitly with
/// `VIVACE_TEST_NETWORK=1 devbox run -- cargo nextest run -E 'binary(init)'`.
#[test]
fn require_produces_a_lock_and_vendor() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping require_produces_a_lock_and_vendor: set VIVACE_TEST_NETWORK=1 to \
             resolve/fetch over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    ctx.viv()
        .args(["init", "--name", "acme/my-app", "--require", "psr/log"])
        .assert()
        .success();

    let project = ctx.project.path();
    assert!(project.join("composer.lock").is_file());
    assert!(project.join("vendor/psr/log").is_dir());

    let composer = std::process::Command::new("composer")
        .arg("--version")
        .output();
    if composer.is_err() {
        eprintln!("skipping composer validate: composer not on PATH (run through devbox)");
        return;
    }
    let status = std::process::Command::new("composer")
        .args(["validate", "--no-check-publish"])
        .current_dir(project)
        .status()
        .unwrap();
    assert!(status.success(), "composer validate reported errors");
}
