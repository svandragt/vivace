//! `install` wiring `merge.viv.driver` into a git checkout, and `init`
//! writing the `.gitattributes` line that asks for it (#298):
//! `docs/research.md` chapter 1's weakest adoption step was the developer
//! configuring `merge.viv.driver` by hand in every clone, since git refuses
//! to read driver commands out of the repository itself. Offline, reusing
//! the `path` fixture the way `tests/install_markers.rs` does.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{TestContext, git_command};
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

fn copy_path_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(path_fixture().join(name), project.join(name)).unwrap();
    }
    copy_tree(&path_fixture().join("packages"), &project.join("packages"));
}

fn git_init(project: &Path) {
    git_command()
        .arg("init")
        .current_dir(project)
        .output()
        .expect("git init");
}

fn configured_driver(project: &Path) -> Option<String> {
    let output = git_command()
        .args(["config", "--local", "--get", "merge.viv.driver"])
        .current_dir(project)
        .output()
        .expect("git config --get");
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

const WIRED_MESSAGE: &str =
    "viv: set merge.viv.driver in this clone so git merges composer.lock through viv lock merge";

#[test]
fn install_sets_the_driver_when_gitattributes_names_it() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);
    git_init(project);
    fs::write(project.join(".gitattributes"), "composer.lock merge=viv\n").unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stderr(predicates::str::contains(WIRED_MESSAGE));

    assert_eq!(
        configured_driver(project).as_deref(),
        Some("viv lock merge %O %A %B")
    );
}

#[test]
fn install_leaves_the_driver_unset_without_gitattributes() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);
    git_init(project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stderr(predicates::str::contains(WIRED_MESSAGE).not());

    assert_eq!(configured_driver(project), None);
}

#[test]
fn install_leaves_a_custom_driver_untouched() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);
    git_init(project);
    fs::write(project.join(".gitattributes"), "composer.lock merge=viv\n").unwrap();
    git_command()
        .args(["config", "--local", "merge.viv.driver", "my-custom-driver"])
        .current_dir(project)
        .output()
        .expect("git config --set");

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stderr(predicates::str::contains(WIRED_MESSAGE).not());

    assert_eq!(
        configured_driver(project).as_deref(),
        Some("my-custom-driver")
    );
}

#[test]
fn init_writes_both_gitattributes_lines_once() {
    let ctx = TestContext::new();
    let project = ctx.project.path();

    ctx.viv().arg("init").assert().success();
    let first = fs::read_to_string(project.join(".gitattributes")).unwrap();
    assert!(first.contains("composer.lock merge=viv"));
    assert!(first.contains("viv.lock merge=viv"));

    ctx.viv().args(["init", "--force"]).assert().success();
    let second = fs::read_to_string(project.join(".gitattributes")).unwrap();
    assert_eq!(second, first, "a second init must not duplicate the lines");
}
