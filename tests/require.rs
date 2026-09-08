//! Resolver stage 5 (`docs/resolver-design.md`, composer/composer#42):
//! `viv require`'s constraint synthesis and `composer.json` edit,
//! byte-diffed against a real Composer run (through `viv normalize`, since
//! #145: `add`/`rm` always normalize now).
//!
//! `tests/fixtures/require-psr-container/composer.lock` and
//! `composer.json.after` were recorded by running real Composer 2.10.2
//! against `tests/fixtures/packagist/repo.packagist.org` as a local
//! `file://` `composer`-type repository (`composer.json.before`'s
//! `repositories` block), not live Packagist: this keeps the recording
//! reproducible from the same fixtures `tests/update.rs` already commits,
//! with no extra network fixtures to maintain and no drift risk. The
//! command was:
//!
//! ```sh
//! composer -d <scratch> require psr/container --no-interaction --no-plugins --no-scripts --no-install
//! ```
//!
//! The hermetic tests below never invoke Composer or the network at all:
//! they drive `vivace::solver`/`vivace::lock_writer` directly (same shape
//! as `tests/update.rs`'s `update_lock`), replaying the same recorded
//! Packagist metadata through `FixtureTransport`.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{FixtureTransport, TestContext, fixtures_root};
use serde_json::{Map, Value};
use vivace::repository::Repository;
use vivace::solver::{self, pool_builder::UpdateAllowMode};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/require-psr-container")
}

/// `composer.lock`'s `content-hash` is `content_hash` of `composer.json`
/// (`src/lock.rs`), which includes the `require`/`require-dev` objects'
/// *own* key order. #145: `add`/`rm` now always normalize `composer.json`
/// (sorting those sections), so the on-disk file — and its content-hash —
/// no longer matches Composer's own unnormalized `JsonManipulator` edit
/// that `composer.lock` fixtures here were recorded against. Every other
/// field is still expected to match byte for byte.
fn lock_without_content_hash(path: &Path) -> Value {
    let mut lock: Value = serde_json::from_slice(&fs_err::read(path).unwrap()).unwrap();
    if let Some(obj) = lock.as_object_mut() {
        obj.remove("content-hash");
    }
    lock
}

fn assert_matches_expected(got: &str, expected_path: &Path) {
    let want = fs_err::read_to_string(expected_path).unwrap();
    assert_eq!(
        got,
        want,
        "{} does not match viv require's output",
        expected_path.display()
    );
}

