//! `#97`: `"vcs"`/`"git"`/`"github"` `repositories[]` entries
//! (`vivace::vcs`), hermetic against a locally built bare-adjacent git
//! fixture (the generic driver, `git` CLI) and recorded JSON fixtures under
//! `tests/fixtures/vcs/github/` (the GitHub driver, via the same
//! `Transport` fake as `tests/repository.rs`, no network). The generic
//! driver's byte-diff test additionally shells out to real Composer and
//! `git` — skipped with a message when either isn't on `PATH`, like
//! `tests/update.rs`'s own php-dependent test.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use reqwest::Url;
use serde_json::{Value, json};
use vivace::fetch::Conditional;
use vivace::repository::{DevAcceptance, Repository, Transport};

// ---------------------------------------------------------------------
// Driver A: a local git repository (`Vcs\GitDriver`).
// ---------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap_or_else(|err| panic!("running git {args:?}: {err}"));
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn write_composer_json(dir: &Path, value: &Value) {
    fs_err::write(
        dir.join("composer.json"),
        serde_json::to_vec_pretty(value).unwrap(),
    )
    .unwrap();
}

/// Two tags (`1.0.0`, `1.1.0`), a `main` branch (the checked-out default,
/// with a `branch-alias`) and a `feature` branch, same as the design
/// brief. `git`'s own commit dates become each version's `time`, so both
/// Composer and viv read the very same value from the very same commit —
/// nothing needs to be pinned for the byte-diff test to agree on `time`.
fn build_fixture_repo() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path();
    git(repo, &["init", "--initial-branch=main", "-q"]);
    git(repo, &["config", "user.email", "vcs-test@example.com"]);
    git(repo, &["config", "user.name", "vcs test"]);
    // Hermetic regardless of the host's global gitconfig (e.g. a
    // `tag.gpgsign`/`commit.gpgsign` that would otherwise try to sign
    // every commit/tag this fixture makes).
    git(repo, &["config", "commit.gpgsign", "false"]);
    git(repo, &["config", "tag.gpgsign", "false"]);

    write_composer_json(repo, &json!({"name": "acme/vcs-pkg"}));
    git(repo, &["add", "."]);
    git(repo, &["commit", "-q", "-m", "1.0.0"]);
    git(repo, &["tag", "1.0.0"]);

    write_composer_json(
        repo,
        &json!({"name": "acme/vcs-pkg", "require": {"php": ">=8.0"}}),
    );
    git(repo, &["commit", "-q", "-am", "1.1.0"]);
    git(repo, &["tag", "1.1.0"]);

    write_composer_json(
        repo,
        &json!({
            "name": "acme/vcs-pkg",
            "extra": {"branch-alias": {"dev-main": "2.0-dev"}},
        }),
    );
    git(repo, &["commit", "-q", "-am", "main tip"]);

    git(repo, &["checkout", "-q", "-b", "feature"]);
    write_composer_json(repo, &json!({"name": "acme/vcs-pkg"}));
    git(repo, &["commit", "-q", "-am", "feature branch"]);
    git(repo, &["checkout", "-q", "main"]);

    dir
}

fn vcs_root(url: &str) -> Value {
    json!({
        "name": "root/pkg",
        "require": {"acme/vcs-pkg": "^1.0"},
        "repositories": {
            "packagist.org": false,
            "acme-vcs": {"type": "vcs", "url": url},
        },
    })
}

/// A vcs-only repository set never needs HTTP at all (`packagist.org` is
/// disabled and the generic driver talks to `git` directly); this panics
/// on any call so a regression that starts making one fails loudly rather
/// than silently trying (and failing) to reach the network.
struct PanicTransport;

impl Transport for &PanicTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &Url,
        _if_modified_since: Option<&str>,
    ) -> anyhow::Result<Conditional> {
        panic!("unexpected HTTP request to {url}: a vcs-only repository set should never need it");
    }
}

