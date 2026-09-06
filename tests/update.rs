//! Resolver stage 4 (`docs/resolver-design.md`): `viv update`'s lock writer,
//! byte-diffed against Composer 2.10.2's own `composer.lock` for the
//! monolog fixture (recorded Packagist metadata, hermetic and offline —
//! same `FixtureTransport` shape as `tests/solver.rs`). The
//! network/php-gated half (`composer validate`/`install --dry-run`/`update
//! --lock`) runs the real `viv` binary against real Packagist, like
//! `tests/install_e2e.rs`.
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
use vivace::solver;

const FIXED_LAST_MODIFIED: &str = "Mon, 01 Jan 2024 00:00:00 GMT";

/// Same recursive copy as `tests/install_e2e.rs`'s helper of the same name:
/// the monolog fixture's `src`/`lib` autoload sources, needed for `composer
/// install` to generate an autoloader without erroring on a missing
/// directory.
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

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/repo.packagist.org")
}

/// Same fixture-replaying transport as `tests/solver.rs`/`tests/repository.rs`.
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

/// Solves `fixture`'s `composer.json` against the recorded Packagist
/// fixtures and writes the lock exactly like `viv update` would, without
/// going through the CLI (which always hits the real network) or the
/// filesystem.
async fn update_lock(fixture: &Path) -> String {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let result = solver::solve_update(&repo, &root, false, false)
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
    vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
        .unwrap()
}

fn assert_matches_expected(got: &str, expected_path: &Path) {
    let want = fs_err::read_to_string(expected_path).unwrap();
    if got == want {
        return;
    }
    let diff = similar_unified_diff(&want, got);
    panic!(
        "{} does not match viv update's output:\n{diff}",
        expected_path.display()
    );
}

/// A minimal unified-ish line diff: no external crate, just enough context
/// to see what changed when a golden test goes red.
fn similar_unified_diff(want: &str, got: &str) -> String {
    let want_lines: Vec<&str> = want.lines().collect();
    let got_lines: Vec<&str> = got.lines().collect();
    let mut out = String::new();
    let max = want_lines.len().max(got_lines.len());
    for i in 0..max {
        let w = want_lines.get(i).copied();
        let g = got_lines.get(i).copied();
        if w != g {
            use std::fmt::Write as _;
            if let Some(w) = w {
                let _ = writeln!(out, "-{i:>4}: {w}");
            }
            if let Some(g) = g {
                let _ = writeln!(out, "+{i:>4}: {g}");
            }
        }
    }
    out
}

#[tokio::test]
async fn update_reproduces_the_monolog_lock() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");
    let got = update_lock(&fixture).await;
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

#[tokio::test]
async fn update_reproduces_the_legacy_lock() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/legacy");
    let got = update_lock(&fixture).await;
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// End-to-end: the real `viv update` binary against real Packagist,
/// byte-diffed the same way, then validated with Composer itself.
/// Gated on `VIVACE_TEST_NETWORK=1` so a bare `cargo nextest run` stays
/// offline, like `tests/install_e2e.rs`.
#[test]
fn viv_update_matches_composer_and_validates() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv_update_matches_composer_and_validates: set VIVACE_TEST_NETWORK=1 to \
             resolve against real Packagist"
        );
        return;
    }
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!("skipping viv_update_matches_composer_and_validates: composer is not on PATH");
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs_err::copy(fixture.join("composer.json"), project.join("composer.json")).unwrap();
    for dir in ["src", "lib"] {
        copy_tree(&fixture.join(dir), &project.join(dir));
    }

    ctx.viv().arg("update").assert().success();

    let got = fs_err::read_to_string(project.join("composer.lock")).unwrap();
    let want = fs_err::read_to_string(fixture.join("composer.lock")).unwrap();
    assert_eq!(
        got, want,
        "viv update's lock differs from the committed one"
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

    // `install --dry-run` only reports "nothing to do" once something is
    // actually installed; real Composer, not `viv install`, so this is
    // Composer's own dist fetch (network, already gated above).
    let install = Command::new("composer")
        .args(["install", "--no-interaction"])
        .current_dir(project)
        .status()
        .unwrap();
    assert!(install.success(), "composer install failed");

    let dry_run = Command::new("composer")
        .args(["install", "--dry-run"])
        .current_dir(project)
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&dry_run.stdout),
        String::from_utf8_lossy(&dry_run.stderr)
    );
    assert!(
        dry_run.status.success() && combined.contains("Nothing to install, update or remove"),
        "composer install --dry-run reported changes: {combined}"
    );

    let before = fs_err::read(project.join("composer.lock")).unwrap();
    Command::new("composer")
        .args(["update", "--lock"])
        .current_dir(project)
        .status()
        .map(|s| assert!(s.success(), "composer update --lock failed"))
        .unwrap();
    let after = fs_err::read(project.join("composer.lock")).unwrap();
    assert_eq!(
        before, after,
        "composer update --lock changed the lock viv update wrote"
    );
}