/// `viv require psr/container` on the monolog fixture (moved out of
/// `require-dev`, since the fixture already declares it there, matching
/// real Composer's own "present in the other key" move): the partial
/// update this stage adds must reproduce Composer's real `composer.lock`
/// byte for byte, given the already-edited `composer.json`.
#[tokio::test]
async fn require_reproduces_composers_lock() {
    let fixture = fixture_dir();
    let composer_json = fs_err::read(fixture.join("composer.json.after")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let locked_by_name = common::locked_by_name(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
    );

    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let result = solver::solve_partial_update(
        &repo,
        &root,
        false,
        false,
        &locked_by_name,
        &["psr/container".to_string()],
        UpdateAllowMode::OnlyListed,
    )
    .await
    .unwrap();

    let options = vivace::lock_writer::LockOptions {
        minimum_stability: result.minimum_stability,
        stability_flags: &result.stability_flags,
        prefer_stable: result.prefer_stable,
        prefer_lowest: result.prefer_lowest,
        platform_reqs: &result.platform_reqs,
        platform_dev_reqs: &result.platform_dev_reqs,
        platform_overrides: &result.platform_overrides,
        aliases: &result.aliases,
    };
    let got =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// `viv remove psr/container --dev` on the monolog fixture: recorded the
/// same way as `require_reproduces_composers_lock`
/// (`tests/fixtures/remove-psr-container`), `composer remove ... --dev`
/// against the local `file://` mirror.
#[tokio::test]
async fn remove_reproduces_composers_lock() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/remove-psr-container");
    let composer_json = fs_err::read(fixture.join("composer.json.after")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let locked_by_name = common::locked_by_name(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
    );

    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let result = solver::solve_partial_update(
        &repo,
        &root,
        false,
        false,
        &locked_by_name,
        &["psr/container".to_string()],
        UpdateAllowMode::WithTransitiveDepsNoRootRequire,
    )
    .await
    .unwrap();

    let options = vivace::lock_writer::LockOptions {
        minimum_stability: result.minimum_stability,
        stability_flags: &result.stability_flags,
        prefer_stable: result.prefer_stable,
        prefer_lowest: result.prefer_lowest,
        platform_reqs: &result.platform_reqs,
        platform_dev_reqs: &result.platform_dev_reqs,
        platform_overrides: &result.platform_overrides,
        aliases: &result.aliases,
    };
    let got =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// End-to-end: the real `viv require` binary, then Composer's own
/// `validate`/`install --dry-run`. Gated on `VIVACE_TEST_NETWORK=1`
/// (`tests/update.rs`'s own pattern) so a bare `cargo nextest run` stays
/// offline; unlike that test, this one still resolves against the local
/// `file://` mirror (`composer.json.before`'s `repositories` override, see
/// the module doc) rather than live Packagist, so it is not actually
/// network-dependent — only gated the same way for consistency and because
/// it shells out to the real `composer`/`viv` binaries rather than calling
/// the library directly.
#[test]
fn viv_require_matches_composer_and_validates() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv_require_matches_composer_and_validates: set VIVACE_TEST_NETWORK=1 to run"
        );
        return;
    }
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!("skipping viv_require_matches_composer_and_validates: composer is not on PATH");
        return;
    }

    let fixture = fixture_dir();
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs_err::copy(
        fixture.join("composer.json.before"),
        project.join("composer.json"),
    )
    .unwrap();
    fs_err::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
        project.join("composer.lock"),
    )
    .unwrap();

    // #145: `require`/`add` always normalize `composer.json` now
    // (`--no-normalize` is a deprecated no-op, same shape as
    // `install`/`dump-autoload`'s), so the written file is compared against
    // `viv normalize`'s own output of Composer's recorded edit
    // (`composer.json.after`), not Composer's own unnormalized
    // `JsonManipulator` formatting. `--no-install` matches the recorded
    // command itself (see the module doc) and keeps this test scoped to the
    // `composer.json`/lock edit, not a real dist fetch (#104: `viv require`
    // installs by default now).
    ctx.viv()
        .args(["require", "psr/container", "--no-install"])
        .assert()
        .success();

    let got_json = fs_err::read_to_string(project.join("composer.json")).unwrap();

    let want_normalized_ctx = TestContext::new();
    let want_normalized_path = want_normalized_ctx.project.path().join("composer.json");
    fs_err::copy(fixture.join("composer.json.after"), &want_normalized_path).unwrap();
    want_normalized_ctx
        .viv()
        .arg("normalize")
        .assert()
        .success();
    let want_normalized = fs_err::read_to_string(&want_normalized_path).unwrap();
    assert_eq!(
        got_json, want_normalized,
        "viv require's composer.json edit differs"
    );

    assert_eq!(
        lock_without_content_hash(&project.join("composer.lock")),
        lock_without_content_hash(&fixture.join("composer.lock")),
        "viv require's lock differs"
    );

    let validate = Command::new("composer")
        .args(["validate", "--strict", "--no-check-publish"])
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "composer validate --strict failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stderr)
    );
}

