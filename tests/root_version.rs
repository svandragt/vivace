//! #125: the root package's `installed.php` block, guessed from its git
//! checkout (`VersionGuesser::guessGitVersion`) when `composer.json` has no
//! explicit `version`. Uses the offline `path` fixture (no network fetch)
//! inside a throwaway git repo; expected strings were captured once from a
//! real `composer install` run against the same repo (`AGENTS.md`'s
//! "byte differences against Composer output are bugs" — confirmed by hand
//! here since there's no golden fixture for a git checkout's root block).

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestContext;

fn fixture() -> PathBuf {
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

/// `git` in `dir`, with a fixed identity and signing disabled so the commit
/// (and the tag in [`tag_detached_head_reads_the_tag_name`]) works the same
/// on a CI runner with no GPG/SSH signing key as on a developer's machine
/// with `commit.gpgsign`/`tag.gpgsign` turned on globally.
fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "vivace")
        .env("GIT_AUTHOR_EMAIL", "vivace@example.com")
        .env("GIT_COMMITTER_NAME", "vivace")
        .env("GIT_COMMITTER_EMAIL", "vivace@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

/// The `path` fixture, committed on `main` at its one commit — every test
/// here just checks out a different ref from there.
fn init_repo(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture().join(name), project.join(name)).unwrap();
    }
    copy_tree(&fixture().join("packages"), &project.join("packages"));
    git(project, &["init", "-q", "-b", "main"]);
    git(project, &["config", "commit.gpgsign", "false"]);
    git(project, &["config", "tag.gpgsign", "false"]);
    git(project, &["add", "-A"]);
    git(project, &["commit", "-q", "-m", "initial"]);
}

fn head_commit(project: &Path) -> String {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(project)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

fn root_block(project: &Path) -> String {
    fs::read_to_string(project.join("vendor/composer/installed.php")).unwrap()
}

#[test]
fn numeric_branch_becomes_x_dev() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    init_repo(project);
    git(project, &["checkout", "-qb", "5.x"]);
    let commit = head_commit(project);

    ctx.viv().arg("install").assert().success();

    let installed = root_block(project);
    assert!(installed.contains("'pretty_version' => '5.x-dev',"));
    assert!(installed.contains("'version' => '5.9999999.9999999.9999999-dev',"));
    assert!(installed.contains(&format!("'reference' => '{commit}',")));
}

#[test]
fn named_branch_becomes_dev_prefixed() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    init_repo(project);
    let commit = head_commit(project);

    ctx.viv().arg("install").assert().success();

    let installed = root_block(project);
    assert!(installed.contains("'pretty_version' => 'dev-main',"));
    assert!(installed.contains("'version' => 'dev-main',"));
    assert!(installed.contains(&format!("'reference' => '{commit}',")));
}

#[test]
fn tag_detached_head_reads_the_tag_name() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    init_repo(project);
    git(project, &["tag", "-m", "1.2.0", "1.2.0"]);
    git(project, &["checkout", "-q", "1.2.0"]);
    let commit = head_commit(project);

    ctx.viv().arg("install").assert().success();

    let installed = root_block(project);
    assert!(installed.contains("'pretty_version' => '1.2.0',"));
    assert!(installed.contains("'version' => '1.2.0.0',"));
    assert!(installed.contains(&format!("'reference' => '{commit}',")));
}

/// An explicit `version` in `composer.json` always wins over the git guess
/// (`RootPackageLoader`/`VersionGuesser::guessVersion`'s doc comment).
#[test]
fn explicit_version_wins_over_the_git_guess() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    init_repo(project);
    git(project, &["checkout", "-qb", "5.x"]);

    let composer_json = fs::read_to_string(project.join("composer.json")).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&composer_json).unwrap();
    value["version"] = serde_json::Value::String("2.5.0".into());
    fs::write(
        project.join("composer.json"),
        serde_json::to_string_pretty(&value).unwrap(),
    )
    .unwrap();

    ctx.viv().arg("install").assert().success();

    let installed = root_block(project);
    assert!(installed.contains("'pretty_version' => '2.5.0',"));
    assert!(installed.contains("'version' => '2.5.0.0',"));
    assert!(installed.contains("'reference' => null,"));
}
