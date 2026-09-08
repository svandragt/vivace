//! `viv install`/`dump-autoload` script (event) dispatch: order, the
//! Composer-shaped environment, `@php`/`@putenv`/chained-script listeners,
//! a failing listener aborting with the event named, and a static
//! `Class::method` callback warning instead of failing (`src/scripts.rs`,
//! GitHub issue #11).
//!
//! Every fixture's lock has no packages, so `viv install` needs no network
//! fetch either: it still runs the full pipeline (nothing to fetch or
//! link), which is enough to exercise `pre-install-cmd`/`post-install-cmd`
//! without `VIVACE_TEST_NETWORK=1`.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not app logging"
)]

#[macro_use]
mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestContext;

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scripts")
}

/// Copy `<fixture>/composer.json` and the shared empty `lock.json` (as
/// `composer.lock`) into `project`.
fn setup(project: &Path, fixture: &str) {
    fs::copy(
        fixtures_dir().join(fixture).join("composer.json"),
        project.join("composer.json"),
    )
    .unwrap();
    fs::copy(
        fixtures_dir().join("lock.json"),
        project.join("composer.lock"),
    )
    .unwrap();
}

/// `dump-autoload` refuses to run against a `vendor/` that doesn't exist
/// yet; `install` creates it itself, so only the dump-autoload tests need
/// this.
fn setup_vendor(project: &Path) {
    fs::create_dir_all(project.join("vendor/composer")).unwrap();
    // Composer prepends bin-dir to PATH only when the directory exists.
    fs::create_dir_all(project.join("vendor/bin")).unwrap();
}

fn read(project: &Path, name: &str) -> String {
    fs::read_to_string(project.join(name)).unwrap_or_else(|err| panic!("reading {name}: {err}"))
}

#[test]
fn install_dispatches_events_in_composer_order() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "order");

    ctx.viv().arg("install").assert().success();

    let log = read(project, "events.log");
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(
        lines,
        vec![
            "pre-install-cmd",
            "pre-autoload-dump",
            "post-autoload-dump",
            "post-install-cmd",
        ],
        "install must dispatch pre-install-cmd, then (after fetch/link) \
         pre-autoload-dump, post-autoload-dump, then post-install-cmd"
    );
}

/// vivace's no-op fast path ("Nothing to install") must not skip Composer's
/// events when the root defines scripts: a second, otherwise-unchanged
/// install still has to run `post-install-cmd`, since callers rely on it
/// running every time (`--no-scripts` still opts back out).
#[test]
fn a_second_install_still_runs_events_when_scripts_are_defined() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "order");

    ctx.viv().arg("install").assert().success();
    assert_eq!(read(project, "events.log").lines().count(), 4);

    ctx.viv().arg("install").assert().success();
    assert_eq!(
        read(project, "events.log").lines().count(),
        8,
        "a second, no-op install must still dispatch every event when scripts are defined"
    );
}

#[test]
fn dump_autoload_only_dispatches_autoload_events() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "order");
    setup_vendor(project);

    ctx.viv().arg("dump-autoload").assert().success();

    let log = read(project, "events.log");
    assert_eq!(
        log.lines().collect::<Vec<_>>(),
        vec!["pre-autoload-dump", "post-autoload-dump"],
        "dump-autoload has no install to run pre/post-install-cmd around"
    );
}

#[test]
fn shell_and_php_listeners_see_the_composer_environment() {
    if Command::new("php").arg("--version").output().is_err() {
        eprintln!(
            "skipping shell_and_php_listeners_see_the_composer_environment: php is not on PATH"
        );
        return;
    }

    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "env-and-php");
    setup_vendor(project);

    ctx.viv().arg("dump-autoload").assert().success();

    let env_marker = read(project, "env.marker");
    assert!(
        env_marker.contains("dev=1"),
        "COMPOSER_DEV_MODE should be 1 in dev mode: {env_marker:?}"
    );
    let expected_bin_dir = fs::canonicalize(project).unwrap().join("vendor/bin");
    let path = env_marker
        .trim()
        .strip_prefix("dev=1 path=")
        .unwrap_or_else(|| panic!("unexpected env.marker: {env_marker:?}"));
    assert!(
        path.split(':').next() == Some(expected_bin_dir.display().to_string().as_str()),
        "vendor/bin ({}) should be prepended to PATH: {env_marker:?}",
        expected_bin_dir.display()
    );

    assert_eq!(
        read(project, "php.marker"),
        "php-ran",
        "an @php listener should run the current PHP binary"
    );
}