/// End-to-end: the real `viv remove` binary, same shape as
/// `viv_require_matches_composer_and_validates`.
#[test]
fn viv_remove_matches_composer_and_validates() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv_remove_matches_composer_and_validates: set VIVACE_TEST_NETWORK=1 to run"
        );
        return;
    }
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!("skipping viv_remove_matches_composer_and_validates: composer is not on PATH");
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/remove-psr-container");
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs_err::copy(
        fixture.join("composer.json.before"),
        project.join("composer.json"),
    )
    .unwrap();
    fs_err::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
        project.join("composer.lock"),
    )
    .unwrap();

    // See `viv_require_matches_composer_and_validates`'s comment: #145
    // means `remove`/`rm` always normalize now, so the written file is
    // compared against `viv normalize`'s own output of Composer's recorded
    // edit. `--no-install` keeps this scoped to the edit rather than a real
    // dist fetch (#104: `viv remove` installs by default now).
    ctx.viv()
        .args(["remove", "psr/container", "--dev", "--no-install"])
        .assert()
        .success();

    let got_json = fs_err::read_to_string(project.join("composer.json")).unwrap();

    let want_normalized_ctx = TestContext::new();
    let want_normalized_path = want_normalized_ctx.project.path().join("composer.json");
    fs_err::copy(fixture.join("composer.json.after"), &want_normalized_path).unwrap();
    want_normalized_ctx
        .viv()
        .arg("normalize")
        .assert()
        .success();
    let want_normalized = fs_err::read_to_string(&want_normalized_path).unwrap();
    assert_eq!(
        got_json, want_normalized,
        "viv remove's composer.json edit differs"
    );

    assert_eq!(
        lock_without_content_hash(&project.join("composer.lock")),
        lock_without_content_hash(&fixture.join("composer.lock")),
        "viv remove's lock differs"
    );

    let validate = Command::new("composer")
        .args(["validate", "--strict", "--no-check-publish"])
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "composer validate --strict failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stderr)
    );
}

/// Same recursive copy as `tests/update.rs`'s helper of the same name:
/// the monolog fixture's `src`/`lib` autoload sources.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// #155: `viv add`'s written `composer.lock` must have a `content-hash`
/// describing the `composer.json` bytes actually left on disk, not the
/// unnormalised bytes read before `write_composer_json`/`maybe_normalize`
/// ran. A deliberately unnormalised `require` (reverse key order, so the
/// normaliser's platform-first sort actually moves something) must still
/// leave a fresh lock behind, so the chained install prints no stale-lock
/// warning. Real network (`require.rs`'s `partial_update` always resolves
/// against live Packagist, unlike `update.rs`'s `solve`, so this can't run
/// offline against the recorded fixtures the way `tests/update.rs`'s own
/// #155 regression does).
#[test]
fn viv_add_normalizes_composer_json_before_computing_the_content_hash() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv_add_normalizes_composer_json_before_computing_the_content_hash: set \
             VIVACE_TEST_NETWORK=1 to resolve against real Packagist"
        );
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");
    let ctx = TestContext::new();
    let project = ctx.project.path();

    let original: Value =
        serde_json::from_slice(&fs_err::read(fixture.join("composer.json")).unwrap()).unwrap();
    let mut require_reversed = Map::new();
    for (name, constraint) in original["require"].as_object().unwrap().iter().rev() {
        require_reversed.insert(name.clone(), constraint.clone());
    }
    let unnormalized = serde_json::json!({
        "require": require_reversed,
        "name": original["name"],
        "license": original["license"],
        "type": original["type"],
        "require-dev": original["require-dev"],
        "autoload": original["autoload"],
        "config": original["config"],
    });
    fs_err::write(
        project.join("composer.json"),
        serde_json::to_vec(&unnormalized).unwrap(),
    )
    .unwrap();
    for dir in ["src", "lib"] {
        copy_tree(&fixture.join(dir), &project.join(dir));
    }

    let output = ctx
        .viv()
        .args(["require", "psr/container"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "viv require failed:\n{combined}");
    assert!(
        !combined.contains("is not up to date"),
        "the chained install must not see a stale lock: {combined}"
    );

    let lock = vivace::lock::read_lock(&project.join("composer.lock")).unwrap();
    let on_disk_composer_json = fs_err::read(project.join("composer.json")).unwrap();
    assert!(
        vivace::lock::is_fresh(&lock, &on_disk_composer_json).unwrap(),
        "content-hash should describe the normalized composer.json actually on disk"
    );
}
