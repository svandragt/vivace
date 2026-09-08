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
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestContext;
use vivace::lock::Package;
use vivace::store::Store;

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

/// `assert_cmd`'s default stderr is a pipe, never a terminal, so the
/// fetch/link progress line (`\r`-rewritten, TTY-only) must stay off: CI logs
/// and any other piped stderr rely on never seeing a `\r` or a bare
/// "Downloading" there.
#[test]
fn install_progress_line_is_silent_off_a_terminal() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_e2e: set VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    copy_monolog_sources(ctx.project.path());

    let assert = ctx.viv().arg("install").assert().success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr);
    assert!(!stderr.contains('\r'), "stderr: {stderr}");
    assert!(!stderr.contains("Downloading"), "stderr: {stderr}");
}

/// A `vendor/` Composer wrote (or an older viv, pre-adopt) has
/// `installed.json` but no `.vivace-state`: a plain install must adopt it
/// automatically, relinking every kept package from the store, with no
/// `--adopt` flag needed (#123).
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

    let adopted = ctx.viv().arg("install").output().unwrap();
    assert!(
        adopted.status.success(),
        "{}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert!(String::from_utf8_lossy(&adopted.stdout).contains("Installed 3 packages"));
    assert!(
        String::from_utf8_lossy(&adopted.stdout)
            .contains("adopted 3 packages from a Composer install"),
        "stdout: {}",
        String::from_utf8_lossy(&adopted.stdout)
    );
    assert!(
        !String::from_utf8_lossy(&adopted.stderr).contains("was not installed by viv"),
        "adopting should not print the old notice: {}",
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

/// Builds a Composer-written vendor tree (state marker gone, every
/// package's hardlink broken into a plain copy, same as
/// `adopts_a_composer_written_vendor_tree`) whose `psr/log` dist can never
/// be fetched again: its URL is broken (a stand-in for a private dist behind
/// an auth key this process has no credentials for) and its archive evicted
/// from the store, while `monolog/monolog`'s and `psr/container`'s stay
/// cached so relinking them needs no further network access. `psr/log`'s
/// `reference` is left untouched, so `plan::plan` still calls it a kept
/// package, not a version bump.
fn composer_written_tree_with_unfetchable_dist(ctx: &TestContext) {
    let project = ctx.project.path();
    copy_monolog_sources(project);

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 3 packages"));

    fs::remove_file(project.join("vendor/composer/.vivace-state")).unwrap();
    for target in [
        "vendor/monolog/monolog/composer.json",
        "vendor/psr/log/composer.json",
        "vendor/psr/container/composer.json",
    ] {
        let path = project.join(target);
        let content = fs::read(&path).unwrap();
        fs::remove_file(&path).unwrap();
        fs::write(&path, &content).unwrap();
    }

    let lock_path = project.join("composer.lock");
    let mut lock: serde_json::Value =
        serde_json::from_slice(&fs::read(&lock_path).unwrap()).unwrap();
    for package in lock["packages"].as_array_mut().unwrap() {
        if package["name"] == "psr/log" {
            package["dist"]["url"] = serde_json::Value::String(
                "https://api.github.com/repos/php-fig/log/zipball/\
                 0000000000000000000000000000000000000000"
                    .to_string(),
            );
        }
    }
    fs::write(&lock_path, serde_json::to_vec(&lock).unwrap()).unwrap();

    // `Store::pointer` keys on `dist.reference` (unchanged above), not the
    // URL, so this is the one dist pointer the broken URL above can reach.
    fs::remove_file(
        ctx.cache
            .path()
            .join("dists-v0/psr/log/f16e1d5863e37f8d8c2a01719f5b34baa2b714d3"),
    )
    .unwrap();
}

/// #123 follow-up: automatic adoption is best-effort per package. A private
/// dist behind an auth key vivace doesn't have (simulated here by a broken
/// URL) must not sink the whole install — Composer's own copy of that one
/// package stays on disk with a warning, and the rest of the tree still
/// adopts normally.
#[test]
fn best_effort_adopts_when_one_dist_is_unfetchable() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_e2e: set VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    composer_written_tree_with_unfetchable_dist(&ctx);
    let project = ctx.project.path();
    let kept = project.join("vendor/psr/log/composer.json");
    let kept_before = fs::read(&kept).unwrap();

    let adopted = ctx.viv().arg("install").output().unwrap();
    assert!(
        adopted.status.success(),
        "a best-effort adopt must not fail the whole install: {}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert!(
        String::from_utf8_lossy(&adopted.stderr).contains("Kept vendor/psr/log as a Composer copy"),
        "stderr: {}",
        String::from_utf8_lossy(&adopted.stderr)
    );
    assert!(
        String::from_utf8_lossy(&adopted.stdout)
            .contains("adopted 2 packages from a Composer install"),
        "stdout: {}",
        String::from_utf8_lossy(&adopted.stdout)
    );

    for target in [
        "vendor/monolog/monolog/composer.json",
        "vendor/psr/container/composer.json",
    ] {
        let nlink = fs::metadata(project.join(target)).unwrap().nlink();
        assert!(
            nlink > 1,
            "{target} should have been relinked from the store"
        );
    }
    let kept_meta = fs::metadata(&kept).unwrap();
    assert_eq!(
        kept_meta.nlink(),
        1,
        "psr/log should still be a plain Composer copy, not relinked"
    );
    assert_eq!(
        fs::read(&kept).unwrap(),
        kept_before,
        "a kept-as-copy package's bytes must be untouched"
    );

    let installed_json: serde_json::Value =
        serde_json::from_slice(&fs::read(project.join("vendor/composer/installed.json")).unwrap())
            .unwrap();
    let names: Vec<&str> = installed_json["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"psr/log"),
        "a kept-as-copy package must still be listed in installed.json"
    );
}

/// `--adopt` is opt-in, so a human asked for the relink and gets told when
/// it can't happen: unlike automatic adoption, it keeps today's hard
/// failure on an unfetchable dist.
#[test]
fn adopt_flag_still_hard_fails_on_an_unfetchable_dist() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_e2e: set VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    composer_written_tree_with_unfetchable_dist(&ctx);

    let output = ctx.viv().args(["install", "--adopt"]).output().unwrap();
    assert!(
        !output.status.success(),
        "--adopt should still hard-fail on an unfetchable dist: {}",
        String::from_utf8_lossy(&output.stdout)
    );
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

/// #37a: removing a `target-dir` package must remove its whole install root
/// (`vendor/symfony/yaml/`), not just the target-dir leaf `install-path`
/// points at (`vendor/symfony/yaml/Symfony/Component/Yaml`) — Composer
/// leaves no `Symfony/` scaffold behind either.
#[test]
fn removing_a_target_dir_package_clears_its_whole_install_root() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping install_e2e: set VIVACE_TEST_NETWORK=1 to fetch real dists over the network"
        );
        return;
    }

    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_legacy_sources(project);

    ctx.viv().arg("install").assert().success();
    assert!(
        project
            .join("vendor/symfony/yaml/Symfony/Component/Yaml")
            .is_dir()
    );

    for (path, key) in [
        (project.join("composer.json"), "require"),
        (project.join("composer.lock"), "packages"),
    ] {
        let mut value: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        if key == "require" {
            value["require"]
                .as_object_mut()
                .unwrap()
                .remove("symfony/yaml");
        } else {
            let packages = value["packages"].as_array_mut().unwrap();
            packages.retain(|p| p["name"] != "symfony/yaml");
        }
        fs::write(&path, serde_json::to_string_pretty(&value).unwrap()).unwrap();
    }

    ctx.viv()
        .args(["install", "--no-normalize"])
        .assert()
        .success();

    assert!(
        !project.join("vendor/symfony/yaml").exists(),
        "the whole target-dir install root should be gone, not just the leaf"
    );
}

