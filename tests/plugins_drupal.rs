//! #93: native adapters for `drupal/core-composer-scaffold` and
//! `symfony/runtime`, byte-diffed against real Composer 11.4.6/v7.4.14's own
//! output. `drupal/core-project-message` and `drupal/core-recipe-unpack`
//! (also enabled on `drupal/recommended-project`) are known-inert instead —
//! see `docs/plugin-strategy.md` — so no fixture covers them here.
//!
//! Mirrors `tests/plugins_yii2_craft.rs`: a full `viv install`, gated on
//! `VIVACE_TEST_NETWORK=1` since the two real plugin packages come from
//! Packagist, while the scaffold *source* packages (`acme/drupal-scaffold-base`,
//! `acme/drupal-scaffold-override`) are local `path` repositories exercising
//! allowed-packages ordering, overrides and every op (replace/append/prepend/
//! skip, `overwrite: false`).
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/drupal")
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

/// Every file under `expected/` (`web/...`, `.editorconfig`) has the same
/// path relative to the project root.
fn assert_matches_expected(expected_dir: &Path, project: &Path, rel: &str) {
    let want = fs::read_to_string(expected_dir.join(rel)).unwrap_or_else(|err| {
        panic!("reading expected/{rel}: {err}");
    });
    let got = fs::read_to_string(project.join(rel)).unwrap_or_else(|err| {
        panic!("reading project's {rel}: {err}");
    });
    assert_eq!(got, want, "{rel} differs from Composer's own output");
}

#[test]
fn drupal_scaffold_and_symfony_runtime_match_composer() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping plugins_drupal test: set VIVACE_TEST_NETWORK=1 to fetch real dists over \
             the network"
        );
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    let fixture_dir = fixture();

    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture_dir.join(name), project.join(name)).unwrap();
    }
    copy_tree(&fixture_dir.join("packages"), &project.join("packages"));
    // A pre-existing `.editorconfig` (`overwrite: false`) and a stale
    // `web/.htaccess` (overridden with `false`, i.e. skipped) — both must
    // survive the scaffold untouched.
    copy_tree(&fixture_dir.join("seed"), project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 4 packages"));

    let expected_dir = fixture_dir.join("expected");
    for rel in [
        ".editorconfig",
        "web/robots.txt",
        "web/index.php",
        "web/.htaccess",
        "web/router.php",
        "web/settings-extra.php",
        "web/sites/default/default.settings.php",
        "web/.gitignore",
        "web/sites/default/.gitignore",
        "web/autoload.php",
        "web/autoload_runtime.php",
    ] {
        assert_matches_expected(&expected_dir, project, rel);
    }

    let want = fs::read_to_string(expected_dir.join("vendor/drupal/DrupalInstalled.php")).unwrap();
    let got = fs::read_to_string(project.join("vendor/drupal/DrupalInstalled.php")).unwrap();
    assert_eq!(
        got, want,
        "vendor/drupal/DrupalInstalled.php differs from Composer's own output"
    );

    let want = fs::read_to_string(expected_dir.join("vendor/autoload_runtime.php")).unwrap();
    let got = fs::read_to_string(project.join("vendor/autoload_runtime.php")).unwrap();
    assert_eq!(
        got, want,
        "vendor/autoload_runtime.php differs from Composer's own output"
    );

    // `Drupal\DrupalInstalled` reaches the generated classmap, the seam
    // that makes `preAutoloadDump`'s classmap injection observable without
    // reaching into `plugins::drupal_scaffold` directly.
    let classmap =
        fs::read_to_string(project.join("vendor/composer/autoload_classmap.php")).unwrap();
    assert!(
        classmap.contains("'Drupal\\\\DrupalInstalled'"),
        "{classmap}"
    );
}
