//! End-to-end `viv install` against the monolog fixture: real network
//! fetches from GitHub, byte-diffed against Composer 2.10.2's own output.
//!
//! Gated on `VIVACE_TEST_NETWORK=1` so a bare `cargo nextest run` stays
//! offline; run explicitly with
//! `VIVACE_TEST_NETWORK=1 devbox run -- cargo nextest run -E 'binary(install_e2e)'`.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::fs;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog")
}

fn legacy_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy")
}

fn wordpress_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wordpress")
}

fn path_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/path")
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

fn copy_monolog_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture().join(name), project.join(name)).unwrap();
    }
    for dir in ["src", "lib"] {
        copy_tree(&fixture().join(dir), &project.join(dir));
    }
}

fn copy_legacy_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(legacy_fixture().join(name), project.join(name)).unwrap();
    }
    for dir in ["src-psr0", "src-psr4", "tests-legacy"] {
        copy_tree(&legacy_fixture().join(dir), &project.join(dir));
    }
}

fn copy_wordpress_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(wordpress_fixture().join(name), project.join(name)).unwrap();
    }
}

/// Every file under `expected` must exist at the same relative path under
/// `vendor` with identical bytes.
fn assert_matches_expected(expected: &Path, vendor: &Path) {
    let mut mismatches = Vec::new();
    walk(expected, expected, &mut |relative| {
        let want = fs::read(expected.join(relative)).unwrap();
        let got = fs::read(vendor.join(relative)).unwrap_or_default();
        if want != got {
            mismatches.push(relative.to_path_buf());
        }
    });
    assert!(
        mismatches.is_empty(),
        "files differing from Composer's expected output: {mismatches:?}"
    );
}

fn walk(root: &Path, dir: &Path, on_file: &mut impl FnMut(&Path)) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if entry.file_type().unwrap().is_dir() {
            walk(root, &path, on_file);
        } else {
            on_file(path.strip_prefix(root).unwrap());
        }
    }
}

