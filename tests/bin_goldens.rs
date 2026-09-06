//! Byte-for-byte check against seven real `vendor/bin` proxies Composer
//! 2.10.2 wrote for a project, in `tests/fixtures/bin/`. Builds a temp
//! `vendor/` tree with just enough of each package (the bin file, at the
//! `bin` path the real package uses) to exercise both proxy shapes: PHP
//! targets (with a `#!/usr/bin/env php` shebang, as all five real ones
//! carry) get the PHP proxy with its `BinProxyWrapper`; the two
//! `fig-r/psr2r-sniffer` bins are plain shell scripts, so they get the `sh`
//! proxy instead.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use vivace::bin::{BinCompat, generate};
use vivace::lock::Package;

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/bin")
            .join(name),
    )
    .unwrap()
}

fn package(name: &str, bin: &[&str]) -> Package {
    Package {
        name: name.to_string(),
        version: "1.0.0".to_string(),
        dist: None,
        autoload: None,
        require: Map::new(),
        provide: Map::new(),
        replace: Map::new(),
        r#type: "library".to_string(),
        target_dir: None,
        include_path: Vec::new(),
        bin: bin.iter().copied().map(str::to_string).collect(),
        dev: false,
        raw: Value::Null,
    }
}

const PHP_TARGET: &[u8] = b"#!/usr/bin/env php\n<?php\n\necho \"stub\\n\";\n";
const SH_TARGET: &[u8] = b"#!/usr/bin/env sh\necho stub\n";

/// The umask-derived proxy mode (`0777 & ~umask()`), read the same way
/// `bin::proxy_mode` does: a freshly created directory already reflects it,
/// no `libc` call needed.
fn expected_proxy_mode() -> u32 {
    let probe = tempfile::tempdir().unwrap();
    fs::metadata(probe.path()).unwrap().permissions().mode() & 0o777
}

#[test]
fn matches_composer_2_10_2_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let vendor_dir = tmp.path().join("vendor");
    let bin_dir = vendor_dir.join("bin");

    let write_target = |install_path: &str, bin_path: &str, content: &[u8]| {
        let path = vendor_dir.join(install_path).join(bin_path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, content).unwrap();
    };
    write_target("nesbot/carbon", "bin/carbon", PHP_TARGET);
    write_target(
        "staabm/annotate-pull-request-from-checkstyle",
        "cs2pr",
        PHP_TARGET,
    );
    write_target("squizlabs/php_codesniffer", "bin/phpcbf", PHP_TARGET);
    write_target("squizlabs/php_codesniffer", "bin/phpcs", PHP_TARGET);
    write_target("fig-r/psr2r-sniffer", "bin/sniff", SH_TARGET);
    write_target("fig-r/psr2r-sniffer", "bin/tokenize", SH_TARGET);
    write_target("ankitpokhrel/tus-php", "bin/tus", PHP_TARGET);

    let packages = [
        (
            package("nesbot/carbon", &["bin/carbon"]),
            vendor_dir.join("nesbot/carbon"),
        ),
        (
            package("staabm/annotate-pull-request-from-checkstyle", &["cs2pr"]),
            vendor_dir.join("staabm/annotate-pull-request-from-checkstyle"),
        ),
        (
            package("squizlabs/php_codesniffer", &["bin/phpcbf", "bin/phpcs"]),
            vendor_dir.join("squizlabs/php_codesniffer"),
        ),
        (
            package("fig-r/psr2r-sniffer", &["bin/sniff", "bin/tokenize"]),
            vendor_dir.join("fig-r/psr2r-sniffer"),
        ),
        (
            package("ankitpokhrel/tus-php", &["bin/tus"]),
            vendor_dir.join("ankitpokhrel/tus-php"),
        ),
    ];
    let refs: Vec<(&Package, PathBuf)> = packages.iter().map(|(p, d)| (p, d.clone())).collect();

    let warnings = generate(&vendor_dir, &bin_dir, BinCompat::Auto, &refs).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    let expected_mode = expected_proxy_mode();
    for name in [
        "carbon", "cs2pr", "phpcbf", "phpcs", "sniff", "tokenize", "tus",
    ] {
        let path = bin_dir.join(name);
        let actual = fs::read(&path).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(
            actual,
            fixture(name),
            "{name}: content differs from Composer's"
        );

        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode & 0o111,
            0o111,
            "{name}: not all exec bits set ({mode:o})"
        );
        assert_eq!(
            mode, expected_mode,
            "{name}: mode should be 0777 & ~umask()"
        );
    }
}

fn legacy_expected(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/legacy/expected/dev/bin")
            .join(name),
    )
    .unwrap()
}

