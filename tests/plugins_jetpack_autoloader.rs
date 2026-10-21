//! Issue #389: `automattic/jetpack-autoloader`, byte-diffed against the files
//! real Composer 2.10.2 writes with the plugin enabled. The plugin's own
//! v6.0.2 source is a local `path` repository, so this needs no network.

mod common;

use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/jetpack-autoloader")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn project() -> TestContext {
    let ctx = TestContext::new();
    let fixture = fixture();
    for name in ["composer.json", "composer.lock"] {
        std::fs::copy(fixture.join(name), ctx.project.path().join(name)).unwrap();
    }
    for dir in ["packages", "src", "tests"] {
        copy_tree(&fixture.join(dir), &ctx.project.path().join(dir));
    }
    ctx
}

/// Every file under `expected` must exist under `vendor` with the same bytes.
fn assert_tree_matches(expected: &Path, vendor: &Path) {
    for entry in std::fs::read_dir(expected).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if entry.file_type().unwrap().is_dir() {
            assert_tree_matches(&entry.path(), &vendor.join(name));
        } else {
            let want = std::fs::read_to_string(entry.path()).unwrap();
            let got = std::fs::read_to_string(vendor.join(&name))
                .unwrap_or_else(|e| panic!("reading {}: {e}", vendor.join(&name).display()));
            assert_eq!(got, want, "{}", vendor.join(&name).display());
        }
    }
}

fn file_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn install_matches_composers_own_output() {
    let ctx = project();
    ctx.viv().arg("install").assert().success();

    let vendor = ctx.project.path().join("vendor");
    let expected = fixture().join("expected/default");
    assert_tree_matches(&expected, &vendor);
    assert_eq!(
        file_names(&vendor.join("jetpack-autoloader")),
        file_names(&expected.join("jetpack-autoloader")),
        "vendor/jetpack-autoloader/ holds exactly the files Composer wrote"
    );
}

/// `-o` scans PSR-4 directories into the class map, so Composer writes no
/// `jetpack_autoload_psr4.php`.
#[test]
fn optimized_install_matches_composers_own_output() {
    let ctx = project();
    ctx.viv().args(["install", "-o"]).assert().success();

    let vendor = ctx.project.path().join("vendor");
    assert_tree_matches(&fixture().join("expected/optimized"), &vendor);
    assert!(!vendor.join("composer/jetpack_autoload_psr4.php").exists());
}

/// `dump-autoload` fires the same `post-autoload-dump` event: it must reuse
/// the suffix of the first install and delete the manifest the optimised
/// dump no longer writes.
#[test]
fn dump_autoload_rewrites_the_manifests() {
    let ctx = project();
    ctx.viv().arg("install").assert().success();
    assert!(
        ctx.project
            .path()
            .join("vendor/composer/jetpack_autoload_psr4.php")
            .is_file()
    );

    ctx.viv().args(["dump-autoload", "-o"]).assert().success();

    let vendor = ctx.project.path().join("vendor");
    assert_tree_matches(&fixture().join("expected/optimized"), &vendor);
    assert_tree_matches(
        &fixture().join("expected/default/jetpack-autoloader"),
        &vendor.join("jetpack-autoloader"),
    );
    assert!(!vendor.join("composer/jetpack_autoload_psr4.php").exists());
}
