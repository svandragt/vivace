//! `install` refusing on unresolved git conflict markers (#274): `viv.lock`
//! and `composer.lock` both fail with a plain parse error when they still
//! carry `<<<<<<<`/`=======`/`>>>>>>>` blocks from an unmerged branch —
//! this checks `install` turns that into a message naming the file, the
//! first marker's line and the packages in conflict, pointing at
//! `viv lock merge`, instead of the raw JSON/TOML error
//! (`docs/research.md` chapter 1's Format section). Offline, reusing the
//! `path` fixture the way `tests/native_lock_install.rs` does.

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

fn copy_path_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(path_fixture().join(name), project.join(name)).unwrap();
    }
    copy_tree(&path_fixture().join("packages"), &project.join("packages"));
}

/// The line number `install`'s error must name: 1-based, the line
/// `<<<<<<<` itself sits on.
fn first_marker_line(content: &str) -> usize {
    content
        .lines()
        .position(|line| line.starts_with("<<<<<<<"))
        .unwrap()
        + 1
}

#[test]
fn install_refuses_a_composer_lock_with_conflict_markers() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    let lock = fs::read_to_string(project.join("composer.lock")).unwrap();
    let conflicted = lock.replacen(
        "        {\n            \"name\": \"acme/hello\",\n            \"version\": \"1.0.0\",\n",
        "        {\n<<<<<<< ours\n            \"name\": \"acme/hello\",\n            \"version\": \"1.0.0\",\n=======\n            \"name\": \"acme/hello\",\n            \"version\": \"2.0.0\",\n>>>>>>> theirs\n",
        1,
    );
    assert_ne!(conflicted, lock, "the package block must have been split");
    fs::write(project.join("composer.lock"), &conflicted).unwrap();
    let line = first_marker_line(&conflicted);

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains(format!(
            "composer.lock has unresolved merge conflict markers (first at line {line})"
        )))
        .stderr(predicates::str::contains("packages: acme/hello"))
        .stderr(predicates::str::contains(
            "run `viv lock merge <base> <ours> <theirs>` or resolve the markers by hand, then retry",
        ));

    assert!(
        !project.join("vendor").exists(),
        "a refused install must not create vendor/"
    );
}

#[test]
fn install_refuses_a_viv_lock_with_conflict_markers() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    ctx.viv().arg("lock").arg("convert").assert().success();
    let viv_lock = fs::read_to_string(project.join("viv.lock")).unwrap();
    let conflicted = viv_lock.replacen(
        "[[package]]\nname = \"acme/hello\"\nversion = \"1.0.0\"\n",
        "<<<<<<< ours\n[[package]]\nname = \"acme/hello\"\nversion = \"1.0.0\"\n=======\n[[package]]\nname = \"acme/hello\"\nversion = \"2.0.0\"\n>>>>>>> theirs\n",
        1,
    );
    assert_ne!(
        conflicted, viv_lock,
        "the acme/hello record must have been split"
    );
    fs::write(project.join("viv.lock"), &conflicted).unwrap();
    let line = first_marker_line(&conflicted);

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains(format!(
            "viv.lock has unresolved merge conflict markers (first at line {line})"
        )))
        .stderr(predicates::str::contains("packages: acme/hello"))
        .stderr(predicates::str::contains(
            "run `viv lock merge <base> <ours> <theirs>` or resolve the markers by hand, then retry",
        ));

    assert!(
        !project.join("vendor").exists(),
        "a refused install must not create vendor/"
    );
}

/// A parse error with no markers is a differently-broken lock (truncated,
/// hand-edited invalid JSON): #274 must not swallow that message.
#[test]
fn install_surfaces_the_plain_parse_error_when_there_are_no_markers() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    let lock = fs::read_to_string(project.join("composer.lock")).unwrap();
    fs::write(project.join("composer.lock"), format!("{lock}garbage")).unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains("parsing"))
        .stderr(predicates::str::contains("as JSON"));

    assert!(!project.join("vendor").exists());
}
