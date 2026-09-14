//! #214: `--ignore-platform-reqs`/`--ignore-platform-req=<name>` on `viv
//! install`, byte-diffed against Composer 2.10.2's own output for a lock
//! with both a `php` and a `php-64bit` requirement. Entirely offline (path
//! repository fixture, `tests/install_e2e.rs`'s `copy_path_sources` twin).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/platform-ignore")
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

fn assert_file_matches(expected_dir: &Path, vendor: &Path, name: &str) {
    let want = fs::read(expected_dir.join("composer").join(name))
        .unwrap_or_else(|err| panic!("reading expected/composer/{name}: {err}"));
    let got = fs::read(vendor.join("composer").join(name)).unwrap_or_default();
    assert_eq!(
        want, got,
        "vendor/composer/{name} differs from Composer's own output"
    );
}

/// No flag: both the PHP version and the 64-bit checks land in
/// `platform_check.php`, and `autoload_real.php` requires it.
#[test]
fn default_install_keeps_full_platform_check() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_sources(project);

    ctx.viv().arg("install").assert().success();

    let vendor = project.join("vendor");
    let expected = fixture().join("expected/default");
    assert_file_matches(&expected, &vendor, "platform_check.php");
    assert_file_matches(&expected, &vendor, "autoload_real.php");
}

/// `--ignore-platform-reqs`: no `platform_check.php` at all, and
/// `autoload_real.php` drops the `require` for it too (#214's second-order
/// effect — Composer's `autoload_real.php` differs there as well, not just
/// by the missing file).
#[test]
fn ignore_platform_reqs_drops_the_check_file_and_its_autoload_require() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_sources(project);

    ctx.viv()
        .args(["install", "--ignore-platform-reqs"])
        .assert()
        .success();

    let vendor = project.join("vendor");
    assert!(
        !vendor.join("composer/platform_check.php").exists(),
        "platform_check.php should not be written when every platform requirement is ignored"
    );
    assert_file_matches(
        &fixture().join("expected/ignore-all"),
        &vendor,
        "autoload_real.php",
    );
}

/// `--ignore-platform-req=php-64bit`: only the named requirement drops out
/// of `platform_check.php`; the `php` version check, and the `require` for
/// the (still written) file, both survive.
#[test]
fn ignore_platform_req_suppresses_only_the_named_requirement() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_sources(project);

    ctx.viv()
        .args(["install", "--ignore-platform-req=php-64bit"])
        .assert()
        .success();

    let vendor = project.join("vendor");
    let expected = fixture().join("expected/ignore-php64bit");
    assert_file_matches(&expected, &vendor, "platform_check.php");
    assert_file_matches(&expected, &vendor, "autoload_real.php");
}
