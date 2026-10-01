//! #350: `viv install` prints one stderr message per plugin whose bundled
//! `vendor/` carries a library also in the site's own lock at a different,
//! unprefixed version. Offline, every dist pre-populated straight into the
//! store (`tests/install_e2e.rs`'s own `store_package`/`zip_of_one_file`
//! pattern, extended here to a multi-file zip for the plugin's own bundled
//! tree) — no network, no real WordPress.org/Packagist fixture needed.
#![allow(
    clippy::print_stderr,
    reason = "a php-not-on-PATH skip message is the point of this test, not app logging"
)]

mod common;

use std::fmt::Write as _;
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

// --- #351: `viv isolate` -----------------------------------------------

/// `acme/isolate-plugin`'s own bundled tree: one library (`foo/bar`) whose
/// single file carries a literal token in place of a real namespace
/// declaration — the fake `php-scoper` below rewrites that token, which is
/// enough to prove the whole pipeline (copy, scope, classmap-regenerate,
/// cache, link) without fighting backslash escaping through a shell
/// script's own `sed` call for a real `namespace Foo\Bar;` line.
const ORIGINAL_TOKEN: &str = "ORIGINAL_NAMESPACE_TOKEN";
const REWRITTEN_TOKEN: &str = "Viv_Isolated_IsolatePlugin";

fn isolate_plugin_zip() -> Vec<u8> {
    let installed_json = json!({
        "packages": [{"name": "foo/bar", "version": "1.0.0", "type": "library"}]
    })
    .to_string();
    zip_of_files(&[
        (
            "isolate-plugin.php",
            b"<?php\n// Plugin Name: Isolate Plugin\n",
        ),
        ("vendor/composer/installed.json", installed_json.as_bytes()),
        (
            "vendor/foo/bar/Baz.php",
            format!("<?php\nnamespace {ORIGINAL_TOKEN};\nclass Baz {{}}\n").as_bytes(),
        ),
    ])
}

