//! #350: `viv install` prints one stderr message per plugin whose bundled
//! `vendor/` carries a library also in the site's own lock at a different,
//! unprefixed version. Offline, every dist pre-populated straight into the
//! store (`tests/install_e2e.rs`'s own `store_package`/`zip_of_one_file`
//! pattern, extended here to a multi-file zip for the plugin's own bundled
//! tree) — no network, no real WordPress.org/Packagist fixture needed.

mod common;

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use common::TestContext;
use predicates::prelude::PredicateBooleanExt as _;
use serde_json::json;
use vivace::lock::Package;
use vivace::store::Store;

/// A lock package with a `zip` dist, for [`Store::add_zip`] — mirrors
/// `tests/install_e2e.rs`'s own `store_package`, parameterized over name,
/// version and dist reference since this fixture pre-populates three.
fn dist_package(name: &str, version: &str, reference: &str, r#type: &str) -> Package {
    serde_json::from_value(json!({
        "name": name,
        "version": version,
        "type": r#type,
        "dist": {
            "type": "zip",
            "url": format!("https://example.invalid/{reference}.zip"),
            "reference": reference,
            "shasum": "",
        },
    }))
    .unwrap()
}

fn zip_of_files(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, content) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// `composer.json`/`composer.lock` for one site: `composer/installers` (so
/// `acme/clashy-plugin`'s `wordpress-plugin` type maps to
/// `wp-content/plugins/clashy-plugin/`), the site's own `guzzlehttp/guzzle`
/// at 7.15.1, and the plugin bundling `guzzlehttp/guzzle` at 7.10.0 — the
/// exact pairing `docs/research.md`'s project D incident describes.
fn write_project(project: &Path) {
    fs::write(
        project.join("composer.json"),
        json!({
            "name": "acme/site",
            "require": {
                "composer/installers": "^2.3",
                "guzzlehttp/guzzle": "^7.15",
                "acme/clashy-plugin": "^1.0",
            },
            "config": {"allow-plugins": {"composer/installers": true}},
        })
        .to_string(),
    )
    .unwrap();
    fs::write(
        project.join("composer.lock"),
        json!({
            "content-hash": "test",
            "packages": [
                {
                    "name": "composer/installers",
                    "version": "v2.3.0",
                    "type": "composer-plugin",
                    "dist": {
                        "type": "zip",
                        "url": "https://example.invalid/installers.zip",
                        "reference": "installers-ref",
                        "shasum": "",
                    },
                },
                {
                    "name": "guzzlehttp/guzzle",
                    "version": "7.15.1",
                    "type": "library",
                    "dist": {
                        "type": "zip",
                        "url": "https://example.invalid/guzzle-ref.zip",
                        "reference": "guzzle-ref",
                        "shasum": "",
                    },
                },
                {
                    "name": "acme/clashy-plugin",
                    "version": "1.0.0",
                    "type": "wordpress-plugin",
                    "dist": {
                        "type": "zip",
                        "url": "https://example.invalid/clashy-ref.zip",
                        "reference": "clashy-ref",
                        "shasum": "",
                    },
                },
            ],
            "packages-dev": [],
            "aliases": [],
            "minimum-stability": "stable",
            "stability-flags": {},
            "prefer-stable": false,
            "prefer-lowest": false,
            "platform": {},
            "platform-dev": {},
            "plugin-api-version": "2.9.0",
        })
        .to_string(),
    )
    .unwrap();
}

