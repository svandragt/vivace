//! #92: native adapters for `yiisoft/yii2-composer` and
//! `craftcms/plugin-installer`. Each is byte-diffed against the artifact the
//! real plugin generates with Composer 2.10.2, mirroring
//! `tests/install_e2e.rs`'s `plugin_generators` module (a separate file so
//! it doesn't collide with concurrent edits there).
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
        .join("tests/fixtures/plugins")
        .join(name)
}

fn copy_lock_sources(fixture_dir: &Path, project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture_dir.join(name), project.join(name)).unwrap();
    }
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

fn skip_without_network() -> bool {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping plugins_yii2_craft test: set VIVACE_TEST_NETWORK=1 to fetch real dists \
             over the network"
        );
        return true;
    }
    false
}

/// `yiisoft/yii2-composer` writes `vendor/yiisoft/extensions.php`, a
/// `var_export` dump of every `yii2-extension` package. `acme/yii2-widget`
/// (`require`) and `acme/yii2-alpha` (`require-dev`) share no `require` edge
/// with each other, so the install-order DFS treats both as roots and
/// Composer's own order between them comes down to name, not root section
/// (#130) — regenerated with real Composer via `devbox run -- composer -d
/// tests/fixtures/plugins/yii2 update`.
#[test]
fn yii2_extensions_matches_composer() {
    if skip_without_network() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_lock_sources(&fixture("yii2"), project);
    copy_tree(&fixture("yii2").join("packages"), &project.join("packages"));

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 3 packages"));

    let want = fs::read_to_string(fixture("yii2").join("expected/extensions.php")).unwrap();
    let got = fs::read_to_string(project.join("vendor/yiisoft/extensions.php")).unwrap();
    assert_eq!(got, want);
}

/// Same fixture, `--no-dev`: `acme/yii2-alpha` (`require-dev`) drops out,
/// leaving just `acme/yii2-widget`.
#[test]
fn yii2_extensions_matches_composer_no_dev() {
    if skip_without_network() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_lock_sources(&fixture("yii2"), project);
    copy_tree(&fixture("yii2").join("packages"), &project.join("packages"));

    ctx.viv()
        .args(["install", "--no-dev"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 2 packages"));

    let want = fs::read_to_string(fixture("yii2").join("expected/no-dev/extensions.php")).unwrap();
    let got = fs::read_to_string(project.join("vendor/yiisoft/extensions.php")).unwrap();
    assert_eq!(got, want);
}

/// `craftcms/plugin-installer` writes `vendor/craftcms/plugins.php`, a
/// `var_export` dump of every `craft-plugin` package.
#[test]
fn craft_plugins_matches_composer() {
    if skip_without_network() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_lock_sources(&fixture("craft"), project);
    copy_tree(
        &fixture("craft").join("packages"),
        &project.join("packages"),
    );

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 2 packages"));

    let want = fs::read_to_string(fixture("craft").join("expected/plugins.php")).unwrap();
    let got = fs::read_to_string(project.join("vendor/craftcms/plugins.php")).unwrap();
    assert_eq!(got, want);
}