/// `bin/phpunit` (from Composer 2.10.2, `phpunit/phpunit` 9.6.36) is the
/// only one of the eight legacy goldens that carries the `PHPUnit` process
/// isolation workaround (`generateUnixyProxyCode`'s `$binContents ===
/// $vendorDir.'/phpunit/phpunit/phpunit'` branch): an extra `$GLOBALS`
/// line, and inside `BinProxyWrapper` a `phpvfscomposer://`-prefixed
/// `$opened_path` plus two `__DIR__`/`__FILE__`-rewriting `str_replace`
/// calls in `stream_read`. The workaround keys on the target's path, not
/// its contents, so a stub target reproduces it byte for byte.
#[test]
fn phpunit_bin_gets_process_isolation_workaround() {
    let tmp = tempfile::tempdir().unwrap();
    let vendor_dir = tmp.path().join("vendor");
    let bin_dir = vendor_dir.join("bin");
    let install_path = vendor_dir.join("phpunit/phpunit");
    fs::create_dir_all(&install_path).unwrap();
    fs::write(
        install_path.join("phpunit"),
        b"#!/usr/bin/env php\n<?php\n\necho \"stub\\n\";\n".as_slice(),
    )
    .unwrap();

    let pkg = package("phpunit/phpunit", &["phpunit"]);
    let warnings = generate(
        &vendor_dir,
        &bin_dir,
        BinCompat::Auto,
        &[(&pkg, install_path)],
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    assert_eq!(
        fs::read(bin_dir.join("phpunit")).unwrap(),
        legacy_expected("phpunit"),
        "phpunit: content differs from Composer's"
    );
}

/// `bin/php-parse` (`nikic/php-parser` 5.6.1) needs the shebang-stripping
/// stream wrapper like `phpunit` does, but its path doesn't match
/// `vendor/phpunit/phpunit/phpunit`, so none of the process-isolation extras
/// apply.
#[test]
fn php_parse_bin_gets_no_process_isolation_workaround() {
    let tmp = tempfile::tempdir().unwrap();
    let vendor_dir = tmp.path().join("vendor");
    let bin_dir = vendor_dir.join("bin");
    let install_path = vendor_dir.join("nikic/php-parser");
    fs::create_dir_all(install_path.join("bin")).unwrap();
    fs::write(
        install_path.join("bin/php-parse"),
        b"#!/usr/bin/env php\n<?php\n\necho \"stub\\n\";\n".as_slice(),
    )
    .unwrap();

    let pkg = package("nikic/php-parser", &["bin/php-parse"]);
    let warnings = generate(
        &vendor_dir,
        &bin_dir,
        BinCompat::Auto,
        &[(&pkg, install_path)],
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");

    assert_eq!(
        fs::read(bin_dir.join("php-parse")).unwrap(),
        legacy_expected("php-parse"),
        "php-parse: content differs from Composer's"
    );
}

/// Composer's `BinaryInstaller` never creates `vendor/bin` when nothing in
/// the lock declares a `bin`; `diff -r` against a real install flags an
/// empty `vendor/bin` viv left behind otherwise.
#[test]
fn no_bin_dir_created_when_no_package_declares_bin() {
    let tmp = tempfile::tempdir().unwrap();
    let vendor_dir = tmp.path().join("vendor");
    let bin_dir = vendor_dir.join("bin");
    let install_path = vendor_dir.join("acme/tool");
    fs::create_dir_all(&install_path).unwrap();

    let pkg = package("acme/tool", &[]);
    let warnings = generate(
        &vendor_dir,
        &bin_dir,
        BinCompat::Auto,
        &[(&pkg, install_path)],
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!bin_dir.exists(), "vendor/bin should not have been created");
}

/// `BinaryInstaller::removeBinaries` deletes stale proxies but leaves the
/// directory itself; a reinstall with no `bin` packages left must clear out
/// a leftover proxy from a previous install without deleting `vendor/bin`.
#[test]
fn stale_proxy_removed_but_empty_bin_dir_kept() {
    let tmp = tempfile::tempdir().unwrap();
    let vendor_dir = tmp.path().join("vendor");
    let bin_dir = vendor_dir.join("bin");
    let install_path = vendor_dir.join("acme/tool");
    fs::create_dir_all(&install_path).unwrap();
    fs::create_dir_all(&bin_dir).unwrap();
    fs::write(
        bin_dir.join("stale"),
        b"#!/usr/bin/env php\n<?php\n// @generated by Composer\n",
    )
    .unwrap();

    let pkg = package("acme/tool", &[]);
    let warnings = generate(
        &vendor_dir,
        &bin_dir,
        BinCompat::Auto,
        &[(&pkg, install_path)],
    )
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(bin_dir.is_dir(), "vendor/bin should still exist");
    assert!(
        !bin_dir.join("stale").exists(),
        "stale proxy should have been removed"
    );
}