#[test]
fn install_matches_composer_and_is_idempotent() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_e2e: set VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_monolog_sources(project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 3 packages"));

    assert_matches_expected(&fixture().join("expected/dev"), &project.join("vendor"));

    // Re-run: nothing changed, so it should take the no-op path.
    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to install"));

    ctx.viv().args(["install", "--no-dev"]).assert().success();
    assert_matches_expected(&fixture().join("expected/no-dev"), &project.join("vendor"));

    // Store layout: extracted trees plus the dist-reference pointers into them.
    assert!(ctx.cache.path().join("archive-v0").is_dir());
    assert!(ctx.cache.path().join("dists-v0").is_dir());

    // Hardlinked vendor files share the store's read-only mode.
    let mode = fs::metadata(project.join("vendor/monolog/monolog/composer.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o444, "hardlinked vendor files should be read-only");

    if Command::new("php").arg("--version").output().is_err() {
        eprintln!("skipping php autoload smoke test: php is not on PATH");
        return;
    }
    let output = Command::new("php")
        .arg("-r")
        .arg(
            r#"require "vendor/autoload.php";
new Monolog\Logger("x");
new Psr\Log\NullLogger;
new App\Greeter;
var_dump(Fixture\Legacy\Mode::On->value, fixture_helper());"#,
        )
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "php autoload smoke test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// A `vendor/` Composer wrote (or an older viv, pre-adopt) has
/// `installed.json` but no `.vivace-state`: a plain install must notice and
/// warn instead of silently claiming it, and `--adopt` must relink every
/// package from the store on request.
#[test]
fn adopts_a_composer_written_vendor_tree() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_e2e: set VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_monolog_sources(project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 3 packages"));

    let target = project.join("vendor/monolog/monolog/composer.json");
    let store_ino = fs::metadata(&target).unwrap().ino();

    // Simulate a tree Composer (or a pre-adopt viv) wrote: drop the state
    // marker and replace one hardlink with a plain writable copy, same as
    // `composer install` would leave behind.
    fs::remove_file(project.join("vendor/composer/.vivace-state")).unwrap();
    let content = fs::read(&target).unwrap();
    fs::remove_file(&target).unwrap();
    fs::write(&target, &content).unwrap();
    assert_ne!(fs::metadata(&target).unwrap().ino(), store_ino);

    let plain = ctx.viv().arg("install").output().unwrap();
    assert!(
        plain.status.success(),
        "{}",
        String::from_utf8_lossy(&plain.stderr)
    );
    assert!(
        String::from_utf8_lossy(&plain.stderr).contains(
            "vendor/ was not installed by viv; packages are plain copies. Run \
             `viv install --adopt` to relink them from the store."
        ),
        "stderr: {}",
        String::from_utf8_lossy(&plain.stderr)
    );
    assert_ne!(
        fs::metadata(&target).unwrap().ino(),
        store_ino,
        "a plain install must not touch files it only keeps"
    );

    let adopted = ctx.viv().args(["install", "--adopt"]).output().unwrap();
    assert!(
        adopted.status.success(),
        "{}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert!(String::from_utf8_lossy(&adopted.stdout).contains("Installed 3 packages"));
    assert!(
        !String::from_utf8_lossy(&adopted.stderr).contains("was not installed by viv"),
        "adopting should not repeat the notice: {}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    let relinked = fs::metadata(&target).unwrap();
    assert_eq!(
        relinked.ino(),
        store_ino,
        "adopt should relink from the store"
    );
    assert!(relinked.nlink() > 1, "adopt should hardlink, not copy");
}

/// Autoload shapes the monolog fixture doesn't reach: old-style PSR-0
/// (`pear/console_getopt`), PSR-0 with `target-dir` (`symfony/yaml` 2.6),
/// `files`-only packages (`swiftmailer/swiftmailer`), `files` ordered across
/// several dependencies (the `symfony/polyfill-*` family), a large real
/// classmap (`phpunit/phpunit`'s dependency tree), `exclude-from-classmap`
/// (the root package's own dev classmap), and `vendor/bin` proxies
/// (`phpunit`, `php-parse`).
#[test]
fn legacy_install_matches_composer_and_is_idempotent() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_e2e: set VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_legacy_sources(project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 37 packages"));

    assert_matches_expected(
        &legacy_fixture().join("expected/dev"),
        &project.join("vendor"),
    );

    // Re-run: nothing changed, so it should take the no-op path.
    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to install"));

    ctx.viv().args(["install", "--no-dev"]).assert().success();
    assert_matches_expected(
        &legacy_fixture().join("expected/no-dev"),
        &project.join("vendor"),
    );

    // Hardlinked vendor files share the store's read-only mode.
    let mode = fs::metadata(project.join("vendor/pear/console_getopt/composer.json"))
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o444, "hardlinked vendor files should be read-only");

    if Command::new("php").arg("--version").output().is_err() {
        eprintln!("skipping php autoload smoke test: php is not on PATH");
        return;
    }

    // --no-dev: phpunit and its vendor/bin proxies must be gone.
    assert!(!project.join("vendor/bin/phpunit").exists());
    let output = Command::new("php")
        .arg("-r")
        .arg(
            r#"require "vendor/autoload.php";
new Console_Getopt();
new HTMLPurifier();
Symfony\Component\Yaml\Yaml::parse("a: 1");
Ramsey\Uuid\Uuid::uuid4();
mb_strlen("hi");
if (class_exists("PHPUnit\Framework\TestCase")) {
    throw new Exception("phpunit should not autoload in --no-dev");
}
new Legacy\Thing();
new App\Legacy\Foo();"#,
        )
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "php --no-dev autoload smoke test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Back to dev: phpunit and its bin proxies return.
    ctx.viv().arg("install").assert().success();
    assert_matches_expected(
        &legacy_fixture().join("expected/dev"),
        &project.join("vendor"),
    );

    let output = Command::new("php")
        .arg("-r")
        .arg(
            r#"require "vendor/autoload.php";
new Console_Getopt();
new Swift_Message();
new HTMLPurifier();
Symfony\Component\Yaml\Yaml::parse("a: 1");
Ramsey\Uuid\Uuid::uuid4();
mb_strlen("hi");
if (!class_exists("PHPUnit\Framework\TestCase")) {
    throw new Exception("phpunit should autoload in dev mode");
}
new Legacy\Thing();
new App\Legacy\Foo();"#,
        )
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "php dev autoload smoke test failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// #13: `dist.type: path` packages, entirely offline (no network fetch, no
/// content-addressed store — see `src/source.rs`). Fixture has one prod and
/// one dev-only path package so `--no-dev` actually changes the plan.
fn copy_path_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(path_fixture().join(name), project.join(name)).unwrap();
    }
    copy_tree(&path_fixture().join("packages"), &project.join("packages"));
}

fn assert_symlinked_path_package(project: &Path, name: &str, target: &str) {
    let dest = project.join("vendor").join(name);
    let meta =
        fs::symlink_metadata(&dest).unwrap_or_else(|err| panic!("{}: {err}", dest.display()));
    assert!(meta.file_type().is_symlink(), "{name} should be a symlink");
    assert_eq!(fs::read_link(&dest).unwrap(), Path::new(target));
}

#[test]
fn path_repository_install_matches_composer_and_is_idempotent() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 2 packages"));

    assert_matches_expected(
        &path_fixture().join("expected/dev"),
        &project.join("vendor"),
    );
    assert_symlinked_path_package(project, "acme/hello", "../../packages/hello");
    assert_symlinked_path_package(project, "acme/testkit", "../../packages/testkit");
    assert_eq!(
        fs::read_to_string(project.join("vendor/acme/hello/src/Greeter.php")).unwrap(),
        fs::read_to_string(path_fixture().join("packages/hello/src/Greeter.php")).unwrap(),
    );

    // Re-run: nothing changed, so it should take the no-op path.
    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to install"));

    ctx.viv().args(["install", "--no-dev"]).assert().success();
    assert_matches_expected(
        &path_fixture().join("expected/no-dev"),
        &project.join("vendor"),
    );
    assert!(
        !project.join("vendor/acme/testkit").exists(),
        "the dev-only path package should be gone, symlink and all"
    );
    assert!(
        path_fixture().join("packages/testkit").is_dir(),
        "removing the symlink must not touch its target"
    );

    // Back to dev: acme/testkit is relinked.
    ctx.viv().arg("install").assert().success();
    assert_matches_expected(
        &path_fixture().join("expected/dev"),
        &project.join("vendor"),
    );
}

/// #13: a dist-less, `source.type: git` lock entry. Built against a throwaway
/// local repo (no `.git` fixture ever committed), isolated from any global
/// git hooks/config a developer machine may have (`core.hooksPath` rewrites
/// commit messages on this machine, for instance).
fn git_available() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

fn git_run(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "vivace test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "vivace test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?}");
}

fn git_head(dir: &Path) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A one-commit upstream repo with a real package layout; returns its HEAD.
fn init_upstream_repo(dir: &Path) -> String {
    git_run(dir, &["init", "-q", "-b", "main"]);
    fs::write(
        dir.join("composer.json"),
        r#"{
    "name": "acme/vcslib",
    "type": "library",
    "autoload": {"psr-4": {"Acme\\Vcs\\": "src/"}}
}"#,
    )
    .unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(
        dir.join("src/Thing.php"),
        "<?php\n\nnamespace Acme\\Vcs;\n\nclass Thing {}\n",
    )
    .unwrap();
    git_run(dir, &["add", "-A"]);
    git_run(dir, &["commit", "-q", "-m", "init"]);
    git_head(dir)
}

