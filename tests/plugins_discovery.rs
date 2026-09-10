//! #157: `php-http/discovery`'s `preAutoloadDump` listener not only writes
//! `vendor/composer/GeneratedDiscoveryStrategy.php` (covered by
//! `src/plugins/discovery.rs`'s own unit test) but also points the root
//! package's autoload `classmap` at it, so `autoload_classmap.php` and
//! `autoload_static.php` carry the class too — byte-diffed here against real
//! Composer 2.8's own output, `php-http/discovery` 1.20.0 fetched from
//! Packagist.
//!
//! Mirrors `tests/plugins_drupal.rs`: a full `viv install`, gated on
//! `VIVACE_TEST_NETWORK=1` since the real plugin package comes from
//! Packagist.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/discovery")
}

#[test]
fn discovery_classmap_matches_composer() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping plugins_discovery test: set VIVACE_TEST_NETWORK=1 to fetch real dists \
             over the network"
        );
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    let fixture_dir = fixture();

    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture_dir.join(name), project.join(name)).unwrap();
    }

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 4 packages"));

    let expected_dir = fixture_dir.join("expected");
    for rel in [
        "composer/GeneratedDiscoveryStrategy.php",
        "composer/autoload_classmap.php",
        "composer/autoload_static.php",
    ] {
        let want = fs::read_to_string(expected_dir.join(Path::new(rel).file_name().unwrap()))
            .unwrap_or_else(|err| panic!("reading expected/{rel}: {err}"));
        let got = fs::read_to_string(project.join("vendor").join(rel))
            .unwrap_or_else(|err| panic!("reading project's vendor/{rel}: {err}"));
        assert_eq!(got, want, "vendor/{rel} differs from Composer's own output");
    }
}
