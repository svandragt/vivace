//! Resolver stage 5 (`docs/resolver-design.md`, composer/composer#42):
//! `viv require`'s constraint synthesis and format-preserving
//! `composer.json` edit, byte-diffed against a real Composer run.
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

use common::TestContext;
use serde_json::Value;
use vivace::repository::{Repository, Transport};
use vivace::solver::{self, pool_builder::UpdateAllowMode};

const FIXED_LAST_MODIFIED: &str = "Mon, 01 Jan 2024 00:00:00 GMT";

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/repo.packagist.org")
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/require-psr-container")
}

/// Same fixture-replaying transport as `tests/update.rs`/`tests/solver.rs`.
struct FixtureTransport {
    root: PathBuf,
}

impl Transport for &FixtureTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &reqwest::Url,
        if_modified_since: Option<&str>,
    ) -> anyhow::Result<vivace::fetch::Conditional> {
        if if_modified_since == Some(FIXED_LAST_MODIFIED) {
            return Ok(vivace::fetch::Conditional::NotModified);
        }
        let path = self.root.join(url.path().trim_start_matches('/'));
        match fs_err::read(&path) {
            Ok(body) => Ok(vivace::fetch::Conditional::Fresh {
                body,
                last_modified: Some(FIXED_LAST_MODIFIED.to_string()),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(vivace::fetch::Conditional::NotFound)
            }
            Err(err) => Err(err.into()),
        }
    }
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

    let lock_before: Value = serde_json::from_slice(
        &fs_err::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut locked_by_name = std::collections::HashMap::new();
    for key in ["packages", "packages-dev"] {
        for entry in lock_before[key].as_array().unwrap() {
            locked_by_name.insert(
                entry["name"].as_str().unwrap().to_ascii_lowercase(),
                entry.clone(),
            );
        }
    }

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

    let lock_before: Value = serde_json::from_slice(
        &fs_err::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut locked_by_name = std::collections::HashMap::new();
    for key in ["packages", "packages-dev"] {
        for entry in lock_before[key].as_array().unwrap() {
            locked_by_name.insert(
                entry["name"].as_str().unwrap().to_ascii_lowercase(),
                entry.clone(),
            );
        }
    }

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

// The `JsonManipulator` port's own byte-diff against this same recorded
// `composer.json.before`/`composer.json.after` pair lives in
// `src/require.rs`'s `#[cfg(test)]` module: `Manipulator` is private to
// that file (mirrors `Json/JsonManipulator.php` not being part of any
// public API either), so an external integration test can't drive it
// directly.

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

    // Composer's own `JsonManipulator` output is what `composer.json.after`
    // records; `viv require` reproduces it byte for byte only with
    // `--no-normalize` (#95: without it, `viv require` also normalizes the
    // file after writing it, which this fixture's formatting doesn't
    // already match).
    ctx.viv()
        .args(["require", "psr/container", "--no-normalize"])
        .assert()
        .success();

    let got_json = fs_err::read_to_string(project.join("composer.json")).unwrap();
    let want_json = fs_err::read_to_string(fixture.join("composer.json.after")).unwrap();
    assert_eq!(
        got_json, want_json,
        "viv require's composer.json edit differs"
    );

    let got_lock = fs_err::read_to_string(project.join("composer.lock")).unwrap();
    let want_lock = fs_err::read_to_string(fixture.join("composer.lock")).unwrap();
    assert_eq!(got_lock, want_lock, "viv require's lock differs");

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

    // Without `--no-normalize`, the same edit followed by normalizing must
    // equal `viv normalize`'s own output on Composer's `composer.json.after`.
    let normalized_ctx = TestContext::new();
    let normalized_project = normalized_ctx.project.path();
    fs_err::copy(
        fixture.join("composer.json.before"),
        normalized_project.join("composer.json"),
    )
    .unwrap();
    fs_err::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
        normalized_project.join("composer.lock"),
    )
    .unwrap();
    normalized_ctx
        .viv()
        .args(["require", "psr/container"])
        .assert()
        .success();
    let got_normalized = fs_err::read_to_string(normalized_project.join("composer.json")).unwrap();

    let want_normalized_ctx = TestContext::new();
    let want_normalized_path = want_normalized_ctx.project.path().join("composer.json");
    fs_err::write(&want_normalized_path, &want_json).unwrap();
    want_normalized_ctx
        .viv()
        .arg("normalize")
        .assert()
        .success();
    let want_normalized = fs_err::read_to_string(&want_normalized_path).unwrap();

    assert_eq!(
        got_normalized, want_normalized,
        "viv require without --no-normalize should match `viv normalize`'s output"
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

    // See `viv_require_matches_composer_and_validates`'s comment: byte-exact
    // parity with Composer's own edit needs `--no-normalize`.
    ctx.viv()
        .args(["remove", "psr/container", "--dev", "--no-normalize"])
        .assert()
        .success();

    let got_json = fs_err::read_to_string(project.join("composer.json")).unwrap();
    let want_json = fs_err::read_to_string(fixture.join("composer.json.after")).unwrap();
    assert_eq!(
        got_json, want_json,
        "viv remove's composer.json edit differs"
    );

    let got_lock = fs_err::read_to_string(project.join("composer.lock")).unwrap();
    let want_lock = fs_err::read_to_string(fixture.join("composer.lock")).unwrap();
    assert_eq!(got_lock, want_lock, "viv remove's lock differs");

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

    // Without `--no-normalize`, the same edit followed by normalizing must
    // equal `viv normalize`'s own output on Composer's `composer.json.after`.
    let normalized_ctx = TestContext::new();
    let normalized_project = normalized_ctx.project.path();
    fs_err::copy(
        fixture.join("composer.json.before"),
        normalized_project.join("composer.json"),
    )
    .unwrap();
    fs_err::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
        normalized_project.join("composer.lock"),
    )
    .unwrap();
    normalized_ctx
        .viv()
        .args(["remove", "psr/container", "--dev"])
        .assert()
        .success();
    let got_normalized = fs_err::read_to_string(normalized_project.join("composer.json")).unwrap();

    let want_normalized_ctx = TestContext::new();
    let want_normalized_path = want_normalized_ctx.project.path().join("composer.json");
    fs_err::write(&want_normalized_path, &want_json).unwrap();
    want_normalized_ctx
        .viv()
        .arg("normalize")
        .assert()
        .success();
    let want_normalized = fs_err::read_to_string(&want_normalized_path).unwrap();

    assert_eq!(
        got_normalized, want_normalized,
        "viv remove without --no-normalize should match `viv normalize`'s output"
    );
}