/// #37g: nothing to install/remove, but a `scripts` listener still forces a
/// full run past the no-op fast path — Composer's own wording for this case
/// ("Nothing to install, update or remove" then "Generating autoload files"),
/// not the package-count summary, which would misleadingly read "Installed
/// 0 packages" for a run that only regenerated the autoloader.
#[test]
fn autoload_only_run_uses_composers_nothing_to_install_wording() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);
    let json_path = project.join("composer.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
    json["scripts"] = serde_json::json!({"post-install-cmd": []});
    fs::write(&json_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 2 packages"));

    // Second run: same lock, same composer.json, so the plan is a no-op —
    // but the `scripts` listener still forces a full run past the fast path.
    let second = ctx.viv().arg("install").output().unwrap();
    assert!(
        second.status.success(),
        "{}",
        String::from_utf8_lossy(&second.stderr)
    );
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("Nothing to install, update or remove"),
        "stdout: {stdout}"
    );
    assert!(stdout.contains("Generating autoload files"), "{stdout}");
    assert!(!stdout.contains("Installed"), "stdout: {stdout}");
}

/// #95: `install` must never write `composer.json` — normalizing moved to
/// `require`/`remove`/`update`, the commands that already rewrite it. The
/// path fixture's `composer.json` orders `repositories` before `require`,
/// which is not schema order, so a pre-#95 `install` would have rewritten
/// it; this asserts the file survives byte for byte instead.
#[test]
fn install_never_touches_composer_json() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);
    let json_path = project.join("composer.json");
    let before = fs::read(&json_path).unwrap();

    ctx.viv().arg("install").assert().success();

    let after = fs::read(&json_path).unwrap();
    assert_eq!(
        after, before,
        "viv install rewrote composer.json; it must only write under vendor/"
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

/// For a local-path `source.url` (this test's own upstream, standing in for
/// a real git remote so it never touches the network), real Composer's own
/// `.git/config` doesn't reliably carry `source.url` byte-for-byte in the
/// `composer` remote: `VcsDownloader::prepareUrls` resolves a local path
/// with `realpath()` before cloning, and `GitDownloader::doInstall`'s
/// trailing `updateOriginUrl` call only rewrites `origin` back to the raw
/// `source.url`, leaving `composer` at whatever `realpath()` produced (e.g.
/// macOS's `/var` -> `/private/var`). That's a quirk of Composer's local-path
/// handling, not part of the `source.url`-byte-for-byte contract this test
/// otherwise holds vivace to, so canonicalize just that one url before
/// comparing.
fn canonicalize_composer_remote_url(config: &str) -> String {
    let marker = "[remote \"composer\"]\n\turl = ";
    let Some(start) = config.find(marker) else {
        return config.to_string();
    };
    let url_start = start + marker.len();
    let url_end = config[url_start..]
        .find('\n')
        .map_or(config.len(), |i| url_start + i);
    let canonical = fs::canonicalize(&config[url_start..url_end]).map_or_else(
        |_| config[url_start..url_end].to_string(),
        |p| p.to_string_lossy().into_owned(),
    );
    format!("{}{canonical}{}", &config[..url_start], &config[url_end..])
}

/// #43/#59: a package with *both* a dist and a git `source` still gets
/// checked out from source, byte-identical `.git/config` and all, when
/// `config.preferred-install` says so — proven against real Composer 2.10.2
/// itself, not a hand-derived expectation, resolving the same package from
/// the same local (`file://`-free, no network) upstream repo both sides
/// point at.
#[test]
fn preferred_install_source_matches_composer_git_config() {
    if !git_available() {
        eprintln!("skipping preferred_install_source_matches_composer_git_config: git not on PATH");
        return;
    }
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!(
            "skipping preferred_install_source_matches_composer_git_config: composer is not on \
             PATH"
        );
        return;
    }

    let upstream = tempfile::tempdir().unwrap();
    init_upstream_repo(upstream.path());
    // A stable release, not a `dev-*` branch: the common case for
    // `preferred-install: source`, and the one whose `.git/config` doesn't
    // depend on this port's branch-vs-detached-HEAD logic (covered instead
    // by `source::tests::checkout_git_checks_out_a_local_branch_for_a_dev_version`).
    git_run(
        upstream.path(),
        &["-c", "tag.gpgsign=false", "tag", "1.0.0"],
    );
    let reference = git_head(upstream.path());
    let upstream_url = upstream.path().to_str().unwrap().to_string();

    // Real Composer, resolving a `type: vcs` repository over that same
    // local path (packagist explicitly disabled, so nothing here ever
    // touches the network) with `preferred-install: source`.
    let composer_project = tempfile::tempdir().unwrap();
    fs::write(
        composer_project.path().join("composer.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "name": "vivace/fixture-preferred-source",
            "version": "1.0.0",
            "repositories": [
                {"type": "vcs", "url": upstream_url},
                {"packagist.org": false},
            ],
            "require": {"acme/vcslib": "1.0.0"},
            "config": {"preferred-install": "source"},
        }))
        .unwrap(),
    )
    .unwrap();
    let install = Command::new("composer")
        .args(["install", "--no-interaction"])
        .current_dir(composer_project.path())
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "composer install failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&install.stdout),
        String::from_utf8_lossy(&install.stderr)
    );
    let want_config = fs::read_to_string(
        composer_project
            .path()
            .join("vendor/acme/vcslib/.git/config"),
    )
    .unwrap();
    let composer_installed: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(
            composer_project
                .path()
                .join("vendor/composer/installed.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let source = composer_installed["packages"][0]["source"].clone();
    assert_eq!(source["reference"], reference);

    // vivace, from a hand-written lock (no solver, no `repositories`):
    // the exact `source` block Composer's own VCS driver resolved, so a
    // byte-identical `.git/config` proves the remotes, refspec and
    // detached state all match, not just the reference.
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::write(
        project.join("composer.json"),
        r#"{"name": "vivace/fixture-preferred-source", "config": {"preferred-install": "source"}}"#,
    )
    .unwrap();
    let lock = serde_json::json!({
        "packages": [{
            "name": "acme/vcslib",
            "version": "1.0.0",
            // Never fetched: `preferred-install: source` steers this
            // package to `source::checkout_git` before the dist is ever
            // looked at, per `install::run`'s `archive_targets` filter.
            "dist": {
                "type": "zip",
                "url": "https://example.test/never-fetched.zip",
                "reference": reference,
                "shasum": "",
            },
            "source": source,
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
    let got_config = fs::read_to_string(checkout.join(".git/config")).unwrap();
    assert_eq!(
        canonicalize_composer_remote_url(&got_config),
        canonicalize_composer_remote_url(&want_config),
        "vendor/acme/vcslib/.git/config differs from real Composer's"
    );

    let installed: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.join("vendor/composer/installed.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(installed["packages"][0]["installation-source"], "source");
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

/// #63: a `metapackage` still declaring a `dist` (like `roots/wordpress`)
/// must never be fetched or linked, mirroring Composer's
/// `MetapackageInstaller` (installs nothing, `install-path: null`). The dist
/// URL points nowhere reachable; a fetch attempt would fail the install.
#[test]
fn metapackage_with_a_dist_is_never_fetched_or_linked() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_path_sources(project);

    let lock_path = project.join("composer.lock");
    let mut lock: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&lock_path).unwrap()).unwrap();
    lock["packages"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "acme/meta",
            "version": "1.0.0",
            "type": "metapackage",
            "dist": {
                "type": "zip",
                "url": "https://127.0.0.1:1/unreachable.zip",
                "reference": "0000000000000000000000000000000000000000"
            }
        }));
    fs::write(&lock_path, serde_json::to_string_pretty(&lock).unwrap()).unwrap();

    let json_path = project.join("composer.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&json_path).unwrap()).unwrap();
    json["require"]["acme/meta"] = serde_json::json!("1.0.0");
    fs::write(&json_path, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    // acme/hello + acme/testkit only; acme/meta installs nothing and does
    // not count.
    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 2 packages"));

    assert!(!project.join("vendor/acme/meta").exists());

    let installed: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(project.join("vendor/composer/installed.json")).unwrap(),
    )
    .unwrap();
    let meta = installed["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "acme/meta")
        .expect("acme/meta should still be recorded in installed.json");
    assert_eq!(meta["install-path"], serde_json::Value::Null);
}

/// #25: a classmap scan is cached per store archive so a rebuilt `vendor/`
/// (deleted, then `-o` re-run against the same warm cache) does not
/// retokenise every package's PHP files again — the output stays byte-
/// identical and a cache sidecar lands next to the archive's `.ok` marker.
#[test]
fn optimized_autoload_reuses_the_classmap_cache_after_vendor_is_rebuilt() {
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
        .args(["install", "-o"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 3 packages"));

    let classmap_path = project.join("vendor/composer/autoload_classmap.php");
    let first = fs::read(&classmap_path).unwrap();

    let sidecars: Vec<PathBuf> = fs::read_dir(ctx.cache.path().join("archive-v0"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path();
            (path.extension().and_then(|e| e.to_str()) == Some("classmap-v0")).then_some(path)
        })
        .collect();
    assert!(
        !sidecars.is_empty(),
        "an -o install should leave at least one classmap cache sidecar"
    );

    // Rebuild `vendor/` from scratch against the same (warm) store, as
    // `bench/run.sh`'s "warm" scenario does.
    fs::remove_dir_all(project.join("vendor")).unwrap();
    ctx.viv()
        .args(["install", "-o"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 3 packages"));

    let second = fs::read(&classmap_path).unwrap();
    assert_eq!(first, second, "cached classmap must match a fresh scan");
}

/// #23: a single-file zip, in memory, for pre-populating the store without
/// ever touching the network.
fn zip_of_one_file(name: &str, content: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(content).unwrap();
    writer.finish().unwrap().into_inner()
}

/// A lock package matching what `Store::add_zip` needs (name and dist), for
/// pre-populating the store the same way a prior `viv install` would have
/// left it — no `composer.lock` on disk needed for this.
fn store_package(name: &str) -> Package {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "version": "1.0.0",
        "dist": {
            "type": "zip",
            "url": "https://example.invalid/pkg.zip",
            "reference": "deadbeef",
            "shasum": "",
        },
    }))
    .unwrap()
}

/// #23: with every dist already in the store, `--offline` must still
/// install successfully — a warm cache is exactly the case offline mode
/// exists for, not just the case it tolerates.
#[test]
fn offline_install_succeeds_when_every_dist_is_already_in_the_store() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::write(
        project.join("composer.json"),
        r#"{"name": "acme/app", "require": {"acme/pkg": "^1.0"}}"#,
    )
    .unwrap();
    fs::write(
        project.join("composer.lock"),
        r#"{
            "packages": [
                {
                    "name": "acme/pkg",
                    "version": "1.0.0",
                    "dist": {
                        "type": "zip",
                        "url": "https://example.invalid/pkg.zip",
                        "reference": "deadbeef",
                        "shasum": ""
                    }
                }
            ],
            "packages-dev": []
        }"#,
    )
    .unwrap();

    // Pre-populate the store directly, the way a prior (online) install
    // would have left it — this test never makes a network request itself.
    let store = Store::open(ctx.cache.path()).unwrap();
    store
        .add_zip(
            &store_package("acme/pkg"),
            &zip_of_one_file("pkg.txt", b"hi"),
        )
        .unwrap();
    drop(store);

    ctx.viv()
        .args(["install", "--offline"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            "Installed 1 packages (1 from cache)",
        ));
    assert_eq!(
        fs::read(project.join("vendor/acme/pkg/pkg.txt")).unwrap(),
        b"hi"
    );
}

/// #23: nothing in the store and `--offline` set errors instead of
/// attempting a download, naming the missing package.
#[test]
fn offline_install_errors_naming_a_package_not_in_the_store() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::write(
        project.join("composer.json"),
        r#"{"name": "acme/app", "require": {"acme/pkg": "^1.0"}}"#,
    )
    .unwrap();
    fs::write(
        project.join("composer.lock"),
        r#"{
            "packages": [
                {
                    "name": "acme/pkg",
                    "version": "1.0.0",
                    "dist": {
                        "type": "zip",
                        "url": "https://example.invalid/pkg.zip",
                        "reference": "deadbeef",
                        "shasum": ""
                    }
                }
            ],
            "packages-dev": []
        }"#,
    )
    .unwrap();

    let output = ctx.viv().args(["install", "--offline"]).output().unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Network disabled, request canceled"),
        "{stderr}"
    );
    assert!(stderr.contains("acme/pkg"), "{stderr}");
}