#[test]
fn putenv_carries_to_a_chained_script() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "putenv-chain");
    setup_vendor(project);

    ctx.viv().arg("dump-autoload").assert().success();

    assert_eq!(
        read(project, "putenv.marker").trim(),
        "var=hello",
        "@putenv should still be visible in the @other-script it chains to"
    );
}

#[test]
fn a_failing_script_aborts_with_the_event_name_in_stderr() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "fail");
    setup_vendor(project);

    let output = ctx.viv().arg("dump-autoload").output().unwrap();
    assert!(
        !output.status.success(),
        "a failing listener must abort the run"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("pre-autoload-dump"),
        "stderr should name the event: {stderr}"
    );
    assert!(
        stderr.contains("exit 3"),
        "stderr should name the failing listener: {stderr}"
    );
    // The listener before the failing one still ran; nothing after an
    // aborted event should have (there is nothing after it here).
    assert_eq!(read(project, "events.log").trim(), "failing");
}

#[test]
fn a_static_php_callback_warns_and_continues() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "warn");
    setup_vendor(project);

    let output = ctx.viv().arg("dump-autoload").output().unwrap();
    assert!(
        output.status.success(),
        "a Class::method listener should warn, not fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("App\\Installer::postInstall") && stderr.contains("pre-autoload-dump"),
        "stderr should warn naming the script and event: {stderr}"
    );
    assert!(
        !stderr.contains("WARN") && !stderr.contains('Z'),
        "the skip notice should be a plain warn_out line, not tracing::warn!'s \
         timestamp and level: {stderr}"
    );
    assert_eq!(
        read(project, "events.log").trim(),
        "continued",
        "the listener after the skipped one should still run"
    );
}

/// GitHub issue #154: `Composer\Config::disableProcessTimeout` only lifts
/// Composer's process timeout, which vivace never applies, so it's known
/// inert and skipped with no output at all.
#[test]
fn an_inert_static_callback_is_silent() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "inert-callback");

    let output = ctx.viv().args(["run", "serve"]).output().unwrap();
    assert!(
        output.status.success(),
        "an inert callback should not fail the run: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "served",
        "stdout should only carry the script's own output"
    );
    assert!(
        output.stderr.is_empty(),
        "an inert callback should print nothing, not even a debug warning: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// GitHub issue #154: an unknown static callback still warns, but as a
/// plain stderr line rather than through `tracing::warn!`.
#[test]
fn an_unknown_static_callback_warns_in_plain_text() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "run-warn");

    let output = ctx.viv().args(["run", "task"]).output().unwrap();
    assert!(
        output.status.success(),
        "a Class::method listener should warn, not fail: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim(),
        "ok",
        "the listener after the skipped one should still run"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Skipping Acme\\Foo::bar"),
        "stderr should name the skipped listener: {stderr}"
    );
    assert!(
        !stderr.contains("WARN") && !stderr.contains('Z'),
        "the skip notice should be a plain warn_out line, not tracing::warn!'s \
         timestamp and level: {stderr}"
    );
}

#[test]
fn no_scripts_skips_everything() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project, "fail");
    setup_vendor(project);

    ctx.viv()
        .args(["dump-autoload", "--no-scripts"])
        .assert()
        .success();

    assert!(
        !project.join("events.log").exists(),
        "--no-scripts must skip every listener, including the one that would fail"
    );
}
