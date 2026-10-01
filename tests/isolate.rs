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
/// script's own `sed` call for a real `namespace Foo\Bar;` line. #352's own
/// `class_exists` check (the main file, below) is what actually exercises
/// the regenerated autoloader, not just the file's own text.
const ORIGINAL_TOKEN: &str = "ORIGINAL_NAMESPACE_TOKEN";
const REWRITTEN_TOKEN: &str = "Viv_Isolated_IsolatePlugin";

/// #352: the main file's own load check — `class_exists` on the class
/// under its *rewritten* namespace, `or throw`, so a fake scoper that
/// leaves the string unprefixed (`FakeScoperMode::LeaveUnrewritten`, below)
/// fails the same way a real `exclude-classes` miss would.
fn isolate_plugin_main_php() -> Vec<u8> {
    format!(
        "<?php\n// Plugin Name: Isolate Plugin\nclass_exists('{REWRITTEN_TOKEN}\\\\Baz', true) or \
         throw new \\RuntimeException('{REWRITTEN_TOKEN}\\\\Baz missing');\n"
    )
    .into_bytes()
}

fn isolate_plugin_zip() -> Vec<u8> {
    // `autoload.psr-4` maps the package's own root (an empty relative path)
    // so the classmap scan `dump_scoped_autoload` runs after scoping has a
    // directory to look in at all — the scan itself reads each file's own
    // *current* namespace, not this declared prefix (`dump_scoped_autoload`'s
    // own doc comment), which is exactly what lets `class_exists` below find
    // `Baz` under its rewritten name without this entry ever changing.
    let installed_json = json!({
        "packages": [{
            "name": "foo/bar",
            "version": "1.0.0",
            "type": "library",
            "autoload": {"psr-4": {"Foo\\Bar\\": ""}},
        }]
    })
    .to_string();
    zip_of_files(&[
        ("isolate-plugin.php", &isolate_plugin_main_php()),
        ("vendor/composer/installed.json", installed_json.as_bytes()),
        (
            "vendor/foo/bar/Baz.php",
            format!("<?php\nnamespace {ORIGINAL_TOKEN};\nclass Baz {{}}\n").as_bytes(),
        ),
    ])
}

