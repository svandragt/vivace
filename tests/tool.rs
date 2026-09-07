//! `viv x`/`viv run`/`viv exec` (#85, `src/tool.rs`): `run`/`exec` are
//! hermetic (a fixture `composer.json` and a hand-written `vendor/bin`
//! proxy, no network); `viv x` needs a real Packagist resolve and dist
//! download for its first run, gated on `VIVACE_TEST_NETWORK=1` like
//! `tests/install_e2e.rs`, and asserts the second run touches neither.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not app logging"
)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::TestContext;
use predicates::prelude::*;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tool/run-exec")
}

fn setup(project: &Path) {
    std::fs::copy(
        fixture().join("composer.json"),
        project.join("composer.json"),
    )
    .unwrap();
}

#[test]
fn run_list_prints_every_script_in_declaration_order() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv()
        .args(["run", "--list"])
        .assert()
        .success()
        .stdout("build\nechoargs\n");
}

#[test]
fn run_executes_a_named_script_and_appends_trailing_args() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv()
        .args(["run", "echoargs", "--", "extra1", "extra2"])
        .assert()
        .success()
        .stdout("hi extra1 extra2\n");
}

#[test]
fn run_an_unknown_script_fails_naming_it() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv()
        .args(["run", "nope"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "Script \"nope\" is not defined in this package",
        ));
}

/// `vendor/bin/mytool` is a plain shell script written by the test, not the
/// fixture: `run`/`exec` never care how a bin got there, and a hand-written
/// script keeps the fixture free of a committed executable bit.
fn write_vendor_bin_tool(project: &Path) {
    let bin_dir = project.join("vendor/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = "#!/bin/sh\necho \"first:$(echo $PATH | cut -d: -f1)\"\necho \"args:$@\"\n";
    let path = bin_dir.join("mytool");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

#[test]
fn exec_prepends_vendor_bin_to_path_and_passes_args_through() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    setup(project);
    write_vendor_bin_tool(project);

    let expected_bin_dir = std::fs::canonicalize(project).unwrap().join("vendor/bin");
    ctx.viv()
        .args(["exec", "mytool", "--", "foo", "bar"])
        .assert()
        .success()
        .stdout(format!(
            "first:{}\nargs:foo bar\n",
            expected_bin_dir.display()
        ));
}

#[test]
fn exec_an_unknown_bin_fails_naming_the_path() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv()
        .args(["exec", "nope"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("nope"));
}

/// `viv x`'s tiny recorded-fixture package: no dependencies beyond `php`/
/// `ext-json`, a single `bin` entry, so a first run only ever needs one dist
/// download.
const TOOL_PACKAGE: &str = "php-parallel-lint/php-parallel-lint";

fn skip_without_network() -> bool {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv x test: set VIVACE_TEST_NETWORK=1 to resolve and fetch {TOOL_PACKAGE} \
             over the network"
        );
        return true;
    }
    false
}

fn skip_without_php() -> bool {
    if Command::new("php").arg("--version").output().is_err() {
        eprintln!("skipping viv x test: php is not on PATH");
        return true;
    }
    false
}

/// First run resolves and installs over the network (`Installed 1 packages`
/// in stdout) and execs the bin; a second run of the same spec, passed
/// `--offline`, must still succeed and produce the same output — the only
/// way that's possible is the cache hit skipping resolve/install entirely,
/// since `--offline` would otherwise error naming every package not already
/// fetched.
#[test]
fn x_installs_once_then_execs_from_cache_without_any_network() {
    if skip_without_network() || skip_without_php() {
        return;
    }

    let ctx = TestContext::new();
    // `viv x` never reads the project's own composer.json/lock, but it does
    // still need a project dir to run in.
    std::fs::create_dir_all(ctx.project.path()).unwrap();

    ctx.viv()
        .args(["x", TOOL_PACKAGE, "--", "--version"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 1 packages"))
        .stdout(predicates::str::contains("PHP Parallel Lint version"));

    ctx.viv()
        .args(["--offline", "x", TOOL_PACKAGE, "--", "--version"])
        .assert()
        .success()
        .stdout(predicates::str::contains("PHP Parallel Lint version"))
        .stdout(predicates::str::contains("Installed").not());

    ctx.viv()
        .args(["x", "--list"])
        .assert()
        .success()
        .stdout(predicates::str::contains(TOOL_PACKAGE));

    ctx.viv()
        .args(["x", "--uninstall", TOOL_PACKAGE])
        .assert()
        .success();
    ctx.viv()
        .args(["x", "--list"])
        .assert()
        .success()
        .stdout(predicates::str::contains(TOOL_PACKAGE).not());
}