/// #19: `--link-mode clone` reflinks store files into `vendor/`, falling
/// back to hardlink (with a warning) the first time a file can't. Reflink
/// only works on btrfs/XFS (Linux) or APFS (macOS), so `stat -f -c %T`
/// decides which branch this filesystem actually exercises rather than
/// asserting one outcome and skipping everywhere else.
#[test]
fn link_mode_clone_reflinks_or_falls_back_depending_on_the_filesystem() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::write(
        project.join("composer.json"),
        r#"{"name": "acme/app", "require": {"acme/pkg": "^1.0"}}"#,
    )
    .unwrap();
    fs::write(
        project.join("composer.lock"),
        r#"{
            "packages": [
                {
                    "name": "acme/pkg",
                    "version": "1.0.0",
                    "dist": {
                        "type": "zip",
                        "url": "https://example.invalid/pkg.zip",
                        "reference": "deadbeef",
                        "shasum": ""
                    }
                }
            ],
            "packages-dev": []
        }"#,
    )
    .unwrap();

    // Pre-populate the store directly, the same offline pattern
    // `offline_install_succeeds_when_every_dist_is_already_in_the_store`
    // uses — this test never makes a network request either.
    let store = Store::open(ctx.cache.path()).unwrap();
    store
        .add_zip(
            &store_package("acme/pkg"),
            &zip_of_one_file("pkg.txt", b"hi"),
        )
        .unwrap();
    drop(store);

    let output = ctx
        .viv()
        .args(["install", "--offline", "--link-mode", "clone"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    let dest = project.join("vendor/acme/pkg/pkg.txt");
    let meta = fs::symlink_metadata(&dest).unwrap();
    assert!(
        meta.file_type().is_file(),
        "a clone (or its hardlink/copy fallback) must still be a regular file"
    );
    assert_eq!(
        fs::read(&dest).unwrap(),
        b"hi",
        "content must match the store regardless of which mode actually ran"
    );

    let fs_type = Command::new("stat")
        .args(["-f", "-c", "%T", project.to_str().unwrap()])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string());
    let Some(fs_type) = fs_type else {
        eprintln!(
            "skipping link_mode_clone_reflinks_or_falls_back_depending_on_the_filesystem: \
             couldn't determine {}'s filesystem type",
            project.display()
        );
        return;
    };

    if fs_type == "btrfs" || fs_type.contains("xfs") {
        assert_eq!(
            meta.permissions().mode() & 0o200,
            0o200,
            "on {fs_type}, --link-mode clone must leave the file writable, not read-only"
        );
        assert!(
            !stderr.contains("cloning into vendor failed"),
            "{fs_type} supports reflink, so the fallback warning must not fire: {stderr}"
        );
    } else {
        assert_eq!(
            meta.permissions().mode() & 0o200,
            0,
            "{fs_type} has no reflink support, so the fallback to hardlink must stay read-only"
        );
        assert!(
            stderr.contains("cloning into vendor failed"),
            "{fs_type} has no reflink support, so the fallback warning must fire: {stderr}"
        );
    }
}