fn write_isolate_project(project: &Path) {
    fs::write(
        project.join("composer.json"),
        json!({
            "name": "acme/site",
            "require": {"acme/isolate-plugin": "^1.0"},
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
                    "name": "acme/isolate-plugin",
                    "version": "1.0.0",
                    "type": "wordpress-plugin",
                    "dist": {
                        "type": "zip",
                        "url": "https://example.invalid/isolate-ref.zip",
                        "reference": "isolate-ref",
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

fn seed_isolate_plugin(cache_dir: &Path) {
    let store = Store::open(cache_dir).unwrap();
    store
        .add_zip(
            &dist_package(
                "acme/isolate-plugin",
                "1.0.0",
                "isolate-ref",
                "wordpress-plugin",
            ),
            &isolate_plugin_zip(),
        )
        .unwrap();
}

fn skip_without_php() -> bool {
    if std::process::Command::new("php")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping viv isolate test: php is not on PATH");
        return true;
    }
    false
}

/// `tool::ensure_tool_env`'s own cache key, reproduced here so this test
/// can prime its warm path directly: a sha256 of `*|php=<version>`, hex
/// encoded — `vivace::store::hex`/`tool::detect_php_version` are both
/// crate-private, so this is a small, deliberate duplicate, not a reused
/// helper.
fn tool_env_key() -> String {
    use sha2::{Digest, Sha256};
    let php_version = std::process::Command::new("php")
        .args(["-r", "echo PHP_VERSION;"])
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .unwrap_or_else(|| "unknown".to_string());
    let digest = Sha256::digest(format!("*|php={php_version}").as_bytes());
    digest.iter().fold(String::new(), |mut hex, byte| {
        let _ = write!(hex, "{byte:02x}");
        hex
    })
}

/// Primes `cache_dir`'s `tools-v0/humbug/php-scoper/<key>/` as if `viv x`
/// had already resolved and installed it: a `.viv-tool-complete` marker,
/// an `installed.json` entry (name, a fake version, one `bin`), and the
/// fake script itself at `vendor/bin/php-scoper` — so `apply_for`'s own
/// `tool::ensure_tool_env` call takes its warm path (a stat, nothing else)
/// and never reaches the network this test must not touch.
fn prime_fake_php_scoper(cache_dir: &Path, key: &str, called_marker: &Path) {
    let env_dir = cache_dir
        .join("tools-v0")
        .join("humbug")
        .join("php-scoper")
        .join(key);
    fs::create_dir_all(env_dir.join("vendor/bin")).unwrap();
    fs::create_dir_all(env_dir.join("vendor/composer")).unwrap();
    fs::write(
        env_dir.join("vendor/composer/installed.json"),
        json!({
            "packages": [{
                "name": "humbug/php-scoper",
                "version": "0.0.0-fake",
                "bin": ["bin/php-scoper"],
            }]
        })
        .to_string(),
    )
    .unwrap();
    let script = format!(
        "#!/bin/sh\nset -e\necho called >> {marker}\nout=\"\"\nwhile [ $# -gt 0 ]; do\n  case \
         \"$1\" in\n    --output-dir) out=\"$2\"; shift 2 ;;\n    *) shift ;;\n  esac\ndone\nmkdir \
         -p \"$out\"\ncp -a ./. \"$out/\"\nf=\"$out/vendor/foo/bar/Baz.php\"; sed 's/{from}/{to}/' \"$f\" > \"$f.tmp\" && mv \"$f.tmp\" \"$f\"\n",
        marker = called_marker.display(),
        from = ORIGINAL_TOKEN,
        to = REWRITTEN_TOKEN,
    );
    let bin = env_dir.join("vendor/bin/php-scoper");
    fs::write(&bin, script).unwrap();
    let mut perms = fs::metadata(&bin).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&bin, perms).unwrap();
    fs::write(env_dir.join(".viv-tool-complete"), b"").unwrap();
}

/// Primes `sniccowp/php-scoper-wordpress-excludes`'s own tool env the same
/// way, with empty exclude lists: `write_scoper_config` only needs the
/// resolve to be warm, not the lists to carry anything, for this test.
fn prime_fake_wordpress_excludes(cache_dir: &Path, key: &str) {
    let env_dir = cache_dir
        .join("tools-v0")
        .join("sniccowp")
        .join("php-scoper-wordpress-excludes")
        .join(key);
    let generated = env_dir.join("vendor/sniccowp/php-scoper-wordpress-excludes/generated");
    fs::create_dir_all(&generated).unwrap();
    for name in [
        "exclude-wordpress-classes.json",
        "exclude-wordpress-functions.json",
        "exclude-wordpress-constants.json",
        "exclude-wordpress-interfaces.json",
        "exclude-wordpress-traits.json",
    ] {
        fs::write(generated.join(name), b"[]").unwrap();
    }
    fs::write(env_dir.join(".viv-tool-complete"), b"").unwrap();
}

fn called_count(marker: &Path) -> usize {
    fs::read_to_string(marker).map_or(0, |content| content.lines().count())
}

#[test]
fn isolate_add_list_and_a_second_install_use_the_cache_with_no_scoper_run() {
    if skip_without_php() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_isolate_project(project);
    seed_isolate_plugin(ctx.cache.path());

    let key = tool_env_key();
    let called_marker = ctx.cache.path().join("fake-scoper-called");
    prime_fake_php_scoper(ctx.cache.path(), &key, &called_marker);
    prime_fake_wordpress_excludes(ctx.cache.path(), &key);

    // Materialise the plain archive first: `viv isolate` refuses to isolate
    // a plugin that isn't installed yet.
    ctx.viv().args(["install", "--offline"]).assert().success();

    let scoped_file = project.join("vendor/acme/isolate-plugin/vendor/foo/bar/Baz.php");
    assert!(
        fs::read_to_string(&scoped_file)
            .unwrap()
            .contains(ORIGINAL_TOKEN)
    );

    // Not `--offline`: the fake tool envs are warm (no network reached),
    // but this run still needs to get past `apply_for`'s own `if offline`
    // branch, which has nothing cached yet.
    ctx.viv()
        .args(["isolate", "acme/isolate-plugin"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "isolated acme/isolate-plugin under Viv\\Isolated\\IsolatePlugin",
        ));

    assert_eq!(
        called_count(&called_marker),
        1,
        "the fake scoper ran exactly once"
    );
    assert!(
        fs::read_to_string(&scoped_file)
            .unwrap()
            .contains(REWRITTEN_TOKEN),
        "the rewritten file reached the plugin's real install path"
    );
    let composer_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project.join("composer.json")).unwrap()).unwrap();
    assert_eq!(
        composer_json.pointer("/extra/viv/isolate").unwrap(),
        &json!(["acme/isolate-plugin"])
    );

    ctx.viv()
        .args(["isolate", "--list"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "acme/isolate-plugin  Viv\\Isolated\\IsolatePlugin",
        ));

    // A second, plain install links the cached `isolated-v0` tree over the
    // plain archive again with no scoper run at all.
    ctx.viv().args(["install"]).assert().success();
    assert_eq!(
        called_count(&called_marker),
        1,
        "the second install must not run the scoper again"
    );
    assert!(
        fs::read_to_string(&scoped_file)
            .unwrap()
            .contains(REWRITTEN_TOKEN)
    );

    // `--rm` restores the plain, unscoped tree and drops the composer.json
    // entry.
    ctx.viv()
        .args(["isolate", "--rm", "acme/isolate-plugin"])
        .assert()
        .success();
    let composer_json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(project.join("composer.json")).unwrap()).unwrap();
    assert!(composer_json.get("extra").is_none());
    assert!(
        fs::read_to_string(&scoped_file)
            .unwrap()
            .contains(ORIGINAL_TOKEN),
        "the plain archive is relinked"
    );
}

#[test]
fn isolate_offline_without_a_cached_build_errors_naming_the_package() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_isolate_project(project);
    seed_isolate_plugin(ctx.cache.path());
    ctx.viv().args(["install", "--offline"]).assert().success();

    ctx.viv()
        .args(["--offline", "isolate", "acme/isolate-plugin"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("acme/isolate-plugin"))
        .stderr(predicates::str::contains("no isolated build cached"));
}