#[tokio::test]
async fn vcs_repository_lists_tags_and_branches() {
    let repo_dir = build_fixture_repo();
    let url = format!("file://{}", repo_dir.path().display());
    let root = vcs_root(&url);

    let cache = tempfile::tempdir().unwrap();
    let transport = PanicTransport;
    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
        .await
        .unwrap();

    let versions = repo
        .load_package("acme/vcs-pkg", DevAcceptance::Both)
        .await
        .unwrap();
    let by_version: HashMap<&str, &vivace::repository::PackageVersion> =
        versions.iter().map(|v| (v.version.as_str(), v)).collect();
    assert_eq!(
        by_version
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        ["1.0.0", "1.1.0", "dev-main", "dev-feature"]
            .into_iter()
            .collect(),
        "{:?}",
        versions.iter().map(|v| &v.version).collect::<Vec<_>>()
    );

    let v100 = by_version["1.0.0"];
    assert_eq!(v100.name, "acme/vcs-pkg");
    assert_eq!(v100.version_normalized, "1.0.0.0");
    assert!(!v100.default_branch);
    assert!(v100.time.is_some(), "{v100:?}");
    let source = v100.source.as_ref().unwrap();
    assert_eq!(source["type"], "git");
    // `Filesystem::getPlatformPath` strips the `file://` scheme from a
    // local path before `VcsDriver` ever sees it, so a local repo's
    // `source.url` never has one.
    assert_eq!(source["url"], repo_dir.path().to_str().unwrap());
    assert!(
        source["reference"].as_str().unwrap().len() == 40,
        "{source:?}"
    );
    assert!(v100.dist.is_none());

    let v110 = by_version["1.1.0"];
    assert_eq!(v110.version_normalized, "1.1.0.0");
    assert_eq!(v110.require["php"].as_str(), Some(">=8.0"));

    let main = by_version["dev-main"];
    assert_eq!(main.version_normalized, "dev-main");
    assert!(main.default_branch, "{main:?}");
    assert!(main.branch_alias.is_some(), "{main:?}");

    let feature = by_version["dev-feature"];
    assert_eq!(feature.version_normalized, "dev-feature");
    assert!(!feature.default_branch);
}

/// Real Composer against the same fixture repository, byte-diffed against
/// `vivace::solver`/`vivace::lock_writer` run the same hermetic way
/// `tests/update.rs`'s `update_lock` does — no `viv` binary, no network
/// (the only two repositories in play are the disabled default and this
/// one local vcs repository).
#[tokio::test]
async fn vcs_update_matches_composer_byte_for_byte() {
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!("skipping vcs_update_matches_composer_byte_for_byte: composer is not on PATH");
        return;
    }
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skipping vcs_update_matches_composer_byte_for_byte: git is not on PATH");
        return;
    }

    let repo_dir = build_fixture_repo();
    let url = format!("file://{}", repo_dir.path().display());
    let root = vcs_root(&url);
    let composer_json = serde_json::to_vec_pretty(&root).unwrap();

    let project_dir = tempfile::tempdir().unwrap();
    fs_err::write(project_dir.path().join("composer.json"), &composer_json).unwrap();
    let composer_home = tempfile::tempdir().unwrap();

    let status = Command::new("composer")
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
    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
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

// ---------------------------------------------------------------------
// Driver B: GitHub, recorded fixtures under tests/fixtures/vcs/github/.
// ---------------------------------------------------------------------

fn github_fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/vcs/github")
}

/// Exact-URL match, unlike `tests/repository.rs`'s path-only
/// `FixtureTransport`: the GitHub API keys several different responses off
/// a query string (`?ref=<sha>`) on the very same path.
struct GitHubFixtureTransport {
    files: HashMap<String, PathBuf>,
}

impl Transport for &GitHubFixtureTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &Url,
        _if_modified_since: Option<&str>,
    ) -> anyhow::Result<Conditional> {
        match self.files.get(url.as_str()) {
            Some(path) => Ok(Conditional::Fresh {
                body: fs_err::read(path)?,
                last_modified: None,
            }),
            None => Ok(Conditional::NotFound),
        }
    }
}

const MAIN_SHA: &str = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
const TAG_SHA: &str = "b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3";