#[test]
fn git_source_package_checks_out_the_locked_reference() {
    if !git_available() {
        eprintln!("skipping git_source_package_checks_out_the_locked_reference: git not on PATH");
        return;
    }

    let upstream = tempfile::tempdir().unwrap();
    let reference = init_upstream_repo(upstream.path());

    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::write(
        project.join("composer.json"),
        r#"{"name": "vivace/fixture-vcs", "config": {"secure-http": true}}"#,
    )
    .unwrap();
    let lock = serde_json::json!({
        "packages": [{
            "name": "acme/vcslib",
            "version": "dev-main",
            "source": {
                "type": "git",
                "url": upstream.path().to_str().unwrap(),
                "reference": reference,
            },
            "type": "library",
            "autoload": {"psr-4": {"Acme\\Vcs\\": "src/"}},
        }],
        "packages-dev": [],
    });
    fs::write(
        project.join("composer.lock"),
        serde_json::to_string_pretty(&lock).unwrap(),
    )
    .unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 1 packages"));

    let checkout = project.join("vendor/acme/vcslib");
    assert!(checkout.join(".git").is_dir());
    assert!(checkout.join("src/Thing.php").is_file());
    assert_eq!(git_head(&checkout), reference);

    let installed: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.join("vendor/composer/installed.json")).unwrap(),
    )
    .unwrap();
    let package = &installed["packages"][0];
    assert_eq!(package["installation-source"], "source");
    assert_eq!(package["source"]["reference"], reference);

    // Re-run: nothing changed, so it should take the no-op path.
    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to install"));
    assert_eq!(git_head(&checkout), reference);
}

/// #51: `composer/installers` and `johnpbloch/wordpress-core-installer`
/// mapped natively — a wordpress-plugin, wordpress-muplugin and
/// wordpress-theme land under `wp-content/`, and `wordpress-core` under
/// `extra.wordpress-install-dir`, never under `vendor/`, byte-diffed against
/// real Composer 2.10.2 with both plugins enabled.
#[test]
fn wordpress_installer_paths_and_wordpress_core_matches_composer() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_e2e: set VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_wordpress_sources(project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 6 packages"));

    assert_matches_expected(
        &wordpress_fixture().join("expected/dev"),
        &project.join("vendor"),
    );

    // Mapped outside vendor/, at composer/installers' and the WordPress
    // core installer's real paths, not vendor/<vendor>/<name>.
    assert!(project.join("wp-content/plugins/soil").is_dir());
    assert!(
        project
            .join("wp-content/mu-plugins/bedrock-disallow-indexing")
            .is_dir()
    );
    assert!(project.join("wp-content/themes/wd_s").is_dir());
    assert!(project.join("wordpress/wp-settings.php").is_file());
    assert!(!project.join("vendor/roots").exists());
    assert!(!project.join("vendor/webdevstudios").exists());
    assert!(!project.join("vendor/johnpbloch/wordpress-core").exists());
    // Not path-mapped: a `composer-plugin` package itself, not a WordPress
    // type `composer/installers`/the core installer map.
    assert!(
        project
            .join("vendor/johnpbloch/wordpress-core-installer")
            .is_dir()
    );
    assert!(project.join("vendor/composer/installers").is_dir());

    // Re-run: nothing changed, so it should take the no-op path.
    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Nothing to install"));

    ctx.viv().args(["install", "--no-dev"]).assert().success();
    assert_matches_expected(
        &wordpress_fixture().join("expected/no-dev"),
        &project.join("vendor"),
    );
    // The dev-only mu-plugin, mapped outside vendor/, is still removed.
    assert!(
        !project
            .join("wp-content/mu-plugins/bedrock-disallow-indexing")
            .exists()
    );
}
