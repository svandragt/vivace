//! Issue #355: `altis/cms-installer`, `altis/core` and
//! `altis/dev-tools-command`, byte-diffed against real Composer's own
//! output — mirroring `tests/plugins_private_c3.rs`'s own c3 test. A local
//! `path` repository for every package here, so this needs no network.

mod common;

use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/altis")
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

#[test]
fn altis_install_matches_composers_own_output() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    let fixture_dir = fixture();
    std::fs::copy(
        fixture_dir.join("composer.json"),
        project.join("composer.json"),
    )
    .unwrap();
    std::fs::copy(
        fixture_dir.join("composer.lock"),
        project.join("composer.lock"),
    )
    .unwrap();
    // The root `extra.altis.modules.custom.entrypoint` target
    // `generate_module_manifest` checks for with `file_exists` — relative to
    // the project root, not `vendor/`.
    std::fs::copy(
        fixture_dir.join("custom-entrypoint.php"),
        project.join("custom-entrypoint.php"),
    )
    .unwrap();
    copy_tree(&fixture_dir.join("packages"), &project.join("packages"));

    ctx.viv().arg("install").assert().success();

    let expected = fixture_dir.join("expected");
    for name in [
        "index.php",
        "wp-config.php",
        ".build-script",
        ".gitignore",
        ".travis.yml",
        ".config/travis.yml",
    ] {
        let want = std::fs::read_to_string(expected.join(name))
            .unwrap_or_else(|e| panic!("reading expected/{name}: {e}"));
        let got = std::fs::read_to_string(project.join(name))
            .unwrap_or_else(|e| panic!("reading {name}: {e}"));
        assert_eq!(got, want, "{name}");
    }
    let want_modules = std::fs::read_to_string(expected.join("modules.php")).unwrap();
    let got_modules = std::fs::read_to_string(project.join("vendor/modules.php")).unwrap();
    assert_eq!(got_modules, want_modules, "vendor/modules.php");

    // `altis/core`'s override: `acme/bundled-plugin` is named in
    // `acme/altis-module`'s `extra.altis.install-overrides`, so it installs
    // at the default vendor path instead of `content/plugins/` even though
    // it's `type: wordpress-plugin` like `acme/normal-plugin`, which isn't
    // overridden.
    assert!(project.join("vendor/acme/bundled-plugin").is_dir());
    assert!(!project.join("content/plugins/bundled-plugin").exists());
    assert!(project.join("content/plugins/normal-plugin").is_dir());
    assert!(project.join("content/themes").is_dir());
}