/// `guzzlehttp/guzzle`'s own `installed.json` entry inside the plugin's
/// bundled tree: version 7.10.0, an `autoload.psr-4` root of `GuzzleHttp\`
/// (so [`Client.php`'s own namespace is read against the declared root, not
/// guessed from the file alone).
fn clashy_plugin_zip(client_namespace: &str) -> Vec<u8> {
    let installed_json = json!({
        "packages": [{
            "name": "guzzlehttp/guzzle",
            "version": "7.10.0",
            "autoload": {"psr-4": {"GuzzleHttp\\": "src/"}},
        }]
    })
    .to_string();
    let client_php = format!("<?php\nnamespace {client_namespace};\nclass Client {{}}\n");
    zip_of_files(&[
        (
            "clashy-plugin.php",
            b"<?php\n// Plugin Name: Clashy Plugin\n",
        ),
        ("vendor/composer/installed.json", installed_json.as_bytes()),
        (
            "vendor/guzzlehttp/guzzle/src/Client.php",
            client_php.as_bytes(),
        ),
    ])
}

fn seed_store(cache_dir: &Path, clashy_zip: &[u8]) {
    let store = Store::open(cache_dir).unwrap();
    store
        .add_zip(
            &dist_package(
                "composer/installers",
                "v2.3.0",
                "installers-ref",
                "composer-plugin",
            ),
            &zip_of_files(&[("src/Composer/Installers/Plugin.php", b"<?php\n")]),
        )
        .unwrap();
    store
        .add_zip(
            &dist_package("guzzlehttp/guzzle", "7.15.1", "guzzle-ref", "library"),
            &zip_of_files(&[(
                "src/Client.php",
                b"<?php\nnamespace GuzzleHttp;\nclass Client {}\n",
            )]),
        )
        .unwrap();
    store
        .add_zip(
            &dist_package(
                "acme/clashy-plugin",
                "1.0.0",
                "clashy-ref",
                "wordpress-plugin",
            ),
            clashy_zip,
        )
        .unwrap();
}

const EXPECTED_MESSAGE: &str = "acme/clashy-plugin bundles guzzlehttp/guzzle 7.10.0 unprefixed; \
                                 the site has 7.15.1. Run viv isolate acme/clashy-plugin to keep \
                                 both.";

#[test]
fn install_prints_the_clash_naming_plugin_versions_and_the_isolate_hint() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_project(project);
    seed_store(ctx.cache.path(), &clashy_plugin_zip("GuzzleHttp"));

    ctx.viv()
        .args(["install", "--offline"])
        .assert()
        .success()
        .stderr(predicates::str::contains(EXPECTED_MESSAGE));
}

#[test]
fn a_prefixed_namespace_is_not_reported() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_project(project);
    seed_store(
        ctx.cache.path(),
        &clashy_plugin_zip("WPForms\\Vendor\\GuzzleHttp"),
    );

    ctx.viv()
        .args(["install", "--offline"])
        .assert()
        .success()
        .stderr(predicates::str::contains("bundles").not());
}

#[test]
fn a_second_install_reprints_the_cached_message_without_reading_the_plugin_tree() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_project(project);
    seed_store(ctx.cache.path(), &clashy_plugin_zip("GuzzleHttp"));

    ctx.viv()
        .args(["install", "--offline"])
        .assert()
        .success()
        .stderr(predicates::str::contains(EXPECTED_MESSAGE));

    let isolate_cache = ctx.cache.path().join("isolate-check-v0");
    let entries: Vec<PathBuf> = fs::read_dir(&isolate_cache)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "one cached verdict after the first install"
    );

    // "Unreadable or removed": removing the plugin's own bundled `vendor/`
    // (not its top-level plugin directory, which `plan::plan`'s own on-disk
    // check still needs to see for this to read as a no-op at all) proves
    // the second run's message comes from the cache, not a fresh read.
    fs::remove_dir_all(project.join("wp-content/plugins/clashy-plugin/vendor")).unwrap();

    ctx.viv()
        .args(["install", "--offline"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Nothing to install, update or remove",
        ))
        .stderr(predicates::str::contains(EXPECTED_MESSAGE));

    let entries_after: Vec<PathBuf> = fs::read_dir(&isolate_cache)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(
        entries, entries_after,
        "the same cache entry, not a new one"
    );
}