/// `acme/isolate-plugin`'s own locked entry plus `extra_packages` (#354: the
/// no-plugins test adds a refused `composer-plugin` entry here), split out
/// of `write_isolate_project` so that test and the memory-limit/PHP-pin one
/// below can each write their own `composer.json` over the same lock.
fn write_isolate_lock(project: &Path, extra_packages: &[serde_json::Value]) {
    let mut packages = vec![json!({
        "name": "acme/isolate-plugin",
        "version": "1.0.0",
        "type": "wordpress-plugin",
        "dist": {
            "type": "zip",
            "url": "https://example.invalid/isolate-ref.zip",
            "reference": "isolate-ref",
            "shasum": "",
        },
    })];
    packages.extend_from_slice(extra_packages);
    fs::write(
        project.join("composer.lock"),
        json!({
            "content-hash": "test",
            "packages": packages,
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

/// #354: every isolate test pins `PHP_PIN` here (rather than falling back to
/// `PATH`'s real `php`) because the fake php-scoper [`prime_fake_php_scoper`]
/// installs is a shell script, not a real PHP file — `build_scoped_tree`
/// now calls it as `<php> -d memory_limit=-1 <bin> ...`, and the real system
/// `php` would try to parse that shell script's own bytes as PHP source
/// (and, worse, can trip on a `<?php` sequence any fake's own rewrite body
/// happens to embed, as `corrupt_a_rewritten_file`'s does). [`seed_isolate_plugin`]
/// pairs this with [`fake_pinned_php`], which execs the fake scoper
/// correctly instead.
fn write_isolate_project(project: &Path) {
    fs::write(
        project.join("composer.json"),
        json!({
            "name": "acme/site",
            "require": {"acme/isolate-plugin": "^1.0"},
            "config": {"platform": {"php": PHP_PIN}},
        })
        .to_string(),
    )
    .unwrap();
    write_isolate_lock(project, &[]);
}

/// `acme/isolate-plugin`'s own dist, plus (#354) [`fake_pinned_php`] under
/// `PHP_PIN` with a default, usually-unchecked marker — every test that
/// needs its own distinguishable marker (the memory-limit one, below) calls
/// `fake_pinned_php` again afterwards with its own path, overwriting this
/// default at the same `php-v0/<PHP_PIN>-<os>-<arch>/php` location.
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
    fake_pinned_php(
        cache_dir,
        PHP_PIN,
        &cache_dir.join(".fake-pinned-php-marker"),
    );
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
/// had already resolved and installed it: a `.viv-tool-complete` marker, an
/// `installed.json` entry (name, `version`, one `bin`), and a fake script at
/// `vendor/bin/php-scoper` that always copies the input tree over
/// `--output-dir` (`cp -a`, same as the real tool's own "every file it
/// finds" behaviour) then runs `rewrite_body` (a shell fragment; empty
/// leaves the copy untouched) — so `apply_for`'s own `tool::ensure_tool_env`
/// call takes its warm path (a stat, nothing else) and never reaches the
/// network this test must not touch. Re-priming the same `key` with a
/// different `version` overwrites `installed.json` in place: `tool_env_key`
/// never depends on the scoper's own reported version, only the PHP one, so
/// this is exactly #352's "a later run with a different php-scoper version"
/// case, not a second tool env.
fn prime_fake_php_scoper(
    cache_dir: &Path,
    key: &str,
    called_marker: &Path,
    version: &str,
    rewrite_body: &str,
) {
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
                "version": version,
                "bin": ["bin/php-scoper"],
            }]
        })
        .to_string(),
    )
    .unwrap();
    let script = format!(
        "#!/bin/sh\nset -e\necho called >> {marker}\nout=\"\"\nwhile [ $# -gt 0 ]; do\n  case \
         \"$1\" in\n    --output-dir) out=\"$2\"; shift 2 ;;\n    *) shift ;;\n  esac\ndone\nmkdir \
         -p \"$out\"\ncp -a ./. \"$out/\"\n{rewrite_body}\n",
        marker = called_marker.display(),
    );
    let bin = env_dir.join("vendor/bin/php-scoper");
    fs::write(&bin, script).unwrap();
    let mut perms = fs::metadata(&bin).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&bin, perms).unwrap();
    fs::write(env_dir.join(".viv-tool-complete"), b"").unwrap();
}

const FAKE_SCOPER_VERSION: &str = "0.0.0-fake";

/// The fake scoper's own "good" rewrite: `foo/bar`'s single file's
/// namespace token, prefixed — exactly what a real php-scoper run would do
/// to it, just via `sed` instead of the real AST rewrite.
fn rewrite_the_namespace_token() -> String {
    format!(
        "f=\"$out/vendor/foo/bar/Baz.php\"; sed 's/{ORIGINAL_TOKEN}/{REWRITTEN_TOKEN}/' \"$f\" > \
         \"$f.tmp\" && mv \"$f.tmp\" \"$f\""
    )
}

/// #352: a fake scoper that copies the input tree straight through with no
/// rewrite at all — syntactically valid PHP, but `foo/bar\Baz`'s class
/// stays under its own original namespace, so the main file's own
/// `class_exists` check (looking for the *rewritten* name) never finds it.
fn leave_the_class_unprefixed() -> String {
    String::new()
}

