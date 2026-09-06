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