/// #52: native adapters for `dealerdirect/phpcodesniffer-composer-installer`,
/// `phpstan/extension-installer` and `tbachert/spi`; #101 adds
/// `php-http/discovery`. Each is byte-diffed against the artifact the real
/// plugin generates with Composer 2.10.2.
mod plugin_generators {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use crate::common::TestContext;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/plugins")
            .join(name)
    }

    fn copy_lock_sources(fixture_dir: &Path, project: &Path) {
        for name in ["composer.json", "composer.lock"] {
            fs::copy(fixture_dir.join(name), project.join(name)).unwrap();
        }
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

    fn skip_without_network() -> bool {
        if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
            eprintln!(
                "skipping plugin_generators test: set VIVACE_TEST_NETWORK=1 to fetch real dists \
                 over the network"
            );
            return true;
        }
        false
    }

    fn skip_without_php() -> bool {
        if Command::new("php").arg("--version").output().is_err() {
            eprintln!("skipping plugin_generators test: php not on PATH");
            return true;
        }
        false
    }

    /// `dealerdirect/phpcodesniffer-composer-installer` runs `phpcs
    /// --config-set installed_paths` after install, writing
    /// `vendor/squizlabs/php_codesniffer/CodeSniffer.conf` with every
    /// `phpcodesniffer-standard` package's ruleset directory, sorted and
    /// comma-joined.
    #[test]
    fn phpcs_installed_paths_matches_composer() {
        if skip_without_network() || skip_without_php() {
            return;
        }
        let ctx = TestContext::new();
        let project = ctx.project.path();
        copy_lock_sources(&fixture("phpcs"), project);

        ctx.viv()
            .arg("install")
            .assert()
            .success()
            .stdout(predicates::str::contains("Installed 5 packages"));

        let want = fs::read(fixture("phpcs").join("expected/CodeSniffer.conf")).unwrap();
        let got =
            fs::read(project.join("vendor/squizlabs/php_codesniffer/CodeSniffer.conf")).unwrap();
        assert_eq!(
            String::from_utf8_lossy(&got),
            String::from_utf8_lossy(&want)
        );
    }

    /// `phpstan/extension-installer` writes
    /// `vendor/phpstan/extension-installer/src/GeneratedConfig.php`, a
    /// `var_export` dump of every `extra.phpstan` package; the fixture's
    /// `install_path` field embeds the project's own absolute path, so the
    /// expected file is a template with a `{{PROJECT_DIR}}` placeholder.
    #[test]
    fn phpstan_generated_config_matches_composer() {
        if skip_without_network() {
            return;
        }
        let ctx = TestContext::new();
        let project = ctx.project.path();
        copy_lock_sources(&fixture("phpstan"), project);
        copy_tree(
            &fixture("phpstan").join("packages"),
            &project.join("packages"),
        );

        ctx.viv()
            .arg("install")
            .assert()
            .success()
            .stdout(predicates::str::contains("Installed 5 packages"));

        let template =
            fs::read_to_string(fixture("phpstan").join("expected/GeneratedConfig.php")).unwrap();
        let project_dir = fs::canonicalize(project).unwrap();
        let want = template.replace("{{PROJECT_DIR}}", &project_dir.display().to_string());
        let got = fs::read_to_string(
            project.join("vendor/phpstan/extension-installer/src/GeneratedConfig.php"),
        )
        .unwrap();
        assert_eq!(got, want);
    }

    /// `tbachert/spi` writes `vendor/composer/GeneratedServiceProviderData.php`
    /// from every package's `extra.spi`; the one declaring providers here is
    /// a local `path` repository package, so only `tbachert/spi` itself (and
    /// its own `composer/semver` dependency) needs the network.
    #[test]
    fn spi_generated_service_provider_data_matches_composer() {
        if skip_without_network() {
            return;
        }
        let ctx = TestContext::new();
        let project = ctx.project.path();
        copy_lock_sources(&fixture("spi"), project);
        copy_tree(&fixture("spi").join("packages"), &project.join("packages"));

        ctx.viv()
            .arg("install")
            .assert()
            .success()
            .stdout(predicates::str::contains("Installed 3 packages"));

        let want =
            fs::read_to_string(fixture("spi").join("expected/GeneratedServiceProviderData.php"))
                .unwrap();
        let got =
            fs::read_to_string(project.join("vendor/composer/GeneratedServiceProviderData.php"))
                .unwrap();
        assert_eq!(got, want);
    }

    /// #101: `php-http/discovery` writes `vendor/composer/GeneratedDiscoveryStrategy.php`,
    /// a `switch` mapping each interface pinned in root `extra.discovery` to
    /// its chosen implementation class; the resolver half (`postUpdate`,
    /// adding a missing `*-implementation` provider to `composer.json`) isn't
    /// ported (`docs/plugin-strategy.md`).
    #[test]
    fn discovery_generated_strategy_matches_composer() {
        if skip_without_network() {
            return;
        }
        let ctx = TestContext::new();
        let project = ctx.project.path();
        copy_lock_sources(&fixture("discovery"), project);

        ctx.viv()
            .arg("install")
            .assert()
            .success()
            .stdout(predicates::str::contains("Installed 4 packages"));

        let want = fs::read_to_string(
            fixture("discovery").join("expected/GeneratedDiscoveryStrategy.php"),
        )
        .unwrap();
        let got =
            fs::read_to_string(project.join("vendor/composer/GeneratedDiscoveryStrategy.php"))
                .unwrap();
        assert_eq!(got, want);
    }
}

