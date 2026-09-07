//! `viv show`/`viv tree` byte-parity checks against Composer 2.10.2's own
//! output on `tests/fixtures/monolog`, recorded with:
//!
//! ```sh
//! devbox run -- composer -d tests/fixtures/monolog show --no-ansi [flags]
//! ```
//!
//! `tests/fixtures/show/expected/monolog-detail.txt` is the full recorded
//! output, including `released`/`license`/`suggests`/`provides` (#94):
//! `released`'s relative age is pinned with `VIV_TEST_NOW` so the fixture
//! never drifts out from under a later test run.
//! `viv outdated`'s own byte-parity check
//! (`find_latest_matches_composers_recorded_outdated` et al.) lives as a
//! unit test inside `src/show.rs` instead, since it needs a fixture
//! `Transport`, not a real `viv` binary (`show.rs`'s module doc, `#69`).

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog")
}

fn expected(name: &str) -> String {
    fs_err::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/show/expected")
            .join(name),
    )
    .unwrap()
}

/// A project as `viv show`/`viv tree` see it: `composer.json`,
/// `composer.lock`, and just `vendor/composer/installed.json` — the only
/// file any of `show.rs`'s views ever read from `vendor/`. Built from the
/// committed `expected/dev` snapshot, not `tests/fixtures/monolog/vendor`
/// itself: that's gitignored (`tests/cli.rs`'s own `fixture_vendor_missing`
/// note) and absent on a fresh checkout or in CI, unlike `expected/dev`.
///
/// The detail view's `path` line still needs each package's own directory
/// to exist (`realpath` on a missing one is Composer's `null` fallback,
/// not the real path this test asserts), but never reads a file inside
/// it, so an empty directory is enough — no package file to fetch or fake.
fn project_with_installed_json(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture().join(name), project.join(name)).unwrap();
    }
    fs::create_dir_all(project.join("vendor/composer")).unwrap();
    fs::copy(
        fixture().join("expected/dev/composer/installed.json"),
        project.join("vendor/composer/installed.json"),
    )
    .unwrap();
    for pkg in ["monolog/monolog", "psr/container", "psr/log"] {
        fs::create_dir_all(project.join("vendor").join(pkg)).unwrap();
    }
}

fn stdout_of(ctx: &TestContext, args: &[&str]) -> String {
    let mut cmd = ctx.viv();
    cmd.args(args);
    let output = cmd.output().expect("failed to run viv");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

#[test]
fn show_list_matches_composer() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(stdout_of(&ctx, &["show"]), expected("monolog-list.txt"));
}

#[test]
fn show_name_only_matches_composer() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(
        stdout_of(&ctx, &["show", "--name-only"]),
        expected("monolog-name-only.txt")
    );
}

#[test]
fn show_direct_matches_composer() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(
        stdout_of(&ctx, &["show", "--direct"]),
        expected("monolog-direct.txt")
    );
}

#[test]
fn show_no_dev_matches_composer() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(
        stdout_of(&ctx, &["show", "--no-dev"]),
        expected("monolog-no-dev.txt")
    );
}

#[test]
fn show_locked_matches_composer() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(
        stdout_of(&ctx, &["show", "--locked"]),
        expected("monolog-locked.txt")
    );
}

#[test]
fn show_format_json_matches_composer() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(
        stdout_of(&ctx, &["show", "--format=json"]),
        expected("monolog-list.json")
    );
}

#[test]
fn show_tree_matches_composer() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(
        stdout_of(&ctx, &["show", "-t"]),
        expected("monolog-tree.txt")
    );
}

/// `viv tree` (#86's spelling) must render the exact same tree as
/// `viv show -t`.
#[test]
fn tree_command_matches_show_tree() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(stdout_of(&ctx, &["tree"]), expected("monolog-tree.txt"));
}

/// `released`'s relative age is pinned two years past the fixture's
/// recorded `time` (2026-09-02) via `VIV_TEST_NOW`, matching
/// `monolog-detail.txt`'s "2 years ago".
#[test]
fn show_detail_matches_composer() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    // Composer prints the realpath, so canonicalise like macOS's /var symlink
    // forces viv to.
    let install_path = std::fs::canonicalize(ctx.project.path().join("vendor/monolog/monolog"))
        .expect("fixture package dir exists");
    let want =
        expected("monolog-detail.txt").replace("[PATH]", &install_path.display().to_string());
    let mut cmd = ctx.viv();
    cmd.env("VIV_TEST_NOW", "2028-09-02T00:00:00+00:00")
        .args(["show", "monolog/monolog"]);
    let output = cmd.output().expect("failed to run viv");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&output.stdout), want);
}

/// `viv tree --invert psr/log` matches Composer's own `composer why
/// psr/log --no-ansi` (recorded on a checkout with no VCS, so the root
/// package's version is Composer's own deterministic `-` fallback rather
/// than a git-branch-derived one).
#[test]
fn tree_invert_matches_composer_why() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(
        stdout_of(&ctx, &["tree", "--invert", "psr/log"]),
        expected("monolog-why-psr-log.txt")
    );
}

/// `viv why` (#86's `composer why`/`depends` alias) renders the exact same
/// output as `viv tree --invert`.
#[test]
fn why_command_matches_tree_invert() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    assert_eq!(
        stdout_of(&ctx, &["why", "psr/log"]),
        expected("monolog-why-psr-log.txt")
    );
}

#[test]
fn why_unknown_package_errors() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    let mut cmd = ctx.viv();
    cmd.args(["why", "acme/does-not-exist"]);
    cmd.assert().failure();
}

#[test]
fn show_unknown_package_errors() {
    let ctx = TestContext::new();
    project_with_installed_json(ctx.project.path());
    let mut cmd = ctx.viv();
    cmd.args(["show", "acme/does-not-exist"]);
    cmd.assert().failure();
}