/// #352: a fake scoper whose output replaces `foo/bar`'s own file with
/// invalid PHP — `php -l`'s own failure case, caught before the load check
/// ever runs.
fn corrupt_a_rewritten_file() -> String {
    "printf '<?php\\nclass {\\n' > \"$out/vendor/foo/bar/Baz.php\"".to_string()
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

/// `.vivace-state`'s own JSON, parsed — #352's `isolate_checked` map lives
/// here beside `isolate`'s own prefixes.
fn read_vivace_state(project: &Path) -> serde_json::Value {
    serde_json::from_str(
        &fs::read_to_string(project.join("vendor/composer/.vivace-state")).unwrap(),
    )
    .unwrap()
}

/// `cache_dir`'s own `isolated-v0` bucket, every entry — empty after a
/// failed `viv isolate` (#352's own check removes `dest` before returning),
/// non-empty after a real build.
fn isolated_bucket_entries(cache_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(cache_dir.join("isolated-v0"))
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default()
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
    prime_fake_php_scoper(
        ctx.cache.path(),
        &key,
        &called_marker,
        FAKE_SCOPER_VERSION,
        &rewrite_the_namespace_token(),
    );
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
    assert_eq!(
        read_vivace_state(project)["isolate_checked"]["acme/isolate-plugin"],
        json!(FAKE_SCOPER_VERSION),
        "the scoper version is recorded beside the prefix once the check passes"
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

    // #352: a different humbug/php-scoper version is a different store key
    // regardless of the plugin or its prefix, so this re-scopes (and
    // re-checks) even though nothing about the plugin itself changed.
    let bumped_version = "0.0.1-fake";
    prime_fake_php_scoper(
        ctx.cache.path(),
        &key,
        &called_marker,
        bumped_version,
        &rewrite_the_namespace_token(),
    );
    ctx.viv()
        .args(["isolate", "acme/isolate-plugin"])
        .assert()
        .success();
    assert_eq!(
        called_count(&called_marker),
        2,
        "a scoper version bump must run the scoper again"
    );
    assert_eq!(
        read_vivace_state(project)["isolate_checked"]["acme/isolate-plugin"],
        json!(bumped_version),
        "the recorded version reflects the re-check, not the stale one"
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

// --- #352: a scoped plugin still boots -------------------------------------

/// A syntax error in the scoped output fails `php -l` before the load check
/// ever runs: `viv isolate` exits 1 naming the file, the plain tree stays
/// linked, and nothing is cached under `isolated-v0`.
#[test]
fn a_php_syntax_error_in_the_scoped_output_fails_isolate_and_keeps_the_plain_tree() {
    if skip_without_php() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_isolate_project(project);
    seed_isolate_plugin(ctx.cache.path());

    let key = tool_env_key();
    let called_marker = ctx.cache.path().join("fake-scoper-called");
    prime_fake_php_scoper(
        ctx.cache.path(),
        &key,
        &called_marker,
        FAKE_SCOPER_VERSION,
        &corrupt_a_rewritten_file(),
    );
    prime_fake_wordpress_excludes(ctx.cache.path(), &key);
    ctx.viv().args(["install", "--offline"]).assert().success();
    let scoped_file = project.join("vendor/acme/isolate-plugin/vendor/foo/bar/Baz.php");

    ctx.viv()
        .args(["isolate", "acme/isolate-plugin"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("Baz.php"))
        .stderr(predicates::str::contains("php -l failed"));

    assert!(
        fs::read_to_string(&scoped_file)
            .unwrap()
            .contains(ORIGINAL_TOKEN),
        "the plain tree must stay linked, not the broken scoped one"
    );
    assert!(
        isolated_bucket_entries(ctx.cache.path()).is_empty(),
        "a failed check must leave nothing cached under isolated-v0"
    );
}

/// A fake scoper that leaves `foo/bar\Baz`'s class under its own original
/// namespace (syntactically valid, so `php -l` passes) fails the load
/// check instead: the main file's own `class_exists` on the rewritten name
/// throws, and `viv isolate` reports that message, the same as a real
/// `exclude-classes` miss would.
#[test]
fn a_fake_scoper_that_leaves_a_class_name_unprefixed_fails_the_load_check() {
    if skip_without_php() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_isolate_project(project);
    seed_isolate_plugin(ctx.cache.path());

    let key = tool_env_key();
    let called_marker = ctx.cache.path().join("fake-scoper-called");
    prime_fake_php_scoper(
        ctx.cache.path(),
        &key,
        &called_marker,
        FAKE_SCOPER_VERSION,
        &leave_the_class_unprefixed(),
    );
    prime_fake_wordpress_excludes(ctx.cache.path(), &key);
    ctx.viv().args(["install", "--offline"]).assert().success();
    let scoped_file = project.join("vendor/acme/isolate-plugin/vendor/foo/bar/Baz.php");

    ctx.viv()
        .args(["isolate", "acme/isolate-plugin"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("load check failed"))
        .stderr(predicates::str::contains(format!(
            "{REWRITTEN_TOKEN}\\Baz missing"
        )));

    assert!(
        fs::read_to_string(&scoped_file)
            .unwrap()
            .contains(ORIGINAL_TOKEN),
        "the plain tree must stay linked"
    );
    assert!(
        isolated_bucket_entries(ctx.cache.path()).is_empty(),
        "a failed check must leave nothing cached under isolated-v0"
    );
}

// --- #354: php-scoper runs under an explicit memory_limit override --------

const PHP_PIN: &str = "9.9.9";

/// A real `php` on `PATH` (already required by [`skip_without_php`]),
/// resolved to its absolute path: [`fake_pinned_php`]'s own script bakes
/// this in rather than calling bare `php` by name, since the `PATH`
/// `build_scoped_tree` composes for the scoper's own subprocess puts the
/// fake's own directory first — calling `php` by name from inside the fake
/// would just re-exec itself.
fn real_php_path() -> PathBuf {
    std::env::var_os("PATH")
        .and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("php"))
                .find(|p| p.is_file())
        })
        .expect("skip_without_php already checked php is on PATH")
}

/// `<cache>/php-v0/<version>-<os>-<arch>/php` (mirrors `tests/tool.rs`'s own
/// `fake_php_install`): intercepts only the one shape #354 makes
/// `build_scoped_tree` call the scoper with (`-d memory_limit=-1 <bin>
/// ...`), recording that it saw the override to `marker`, then `exec`s the
/// remaining args (the fake php-scoper script) directly. Every other
/// invocation on this same pinned interpreter — `tool::ensure_tool_env`'s
/// own `-r` version probe, `boot_check`'s `-l`/bootstrap run — is handed
/// straight to the real interpreter unchanged (its absolute path, not a
/// `PATH` lookup, so it can never resolve back to this fake), so neither of
/// those checks' behaviour is affected by priming this pin.
fn fake_pinned_php(cache_dir: &Path, version: &str, marker: &Path) {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let dir = cache_dir
        .join("php-v0")
        .join(format!("{version}-{os}-{arch}"));
    fs::create_dir_all(&dir).unwrap();
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = \"-d\" ] && [ \"$2\" = \"memory_limit=-1\" ]; then\n  echo ok \
         > \"{marker}\"\n  shift 2\n  exec \"$@\"\nfi\nexec \"{real_php}\" \"$@\"\n",
        marker = marker.display(),
        real_php = real_php_path().display(),
    );
    let php_path = dir.join("php");
    fs::write(&php_path, script).unwrap();
    let mut perms = fs::metadata(&php_path).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    fs::set_permissions(&php_path, perms).unwrap();
    fs::write(dir.join(".ok"), b"").unwrap();
}

/// #354: `build_scoped_tree` now calls the scoper as `<php> -d
/// memory_limit=-1 <bin> add-prefix ...` instead of execing the bin's own
/// shebang straight off `PATH` — the only way an `-d` ini override reaches
/// php-scoper at all. [`fake_pinned_php`] stands in for the project's own
/// pinned interpreter and records whether it saw the override.
#[test]
fn isolate_runs_the_scoper_with_an_explicit_memory_limit_override() {
    if skip_without_php() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_isolate_project(project);
    seed_isolate_plugin(ctx.cache.path());

    let memory_marker = ctx.cache.path().join("saw-memory-limit-override");
    fake_pinned_php(ctx.cache.path(), PHP_PIN, &memory_marker);

    let key = tool_env_key();
    let called_marker = ctx.cache.path().join("fake-scoper-called");
    prime_fake_php_scoper(
        ctx.cache.path(),
        &key,
        &called_marker,
        FAKE_SCOPER_VERSION,
        &rewrite_the_namespace_token(),
    );
    prime_fake_wordpress_excludes(ctx.cache.path(), &key);

    ctx.viv().args(["install", "--offline"]).assert().success();

    ctx.viv()
        .args(["isolate", "acme/isolate-plugin"])
        .assert()
        .success();

    assert!(
        memory_marker.is_file(),
        "the scoper must run under an explicit -d memory_limit=-1, not the bin's own shebang"
    );
}

// --- #354: `viv isolate --no-plugins` reaches the scoper -------------------

/// `acme/isolate-plugin` (the isolate target) plus `acme/mystery-plugin`, an
/// enabled `composer-plugin` neither a native adapter nor known-inert
/// (`tests/cli.rs`'s own `write_unknown_plugin_lock`, same shape, extended
/// with the isolate target): the site-level refusal `plugins::resolve`
/// raises blocks every `viv isolate` call regardless of which plugin it
/// names, since `load_project` resolves the whole lock before anything else
/// here runs.
fn write_isolate_project_with_unknown_plugin(project: &Path) {
    fs::write(
        project.join("composer.json"),
        json!({
            "name": "acme/site",
            "require": {"acme/isolate-plugin": "^1.0"},
            "config": {
                "allow-plugins": {"acme/mystery-plugin": true},
                "platform": {"php": PHP_PIN},
            },
        })
        .to_string(),
    )
    .unwrap();
    write_isolate_lock(
        project,
        &[json!({
            "name": "acme/mystery-plugin",
            "version": "1.0.0",
            "type": "composer-plugin",
            "dist": {
                "type": "zip",
                "url": "https://example.invalid/mystery-ref.zip",
                "reference": "mystery-ref",
                "shasum": "",
            },
        })],
    );
}

fn seed_mystery_plugin(cache_dir: &Path) {
    let store = Store::open(cache_dir).unwrap();
    store
        .add_zip(
            &dist_package(
                "acme/mystery-plugin",
                "1.0.0",
                "mystery-ref",
                "composer-plugin",
            ),
            &zip_of_files(&[("src/Plugin.php", b"<?php\n")]),
        )
        .unwrap();
}

/// #354's own done-when: `viv isolate` on a lock naming a plugin viv refuses
/// never reaches the scoper at all without `--no-plugins`; with it, the
/// refusal downgrades to a warning (same as `install`) and isolation
/// proceeds normally.
#[test]
fn isolate_no_plugins_reaches_the_scoper_past_a_refused_plugin() {
    if skip_without_php() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    write_isolate_project_with_unknown_plugin(project);
    seed_isolate_plugin(ctx.cache.path());
    seed_mystery_plugin(ctx.cache.path());

    let key = tool_env_key();
    let called_marker = ctx.cache.path().join("fake-scoper-called");
    prime_fake_php_scoper(
        ctx.cache.path(),
        &key,
        &called_marker,
        FAKE_SCOPER_VERSION,
        &rewrite_the_namespace_token(),
    );
    prime_fake_wordpress_excludes(ctx.cache.path(), &key);

    // `install` needs `--no-plugins` too: the same refusal blocks it before
    // `acme/isolate-plugin` is even materialised on disk.
    ctx.viv()
        .args(["install", "--offline", "--no-plugins"])
        .assert()
        .success();

    ctx.viv()
        .args(["isolate", "acme/isolate-plugin"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("cannot run the Composer plugin"))
        .stderr(predicates::str::contains("acme/mystery-plugin"));
    assert_eq!(
        called_count(&called_marker),
        0,
        "the scoper never ran without --no-plugins"
    );

    ctx.viv()
        .args(["isolate", "--no-plugins", "acme/isolate-plugin"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "isolated acme/isolate-plugin under Viv\\Isolated\\IsolatePlugin",
        ));
    assert_eq!(called_count(&called_marker), 1);
}
