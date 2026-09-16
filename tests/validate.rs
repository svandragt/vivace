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
//!
//! `fixable/` (#262, `--fix`) hits the same `RootPackageLoader` abort for
//! two of the six fixable findings: a self-require and an upper-case name
//! both crash real `composer validate` on the root file (verified with
//! `devbox run -- composer validate` against each in isolation), so its
//! `composer.json` carries the other four (stale lock, require/require-dev
//! overlap, a require a provide already shadows, an orphaned
//! `scripts-descriptions` entry) plus one finding `--fix` leaves alone (a
//! missing `description`); `src/validate.rs`'s own `tests` module covers the
//! self-require/rename fixes directly, without a Composer recording to
//! diff against.

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

/// Asserts `viv validate <args>` in `scenario` starts with Composer's
/// recorded `expected/<label>.{out,err}` bytes exactly, and exit code
/// matches. `--fix` (#262) appends its own lines after Composer's own
/// report, so this is a prefix match, not equality: whatever follows the
/// recorded bytes must be empty or start with the blank line viv's own
/// output adds before appending.
fn assert_matches_composer(scenario: &str, args: &[&str], label: &str, exit_code: i32) {
    let output = run(scenario, args);
    assert_eq!(
        output.status.code(),
        Some(exit_code),
        "{scenario} {args:?}: exit code"
    );
    assert_starts_with_composer(
        &String::from_utf8_lossy(&output.stdout),
        &expected(scenario, &format!("{label}.out")),
        &format!("{scenario} {args:?}: stdout"),
    );
    assert_starts_with_composer(
        &String::from_utf8_lossy(&output.stderr),
        &expected(scenario, &format!("{label}.err")),
        &format!("{scenario} {args:?}: stderr"),
    );
}

fn assert_starts_with_composer(actual: &str, recorded: &str, context: &str) {
    assert!(
        actual.starts_with(recorded),
        "{context}: does not start with Composer's recorded bytes\n--- actual ---\n{actual}\n--- recorded ---\n{recorded}"
    );
    let rest = &actual[recorded.len()..];
    assert!(
        rest.is_empty() || rest.starts_with('\n'),
        "{context}: bytes after Composer's own output must start with a blank line, got {rest:?}"
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

/// #233: a manifest missing both `name` and `description` fails Composer's
/// publish check (schema-required, not any hand-written check) and exits 2;
/// `--no-check-publish` suppresses it back down to the license warning and
/// exit 0.
#[test]
fn missing_required_publish_fields_exit_two_unless_no_check_publish() {
    assert_matches_composer("publish-errors", &[], "plain", 2);
    assert_matches_composer(
        "publish-errors",
        &["--no-check-publish"],
        "no-check-publish",
        0,
    );
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

/// #234: `viv validate -d <dir>` behaves as `cd <dir> && viv validate`,
/// run from an unrelated cwd so a bug that resolved paths from the current
/// directory instead of `-d` would still pass in-fixture-dir tests.
#[test]
fn project_dir_behaves_like_cd_into_it() {
    let cwd = tempfile::tempdir().unwrap();
    let output = Command::new(cargo_bin("viv"))
        .current_dir(cwd.path())
        .args(["validate", "-d"])
        .arg(fixture("valid"))
        .output()
        .expect("failed to run viv");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected("valid", "plain.out")
    );
}

/// #234: `--project-dir` also drives the lock-freshness check, not just
/// which `composer.json` gets read.
#[test]
fn project_dir_checks_that_directory_own_lock_freshness() {
    let cwd = tempfile::tempdir().unwrap();
    let output = Command::new(cargo_bin("viv"))
        .current_dir(cwd.path())
        .args(["validate", "-d"])
        .arg(fixture("stale-lock"))
        .output()
        .expect("failed to run viv");
    assert_eq!(output.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected("stale-lock", "plain.out")
    );
}

/// #234: a `FILE` positional and `--project-dir` together are ambiguous
/// about which lock is meant, so it's an error rather than one silently
/// winning.
#[test]
fn project_dir_combined_with_file_is_an_error() {
    let output = Command::new(cargo_bin("viv"))
        .args(["validate", "-d"])
        .arg(fixture("valid"))
        .arg(fixture("valid").join("composer.json"))
        .output()
        .expect("failed to run viv");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("cannot combine a FILE argument"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// #262: without `--fix`, a count of how many findings could be
/// auto-fixed, after a blank line following Composer's own report.
#[test]
fn fixable_count_line_follows_composers_report() {
    assert_matches_composer("fixable", &[], "plain", 2);
    let output = run("fixable", &[]);
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        format!(
            "{}\n4 of 5 findings can be fixed: run viv validate --fix\n",
            expected("fixable", "plain.err")
        )
    );
}

/// Copies `scenario`'s `composer.json`/`composer.lock` into `dest`, so a
/// test that mutates them (`--fix`) never touches the committed fixture.
fn copy_fixture_project(scenario: &str, dest: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs_err::copy(fixture(scenario).join(name), dest.join(name)).unwrap();
    }
}

/// #262: `--fix` applies every fixable finding in a temp copy, writes
/// `composer.json` back through the normaliser, rewrites the stale lock's
/// `content-hash`, and re-runs validation reporting what's left; a second
/// plain run afterwards sees only the one finding `--fix` leaves alone (a
/// missing `description`), with no count line (nothing left to fix).
#[test]
fn fix_applies_every_fixable_finding() {
    let dir = tempfile::tempdir().unwrap();
    copy_fixture_project("fixable", dir.path());

    let output = Command::new(cargo_bin("viv"))
        .current_dir(dir.path())
        .args(["validate", "--fix"])
        .output()
        .expect("failed to run viv");
    assert_eq!(output.status.code(), Some(2), "{output:?}");

    let fixed_json = fs_err::read_to_string(dir.path().join("composer.json")).unwrap();
    let normalized_expected = {
        let scratch = tempfile::tempdir().unwrap();
        fs_err::copy(
            fixture("fixable").join("expected/fixed.composer.json"),
            scratch.path().join("composer.json"),
        )
        .unwrap();
        Command::new(cargo_bin("viv"))
            .current_dir(scratch.path())
            .arg("normalize")
            .output()
            .expect("failed to run viv normalize");
        fs_err::read_to_string(scratch.path().join("composer.json")).unwrap()
    };
    assert_eq!(fixed_json, normalized_expected);

    // A second run sees only the unfixable finding, with the lock now
    // fresh (no "# Lock file errors" section) and no count line (nothing
    // left `--fix` could resolve).
    let second = Command::new(cargo_bin("viv"))
        .current_dir(dir.path())
        .arg("validate")
        .output()
        .expect("failed to run viv");
    assert_eq!(second.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&second.stderr),
        "./composer.json is valid for simple usage with Composer but has\n\
         strict errors that make it unable to be published as a package\n\
         See https://getcomposer.org/doc/04-schema.md for details on the schema\n\
         # Publish errors\n\
         - description : The property description is required\n"
    );
}
