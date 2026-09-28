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

    // No `--` before the trailing args (#340): `command` is one positional
    // now, so nothing after the script name needs shielding from viv's own
    // flag parsing any more.
    ctx.viv()
        .args(["run", "echoargs", "extra1", "extra2"])
        .assert()
        .success()
        .stdout("hi extra1 extra2\n");
}

/// Not a declared script, not in `vendor/bin` (the fixture project has
/// none), and not on `PATH` either: `viv run`'s three-way fallback chain
/// (#338) replaces the old plain "Script is not defined" error with one
/// naming all three lookups it tried.
#[test]
fn run_an_unknown_script_fails_naming_all_three_lookups() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv()
        .args(["run", "nope"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "\"nope\": not a script in composer.json, not in vendor/bin, and not on PATH",
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
    // No `--` before the trailing args (#340), same as `run` above.
    ctx.viv()
        .args(["exec", "mytool", "foo", "bar"])
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

/// `vendor/bin/echoargs` prints each of its own argv entries on its own
/// line, so a run/exec that let one of them leak into viv's own flag
/// parsing (#340) shows up as a missing or reordered line rather than a
/// silent hang.
fn write_vendor_bin_echoargs(project: &Path) {
    let bin_dir = project.join("vendor/bin");
    std::fs::create_dir_all(&bin_dir).unwrap();
    let script = "#!/bin/sh\nfor a in \"$@\"; do echo \"$a\"; done\n";
    let path = bin_dir.join("echoargs");
    std::fs::write(&path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// #340: `viv run echoargs -v -d x -h --list` used to hand `-v`/`-d x`/`-h`
/// to viv's own global/`run` flags instead of `echoargs`, since clap
/// matched a global flag before `command`'s trailing var-arg positional had
/// captured any value at all. No `scripts` entry named `echoargs` here
/// (unlike the fixture project), so this exercises the `vendor/bin`
/// fallback, not `scripts::Runner`.
#[test]
fn run_passes_every_flag_looking_argument_through_to_the_script() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    std::fs::write(project.join("composer.json"), "{}").unwrap();
    write_vendor_bin_echoargs(project);

    ctx.viv()
        .args(["run", "echoargs", "-v", "-d", "x", "-h", "--list"])
        .assert()
        .success()
        .stdout("-v\n-d\nx\n-h\n--list\n");
}

/// Same bug, `exec`'s own positional.
#[test]
fn exec_passes_a_flag_looking_argument_through_to_the_bin() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    std::fs::write(project.join("composer.json"), "{}").unwrap();
    write_vendor_bin_echoargs(project);

    ctx.viv()
        .args(["exec", "echoargs", "-v"])
        .assert()
        .success()
        .stdout("-v\n");
}

/// `<cache>/php-v0/<version>-<os>-<arch>/php` (#338): a fake install
/// mirroring `php::install_dir`'s own naming, no real download — an
/// executable shell script that echoes an unmistakable marker plus its own
/// args, so a script/bin that runs `php`/`@php` can prove it hit *this*
/// binary and not whatever real `php` sits on the system PATH.
fn fake_php_install(cache_dir: &Path, version: &str) {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    let dir = cache_dir
        .join("php-v0")
        .join(format!("{version}-{os}-{arch}"));
    std::fs::create_dir_all(&dir).unwrap();
    let php_path = dir.join("php");
    std::fs::write(
        &php_path,
        format!("#!/bin/sh\necho \"FAKE-PHP {version} $@\"\n"),
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&php_path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    std::fs::write(dir.join(".ok"), b"").unwrap();
}

fn write_executable(path: &Path, script: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, script).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

/// A `config.platform.php`-pinned project's `composer.json`: `pin` is
/// either a minor (`"8.4"`) or an exact (`"8.5.0"`) version, matching
/// `config.platform.php`'s own two accepted shapes.
fn write_pinned_composer_json(project: &Path, pin: &str) {
    std::fs::write(
        project.join("composer.json"),
        format!(
            "{{\n    \"config\": {{ \"platform\": {{ \"php\": \"{pin}\" }} }},\n    \
             \"scripts\": {{ \"v\": \"php -v\" }}\n}}\n"
        ),
    )
    .unwrap();
}

#[test]
fn run_uses_the_pinned_php_for_a_script() {
    let ctx = TestContext::new();
    fake_php_install(ctx.cache.path(), "8.4.17");
    write_pinned_composer_json(ctx.project.path(), "8.4");

    ctx.viv()
        .args(["run", "v"])
        .assert()
        .success()
        .stdout(predicates::str::contains("FAKE-PHP"));
}

#[test]
fn run_falls_back_to_vendor_bin_and_the_pinned_php_wins_on_path() {
    let ctx = TestContext::new();
    fake_php_install(ctx.cache.path(), "8.4.17");
    write_pinned_composer_json(ctx.project.path(), "8.4");
    write_executable(
        &ctx.project.path().join("vendor/bin/tool"),
        "#!/bin/sh\necho TOOL\nphp -v\n",
    );

    ctx.viv()
        .args(["run", "tool"])
        .assert()
        .success()
        .stdout(predicates::str::contains("TOOL"))
        .stdout(predicates::str::contains("FAKE-PHP"));
}

#[test]
fn run_falls_back_to_path_for_a_binary_not_a_script_or_vendor_bin() {
    let ctx = TestContext::new();
    setup(ctx.project.path());

    ctx.viv().args(["run", "true"]).assert().success();
}

#[test]
fn run_and_exec_error_when_the_pinned_php_is_not_installed() {
    let ctx = TestContext::new();
    write_pinned_composer_json(ctx.project.path(), "8.5.0");
    write_executable(
        &ctx.project.path().join("vendor/bin/tool"),
        "#!/bin/sh\necho TOOL\n",
    );

    let not_installed =
        "php 8.5.0 is pinned in composer.json but not installed; run viv php install";
    ctx.viv()
        .args(["run", "v"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(not_installed));
    ctx.viv()
        .args(["exec", "tool"])
        .assert()
        .failure()
        .stderr(predicates::str::contains(not_installed));
}

#[test]
fn run_uses_the_path_php_when_nothing_is_pinned() {
    if skip_without_php() {
        return;
    }
    let ctx = TestContext::new();
    std::fs::write(
        ctx.project.path().join("composer.json"),
        "{\n    \"scripts\": { \"v\": \"php -v\" }\n}\n",
    )
    .unwrap();

    ctx.viv().args(["run", "v"]).assert().success();
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

    // No `--` before `--version` (#340): `command`'s trailing var-arg
    // positional passes a hyphenated argument through untouched once the
    // package name has already filled it, `--` would now be captured
    // literally rather than stripped as a separator.
    ctx.viv()
        .args(["x", TOOL_PACKAGE, "--version"])
        .assert()
        .success()
        .stdout(predicates::str::contains("Installed 1 packages"))
        .stdout(predicates::str::contains("PHP Parallel Lint version"));

    ctx.viv()
        .args(["--offline", "x", TOOL_PACKAGE, "--version"])
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
