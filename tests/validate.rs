//! `viv validate` (#70): a native `ValidateCommand`/`ConfigValidator` port,
//! byte-diffed against Composer 2.10.2's own `validate --no-ansi` output
//! (recorded per fixture under `tests/fixtures/validate/<scenario>/expected/`,
//! `<flags>.out`/`<flags>.err`).
//!
//! `errors/` reproduces a bad name and an unparseable constraint via
//! `--with-dependencies`, not the root file: a bad root name aborts real
//! `composer validate` inside `RootPackageLoader` before `ConfigValidator`
//! ever runs (a different, `Factory`-rendered crash this port doesn't
//! reproduce — see `src/validate.rs`'s module doc), so an installed
//! dependency's own `composer.json` is the only place those two errors show
//! up in `ConfigValidator`'s own formatted output.

use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;

fn fixture(scenario: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/validate")
        .join(scenario)
}

fn run(scenario: &str, args: &[&str]) -> Output {
    Command::new(cargo_bin("viv"))
        .current_dir(fixture(scenario))
        .arg("validate")
        .args(args)
        .output()
        .expect("failed to run viv")
}

fn expected(scenario: &str, name: &str) -> String {
    fs_err::read_to_string(fixture(scenario).join("expected").join(name)).unwrap()
}

/// Asserts `viv validate <args>` in `scenario` matches Composer's recorded
/// `expected/<label>.{out,err}` and exit code, byte-for-byte.
fn assert_matches_composer(scenario: &str, args: &[&str], label: &str, exit_code: i32) {
    let output = run(scenario, args);
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "{scenario} {args:?}: exit code"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected(scenario, &format!("{label}.out")),
        "{scenario} {args:?}: stdout"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        expected(scenario, &format!("{label}.err")),
        "{scenario} {args:?}: stderr"
    );
}

#[test]
fn valid_project_passes_plain_and_strict() {
    assert_matches_composer("valid", &[], "plain", 0);
    assert_matches_composer("valid", &["--strict"], "strict", 0);
}

/// Missing license + an unbound `*` constraint: warnings only, exit 0 unless
/// `--strict`.
#[test]
fn warnings_only_exit_zero_unless_strict() {
    assert_matches_composer("warnings", &[], "plain", 0);
    assert_matches_composer("warnings", &["--strict"], "strict", 1);
}

/// A bad name and an unparseable constraint on an installed dependency,
/// found via `--with-dependencies`: exit 2 regardless of `--strict` (errors
/// always exit non-zero).
#[test]
fn dependency_errors_exit_two_with_or_without_strict() {
    assert_matches_composer("errors", &["--with-dependencies"], "plain", 2);
    assert_matches_composer("errors", &["--with-dependencies", "--strict"], "strict", 2);
}

/// #33-style stale `content-hash`: a lock file error by default, downgraded
/// to a warning (and a clean exit) by `--no-check-lock`.
#[test]
fn stale_lock_is_an_error_unless_no_check_lock() {
    assert_matches_composer("stale-lock", &[], "plain", 2);
    assert_matches_composer("stale-lock", &["--strict"], "strict", 2);
    assert_matches_composer("stale-lock", &["--no-check-lock"], "no-check-lock", 0);
}

/// A file argument that doesn't exist exits `3` (`ValidateCommand`'s own
/// doc block), naming the file the same way Composer does.
#[test]
fn missing_file_exits_three() {
    let ctx = tempfile::tempdir().unwrap();
    let output = Command::new(cargo_bin("viv"))
        .current_dir(ctx.path())
        .args(["validate", "missing.json"])
        .output()
        .expect("failed to run viv");
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "missing.json not found.\n"
    );
}