fn github_fixture_transport() -> GitHubFixtureTransport {
    let root = github_fixtures_root();
    let files = [
        (
            "https://api.github.com/repos/acme/widget-gh".to_string(),
            root.join("repo.json"),
        ),
        (
            "https://api.github.com/repos/acme/widget-gh/tags?per_page=100".to_string(),
            root.join("tags.json"),
        ),
        (
            "https://api.github.com/repos/acme/widget-gh/git/refs/heads?per_page=100".to_string(),
            root.join("branches.json"),
        ),
        (
            format!(
                "https://api.github.com/repos/acme/widget-gh/contents/composer.json?ref={MAIN_SHA}"
            ),
            root.join("contents-main.json"),
        ),
        (
            format!(
                "https://api.github.com/repos/acme/widget-gh/contents/composer.json?ref={TAG_SHA}"
            ),
            root.join("contents-1.0.0.json"),
        ),
        (
            format!("https://api.github.com/repos/acme/widget-gh/commits/{MAIN_SHA}"),
            root.join("commit-main.json"),
        ),
        (
            format!("https://api.github.com/repos/acme/widget-gh/commits/{TAG_SHA}"),
            root.join("commit-1.0.0.json"),
        ),
    ]
    .into_iter()
    .collect();
    GitHubFixtureTransport { files }
}

#[tokio::test]
async fn github_repository_lists_tags_and_the_default_branch() {
    let root = json!({
        "name": "root/pkg",
        "require": {"acme/widget-gh": "^1.0"},
        "repositories": {
            "packagist.org": false,
            "gh": {"type": "github", "url": "https://github.com/acme/widget-gh"},
        },
    });
    let cache = tempfile::tempdir().unwrap();
    let transport = github_fixture_transport();
    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
        .await
        .unwrap();

    let versions = repo
        .load_package("acme/widget-gh", DevAcceptance::Both)
        .await
        .unwrap();
    let by_version: HashMap<&str, &vivace::repository::PackageVersion> =
        versions.iter().map(|v| (v.version.as_str(), v)).collect();
    assert_eq!(
        by_version
            .keys()
            .copied()
            .collect::<std::collections::BTreeSet<_>>(),
        ["1.0.0", "dev-main"].into_iter().collect(),
        "{:?}",
        versions.iter().map(|v| &v.version).collect::<Vec<_>>()
    );

    let tag = by_version["1.0.0"];
    assert_eq!(tag.version_normalized, "1.0.0.0");
    assert!(!tag.default_branch);
    assert_eq!(tag.time.as_deref(), Some("2023-06-15T12:30:00+00:00"));
    let source = tag.source.as_ref().unwrap();
    assert_eq!(source["url"], "https://github.com/acme/widget-gh.git");
    assert_eq!(source["reference"], TAG_SHA);
    let dist = tag.dist.as_ref().unwrap();
    assert_eq!(
        dist["url"],
        format!("https://api.github.com/repos/acme/widget-gh/zipball/{TAG_SHA}")
    );
    assert_eq!(dist["reference"], TAG_SHA);
    assert_eq!(
        tag.raw["support"]["source"],
        "https://github.com/acme/widget-gh/tree/1.0.0"
    );

    let main = by_version["dev-main"];
    assert!(main.default_branch, "{main:?}");
    assert_eq!(main.time.as_deref(), Some("2024-01-01T00:00:00+00:00"));
    assert_eq!(main.require["php"].as_str(), Some(">=8.0"));
    assert_eq!(
        main.raw["support"]["source"],
        "https://github.com/acme/widget-gh/tree/main"
    );
}

#[tokio::test]
async fn github_repository_never_matches_an_unrelated_package_name() {
    let root = json!({
        "name": "root/pkg",
        "require": {"other/pkg": "^1.0"},
        "repositories": {
            "packagist.org": false,
            "gh": {"type": "github", "url": "https://github.com/acme/widget-gh"},
        },
    });
    let cache = tempfile::tempdir().unwrap();
    let transport = github_fixture_transport();
    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
        .await
        .unwrap();

    let versions = repo
        .load_package("other/pkg", DevAcceptance::Both)
        .await
        .unwrap();
    assert!(versions.is_empty(), "{versions:?}");
}
