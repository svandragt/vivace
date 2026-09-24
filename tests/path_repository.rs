//! #305: a top-level `"type": "path"` repository (`vivace::path_repo`) —
//! discovering one package per matched directory the same way #294's
//! `"package"` repository (`tests/package_repository.rs`) declares one
//! inline. The lock byte-diff below is built the way `tests/vcs.rs`'s own
//! generic-driver test is, not a committed golden: a path package's
//! `dist.reference` for a directory with its own `.git` is that checkout's
//! current commit, freshly minted (and so different) on every run, so the
//! fixture is built and Composer is shelled out to at test time instead
//! (skipped with a message when `composer`/`git` isn't on `PATH`).
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::path::Path;

use reqwest::Url;
use serde_json::{Value, json};
use vivace::fetch::Conditional;
use vivace::repository::{DevAcceptance, Repository, Transport};

fn write_json(path: &Path, value: &Value) {
    fs_err::create_dir_all(path.parent().unwrap()).unwrap();
    fs_err::write(path, serde_json::to_vec_pretty(value).unwrap()).unwrap();
}

/// `packages/foo` (an explicit `version`, no `.git` of its own — its
/// `dist.reference` is the sha1 fallback `PathRepository::initialize`
/// computes for every match) and `packages/bar` (no `version` at all, its
/// own git checkout on `main` — `VersionGuesser` guesses `dev-main`, and
/// `dist.reference` becomes that commit's own hash instead of the sha1,
/// matching `initialize`'s `is_dir($path/.git)` override).
fn build_fixture(root: &Path) {
    write_json(
        &root.join("packages/foo/composer.json"),
        &json!({"name": "acme/foo", "version": "1.2.3", "type": "library"}),
    );
    write_json(
        &root.join("packages/bar/composer.json"),
        &json!({"name": "acme/bar", "type": "library"}),
    );
    let bar = root.join("packages/bar");
    let git = |args: &[&str]| {
        let status = common::git_command()
            .args(args)
            .current_dir(&bar)
            .status()
            .unwrap_or_else(|err| panic!("running git {args:?}: {err}"));
        assert!(status.success(), "git {args:?} failed in {}", bar.display());
    };
    git(&["init", "--initial-branch=main", "-q"]);
    git(&["config", "user.email", "path-repo-test@example.com"]);
    git(&["config", "user.name", "path repo test"]);
    // Hermetic regardless of the host's global gitconfig (e.g. a
    // `commit.gpgsign` that would otherwise try to sign this fixture's
    // commit).
    git(&["config", "commit.gpgsign", "false"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "init"]);
}

fn root_json() -> Value {
    json!({
        "name": "vivace/fixture-path-repository",
        "repositories": [
            {"packagist.org": false},
            {"type": "path", "url": "packages/*"},
        ],
        "require": {"acme/foo": "1.2.3", "acme/bar": "dev-main"},
    })
}

/// A path-only repository set never needs HTTP at all (`packagist.org` is
/// disabled and `path_repo::discover` reads the filesystem directly); this
/// panics on any call so a regression that starts making one fails loudly
/// rather than silently trying (and failing) to reach the network.
struct PanicTransport;

impl Transport for &PanicTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &Url,
        _if_modified_since: Option<&str>,
    ) -> anyhow::Result<Conditional> {
        panic!("unexpected HTTP request to {url}: a path-only repository set should never need it");
    }
}

#[tokio::test]
async fn path_repository_update_matches_composer_byte_for_byte() {
    if common::scrubbed_command("composer")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!(
            "skipping path_repository_update_matches_composer_byte_for_byte: composer is not \
             on PATH"
        );
        return;
    }
    if common::git_command().arg("--version").output().is_err() {
        eprintln!(
            "skipping path_repository_update_matches_composer_byte_for_byte: git is not on PATH"
        );
        return;
    }

    let project_dir = tempfile::tempdir().unwrap();
    build_fixture(project_dir.path());
    let root = root_json();
    let composer_json = serde_json::to_vec_pretty(&root).unwrap();
    fs_err::write(project_dir.path().join("composer.json"), &composer_json).unwrap();

    let composer_home = tempfile::tempdir().unwrap();
    let status = common::scrubbed_command("composer")
        .args(["update", "--no-install", "--no-plugins"])
        .current_dir(project_dir.path())
        .env("COMPOSER_HOME", composer_home.path())
        .env("COMPOSER_CACHE_DIR", composer_home.path().join("cache"))
        .status()
        .unwrap();
    assert!(status.success(), "composer update failed");
    let want = fs_err::read_to_string(project_dir.path().join("composer.lock")).unwrap();

    let cache = tempfile::tempdir().unwrap();
    let transport = PanicTransport;
    let repo = Repository::from_composer_json(project_dir.path(), &root, cache.path(), &transport)
        .await
        .unwrap();
    let result = vivace::solver::solve_update(&repo, &root, false, false)
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

    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}

