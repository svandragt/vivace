//! End-to-end autoloader check against Composer 2.10's own output for the
//! monolog fixture, in dev and `--no-dev` mode. The fixture is copied to a
//! temp dir so the generator never writes over Composer's `vendor/`.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use vivace::autoload::generator::{Input, Package, RootPackage, generate};
use vivace::lock::{read_lock, read_root};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog")
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

/// The fixture's `vendor/` is gitignored and only populated locally by
/// `make fixtures` (needs devbox composer), so skip instead of panicking
/// when it's missing, e.g. on a fresh CI checkout.
#[allow(clippy::print_stderr, reason = "test skip notice, not app logging")]
fn fixture_vendor_missing() -> bool {
    if fixture()
        .join("vendor/composer/installed.json")
        .try_exists()
        .unwrap_or(false)
    {
        return false;
    }
    eprintln!(
        "skipping: run `make fixtures` (needs devbox composer) to populate tests/fixtures/monolog/vendor"
    );
    true
}

fn check(dev_mode: bool, expected_dir: &str) {
    if fixture_vendor_missing() {
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().canonicalize().unwrap();
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture().join(name), project.join(name)).unwrap();
    }
    for dir in ["src", "lib", "vendor/monolog", "vendor/psr"] {
        copy_tree(&fixture().join(dir), &project.join(dir));
    }
    let vendor_dir = project.join("vendor");

    let root = read_root(&project.join("composer.json")).unwrap();
    let lock = read_lock(&project.join("composer.lock")).unwrap();
    let keys = |map: &serde_json::Map<String, Value>| map.keys().cloned().collect::<Vec<_>>();

    let input = Input {
        root: RootPackage {
            name: root.name.clone().unwrap(),
            autoload: root.autoload.clone().unwrap_or(Value::Null),
            autoload_dev: root.autoload_dev.clone().unwrap_or(Value::Null),
            target_dir: None,
            requires: keys(&root.require),
        },
        packages: lock
            .packages(true)
            .map(|p| Package {
                name: p.name.clone(),
                autoload: p.autoload.clone().unwrap_or(Value::Null),
                requires: keys(&p.require),
                replaces: keys(&p.replace),
                provides: keys(&p.provide),
                target_dir: p.target_dir.clone(),
                install_path: (p.r#type != "metapackage").then(|| vendor_dir.join(&p.name)),
                is_dev: p.dev,
            })
            .collect(),
        dev_mode,
        scan_psr: false,
        suffix: root.config.autoloader_suffix.clone().unwrap(),
        vendor_dir: vendor_dir.clone(),
        base_dir: project.clone(),
        platform_check: true,
        prepend_autoloader: root.config.prepend_autoloader,
    };
    generate(&input).unwrap();

    let expected = fixture().join("expected").join(expected_dir);
    let mut mismatches = Vec::new();
    for name in [
        "autoload.php",
        "composer/autoload_namespaces.php",
        "composer/autoload_psr4.php",
        "composer/autoload_classmap.php",
        "composer/autoload_files.php",
        "composer/autoload_static.php",
        "composer/autoload_real.php",
        "composer/ClassLoader.php",
        "composer/InstalledVersions.php",
        "composer/LICENSE",
    ] {
        let want = fs::read(expected.join(name)).unwrap();
        let got = fs::read(vendor_dir.join(name)).unwrap_or_default();
        if want != got {
            mismatches.push(name);
        }
    }
    assert!(
        mismatches.is_empty(),
        "files differing from Composer: {mismatches:?}"
    );
}

#[test]
fn monolog_dev_matches_composer() {
    check(true, "dev");
}

#[test]
fn monolog_no_dev_matches_composer() {
    check(false, "no-dev");
}