/// #53: `cweagans/composer-patches` 2.0.0 (docs/plugin-strategy.md's rule 3):
/// patches from root `extra.patches` and the `patches-file` applied via a
/// temporary `git init`/`git apply` (2.0.0 dropped the plain `patch` CLI
/// patcher 1.x had — `src/plugins/patches.rs`'s own doc comment), with
/// `patches.lock.json` byte-diffed against the real plugin's own. Gated on
/// `git`, not `php`: viv never runs the plugin's PHP, only mirrors what it
/// would have applied.
mod composer_patches {
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use crate::common::TestContext;

    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/composer-patches")
    }

    fn copy_sources(project: &Path) {
        for name in ["composer.json", "composer.lock", "patches.json"] {
            fs::copy(fixture().join(name), project.join(name)).unwrap();
        }
        super::copy_tree(&fixture().join("patches"), &project.join("patches"));
    }

    fn skip_without_network() -> bool {
        if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
            eprintln!(
                "skipping composer_patches test: set VIVACE_TEST_NETWORK=1 to fetch real dists \
                 over the network"
            );
            return true;
        }
        false
    }

    fn skip_without_git() -> bool {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("skipping composer_patches test: git not on PATH");
            return true;
        }
        false
    }

    #[test]
    fn applies_patches_and_writes_lock_matching_composer() {
        if skip_without_network() || skip_without_git() {
            return;
        }
        let ctx = TestContext::new();
        let project = ctx.project.path();
        copy_sources(project);

        ctx.viv()
            .arg("install")
            .assert()
            .success()
            .stdout(predicates::str::contains("Installed 3 packages"));

        super::assert_matches_expected(&fixture().join("expected/dev"), &project.join("vendor"));
        assert_eq!(
            fs::read(project.join("vendor/psr/log/src/LogLevel.php")).unwrap(),
            fs::read(fixture().join("expected/patched/psr-log/src/LogLevel.php")).unwrap(),
            "patch from root extra.patches"
        );
        assert_eq!(
            fs::read(project.join("vendor/psr/log/src/NullLogger.php")).unwrap(),
            fs::read(fixture().join("expected/patched/psr-log/src/NullLogger.php")).unwrap(),
            "patch from the patches-file"
        );
        assert_eq!(
            fs::read(project.join("patches.lock.json")).unwrap(),
            fs::read(fixture().join("expected/patches.lock.json")).unwrap(),
            "patches.lock.json must match the real plugin's own, byte for byte"
        );

        // Patching breaks this package's store hardlink: an independent,
        // writable copy, never the store's shared read-only inode.
        let mode = fs::metadata(project.join("vendor/psr/log/composer.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_ne!(
            mode, 0o444,
            "a patched package must not keep sharing the store's read-only inode"
        );
        assert!(
            !project.join("vendor/psr/log/.git").exists(),
            "GitInitPatcher's temporary repo must be cleaned up"
        );

        // Re-run: nothing changed, so it's a no-op (and doesn't re-run git).
        ctx.viv()
            .arg("install")
            .assert()
            .success()
            .stdout(predicates::str::contains("Nothing to install"));

        // Editing only the patches file changes neither composer.lock nor
        // composer.json, so the ordinary plan/state diff can't see it —
        // `State::patches_fingerprint` (src/install.rs) must still catch it
        // and re-patch from a pristine store copy.
        fs::write(project.join("patches.json"), r#"{ "patches": {} }"#).unwrap();
        ctx.viv().arg("install").assert().success();
        assert_eq!(
            fs::read(project.join("vendor/psr/log/src/NullLogger.php")).unwrap(),
            fs::read(fixture().join("expected/pristine/psr-log/src/NullLogger.php")).unwrap(),
            "dropping the patches-file patch must revert this file"
        );
        assert_eq!(
            fs::read(project.join("vendor/psr/log/src/LogLevel.php")).unwrap(),
            fs::read(fixture().join("expected/patched/psr-log/src/LogLevel.php")).unwrap(),
            "the root extra.patches patch is untouched by the patches-file change"
        );
    }

    /// `apply`'s `link_tree(&store_dir, &install_path, LinkMode::Copy)` must
    /// break the hardlink before `git apply` touches the file, never write
    /// through into the store's shared, read-only tree.
    #[test]
    fn patching_never_writes_through_store_hardlinks() {
        if skip_without_network() || skip_without_git() {
            return;
        }
        let ctx = TestContext::new();
        let project = ctx.project.path();
        copy_sources(project);

        ctx.viv().arg("install").assert().success();

        // `dists-v0/psr/log/<reference>` is a relative symlink to the
        // archive dir this test's isolated cache resolved for psr/log.
        let dists_dir = ctx.cache.path().join("dists-v0/psr/log");
        let reference = fs::read_dir(&dists_dir).unwrap().next().unwrap().unwrap();
        let store_dir = fs::canonicalize(reference.path()).unwrap();
        let store_log_level = store_dir.join("src/LogLevel.php");

        let store_bytes = fs::read(&store_log_level).unwrap();
        let patched =
            fs::read(fixture().join("expected/patched/psr-log/src/LogLevel.php")).unwrap();
        assert_ne!(store_bytes, patched, "the store copy must stay unpatched");
        assert!(
            !String::from_utf8_lossy(&store_bytes)
                .contains("patched by cweagans/composer-patches fixture"),
            "the store copy must not carry the patch"
        );

        let store_meta = fs::metadata(&store_log_level).unwrap();
        assert_eq!(
            store_meta.nlink(),
            1,
            "the vendor copy must no longer share the store's inode"
        );
        assert_eq!(
            store_meta.permissions().mode() & 0o222,
            0,
            "the store copy must stay read-only"
        );
    }
}