#[tokio::test]
async fn options_symlink_false_lands_in_transport_options_as_composer_writes_it() {
    let project_dir = tempfile::tempdir().unwrap();
    write_json(
        &project_dir.path().join("packages/foo/composer.json"),
        &json!({"name": "acme/foo", "version": "1.2.3", "type": "library"}),
    );
    let root = json!({
        "repositories": [
            {"packagist.org": false},
            {"type": "path", "url": "packages/foo", "options": {"symlink": false}},
        ],
    });

    let cache = tempfile::tempdir().unwrap();
    let transport = PanicTransport;
    let repo = Repository::from_composer_json(project_dir.path(), &root, cache.path(), &transport)
        .await
        .unwrap();
    let versions = repo
        .load_package("acme/foo", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert_eq!(versions.len(), 1, "{versions:?}");
    let raw = versions[0].raw();
    assert_eq!(
        raw["transport-options"],
        json!({"symlink": false, "relative": true})
    );
    assert_eq!(raw["dist"]["type"], "path");
    assert_eq!(raw["dist"]["url"], "packages/foo");
}

#[tokio::test]
async fn a_plain_url_with_no_composer_json_fails_with_composers_wording() {
    let project_dir = tempfile::tempdir().unwrap();
    let root = json!({
        "repositories": [
            {"packagist.org": false},
            {"type": "path", "url": "missing"},
        ],
    });

    let cache = tempfile::tempdir().unwrap();
    let transport = PanicTransport;
    let err =
        match Repository::from_composer_json(project_dir.path(), &root, cache.path(), &transport)
            .await
        {
            Ok(_) => panic!("expected the missing path repository to error"),
            Err(err) => err.to_string(),
        };
    assert_eq!(
        err,
        "The `url` supplied for the path (missing) repository does not exist"
    );
}

/// #13's own `install_path_defaults_to_a_relative_symlink`/
/// `install_path_mirrors_when_symlink_is_false` (`src/source.rs`), but
/// fed by a lock `viv update` itself wrote from a discovered `"path"`
/// repository rather than a hand-written one — the round trip #305 adds.
#[test]
fn path_repository_install_links_by_default_and_copies_when_symlink_is_false() {
    let ctx = common::TestContext::new();
    let project = ctx.project.path();

    write_json(
        &project.join("packages/default-link/composer.json"),
        &json!({"name": "acme/default-link", "version": "1.0.0", "type": "library"}),
    );
    write_json(
        &project.join("packages/copied/composer.json"),
        &json!({"name": "acme/copied", "version": "1.0.0", "type": "library"}),
    );
    fs_err::write(project.join("packages/copied/marker.txt"), "hello").unwrap();
    write_json(
        &project.join("composer.json"),
        &json!({
            "repositories": [
                {"packagist.org": false},
                {"type": "path", "url": "packages/default-link"},
                {"type": "path", "url": "packages/copied", "options": {"symlink": false}},
            ],
            "require": {
                "acme/default-link": "1.0.0",
                "acme/copied": "1.0.0",
            },
        }),
    );

    ctx.viv()
        .args(["update", "--no-install"])
        .assert()
        .success();
    ctx.viv().arg("install").assert().success();

    let linked = project.join("vendor/acme/default-link");
    assert!(
        fs_err::symlink_metadata(&linked).unwrap().is_symlink(),
        "default `symlink` option should leave a symlink"
    );
    assert_eq!(
        fs_err::read_link(&linked).unwrap(),
        Path::new("../../packages/default-link")
    );

    let copied = project.join("vendor/acme/copied");
    assert!(
        !fs_err::symlink_metadata(&copied).unwrap().is_symlink(),
        "`options.symlink: false` should mirror a real copy, not a symlink"
    );
    assert_eq!(
        fs_err::read_to_string(copied.join("marker.txt")).unwrap(),
        "hello"
    );
}
