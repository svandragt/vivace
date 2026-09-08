//! #149: a `type: metapackage` lock entry with neither `dist` nor `source`
//! (Composer's `MetapackageInstaller` downloads nothing for it) must still
//! install, entirely offline — path repository for the one real package,
//! `package` repository for the metapackage.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/metapackage")
}

fn copy_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture().join(name), project.join(name)).unwrap();
    }
    copy_tree(&fixture().join("packages"), &project.join("packages"));
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

/// Every file under `expected` must exist at the same relative path under
/// `vendor` with identical bytes (`tests/install_e2e.rs`'s helper).
fn assert_matches_expected(expected: &Path, vendor: &Path) {
    let mut mismatches = Vec::new();
    walk(expected, expected, &mut |relative| {
        let want = fs::read(expected.join(relative)).unwrap();
        let got = fs::read(vendor.join(relative)).unwrap_or_default();
        if want != got {
            mismatches.push(relative.to_path_buf());
        }
    });
    assert!(
        mismatches.is_empty(),
        "files differing from Composer's expected output: {mismatches:?}"
    );
}

fn walk(root: &Path, dir: &Path, on_file: &mut impl FnMut(&Path)) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            walk(root, &path, on_file);
        } else {
            on_file(path.strip_prefix(root).unwrap());
        }
    }
}

#[test]
fn metapackage_with_no_dist_and_no_source_installs() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_sources(project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 1 packages"));

    assert_matches_expected(&fixture().join("expected/dev"), &project.join("vendor"));
    assert!(
        !project.join("vendor/acme/conflicts").exists(),
        "a metapackage owns no directory"
    );

    // Re-run: nothing changed, so it should take the no-op path.
    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to install"));
}
