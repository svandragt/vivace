//! #345: a `dev-*` package kept at its locked version (`preferred`,
//! `--minimal-changes` or `viv lock merge` rung 3's own everything-preferred
//! pin) whose registry-fetched branch entry no longer describes the pinned
//! commit — Packagist's own provider file only ever answers with a branch's
//! *current* head, so a rolling branch that has since moved leaves the lock
//! pinning a commit the registry can no longer describe at all
//! (`solver::pool_builder::pin_dev_commits`). Everything here is built at
//! test time rather than a static fixture: the pinned commit's own sha and
//! the throwaway git repo's own path are only known once the repo exists.

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use common::git_command;
use reqwest::Url;
use vivace::fetch::Conditional;
use vivace::repository::{Repository, Transport};
use vivace::solver;
use vivace::store::{commit_meta_missing_path, commit_meta_path};

/// Same fixed identity/no-signing `git` helper as `tests/root_version.rs`'s
/// own `git`, so a commit here works the same on a CI runner with no GPG
/// key as on a developer machine with `commit.gpgsign` on globally.
fn git(dir: &Path, args: &[&str]) {
    let status = git_command()
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "vivace")
        .env("GIT_AUTHOR_EMAIL", "vivace@example.com")
        .env("GIT_COMMITTER_NAME", "vivace")
        .env("GIT_COMMITTER_EMAIL", "vivace@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn head_sha(dir: &Path) -> String {
    let output = git_command()
        .args(["rev-parse", "HEAD"])
        .current_dir(dir)
        .output()
        .unwrap();
    String::from_utf8(output.stdout).unwrap().trim().to_string()
}

/// A throwaway `acme/branchy` git checkout whose `main` branch moved past
/// the commit this test pins: the first commit's own `composer.json` has
/// no `require` at all, the second (the branch's current head) requires a
/// package that exists nowhere in the registry fixture below — so a solve
/// that used the *head*'s metadata instead of the pinned commit's would
/// fail outright, rather than merely resolve differently, making a
/// regression here impossible to miss.
struct BranchRepo {
    dir: PathBuf,
    old_sha: String,
    new_sha: String,
}

fn build_branch_repo(dir: &Path) -> BranchRepo {
    std::fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    fs_err::write(
        dir.join("composer.json"),
        r#"{"name": "acme/branchy", "require": {}}"#,
    )
    .unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "old"]);
    let old_sha = head_sha(dir);

    fs_err::write(
        dir.join("composer.json"),
        r#"{"name": "acme/branchy", "require": {"acme/ghost-package": "^1.0"}}"#,
    )
    .unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "new"]);
    let new_sha = head_sha(dir);

    BranchRepo {
        dir: dir.to_path_buf(),
        old_sha,
        new_sha,
    }
}

/// The registry fixture (`packages.json` + one `p2/` provider file), built
/// at test time since it must embed [`BranchRepo`]'s own dynamic path/sha:
/// its `dev-main` entry describes only `new_sha` (the branch's current
/// head), same as a real Packagist provider file for a package whose
/// branch has moved since the lock pinned it.
fn write_registry(root: &Path, branch: &BranchRepo) {
    std::fs::create_dir_all(root.join("p2/acme")).unwrap();
    fs_err::write(
        root.join("packages.json"),
        r#"{"metadata-url": "/p2/%package%.json"}"#,
    )
    .unwrap();
    let provider = serde_json::json!({
        "packages": {
            "acme/branchy": [{
                "name": "acme/branchy",
                "version": "dev-main",
                "version_normalized": "dev-main",
                "require": {"acme/ghost-package": "^1.0"},
                "source": {
                    "type": "git",
                    "url": branch.dir.to_string_lossy(),
                    "reference": branch.new_sha,
                },
                "dist": {
                    "type": "zip",
                    "url": "https://example.test/acme/branchy.zip",
                    "reference": branch.new_sha,
                    "shasum": "",
                },
            }],
        },
    });
    fs_err::write(
        root.join("p2/acme/branchy.json"),
        serde_json::to_vec(&provider).unwrap(),
    )
    .unwrap();
}

/// Serves `root`'s files unconditionally, `offline` reported straight off
/// the flag this test sets, same shape `tests/common::FixtureTransport`
/// serves a `Conditional::Fresh`/`NotFound` from disk.
struct RegistryTransport {
    root: PathBuf,
    offline: bool,
}

impl Transport for &RegistryTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &Url,
        _if_modified_since: Option<&str>,
    ) -> anyhow::Result<Conditional> {
        let path = self.root.join(url.path().trim_start_matches('/'));
        match fs_err::read(&path) {
            Ok(body) => Ok(Conditional::Fresh {
                body,
                last_modified: None,
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Conditional::NotFound),
            Err(err) => Err(err.into()),
        }
    }

    fn offline(&self) -> bool {
        self.offline
    }
}

fn locked_by_name(branch: &BranchRepo) -> HashMap<String, serde_json::Value> {
    let mut by_name = HashMap::new();
    by_name.insert(
        "acme/branchy".to_string(),
        serde_json::json!({
            "name": "acme/branchy",
            "version": "dev-main",
            "source": {
                "type": "git",
                "url": branch.dir.to_string_lossy(),
                "reference": branch.old_sha,
            },
        }),
    );
    by_name
}

fn preferred(branch: &BranchRepo) -> HashMap<String, vivace::semver::NormalizedVersion> {
    let _ = branch;
    let mut preferred = HashMap::new();
    preferred.insert(
        "acme/branchy".to_string(),
        vivace::semver::normalize("dev-main").unwrap(),
    );
    preferred
}

fn root_json() -> serde_json::Value {
    serde_json::json!({
        "name": "vivace/fixture-dev-commit-pin",
        "require": {"acme/branchy": "dev-main"},
    })
}

/// The solve itself: `acme/branchy` kept at `dev-main`
/// (`preferred`/`--minimal-changes`'s own pin), locked to `old_sha`, while
/// the registry only ever describes `new_sha`.
async fn solve(
    registry_root: &Path,
    project_dir: &Path,
    cache_dir: &Path,
    branch: &BranchRepo,
    offline: bool,
) -> anyhow::Result<vivace::solver::UpdateResult> {
    let transport = RegistryTransport {
        root: registry_root.to_path_buf(),
        offline,
    };
    let repo = Repository::load("https://registry.example.test", cache_dir, &transport)
        .await
        .unwrap();
    let root = root_json();
    let locked = locked_by_name(branch);
    let seed: Vec<String> = locked.keys().cloned().collect();

    solver::solve_update_seeded(
        &repo,
        &root,
        project_dir,
        false,
        false,
        &seed,
        preferred(branch),
        &locked,
        None::<vivace::solver::pool_builder::AdvisoryFilter<'_, vivace::audit::NoAdvisories>>,
        Some(cache_dir),
        None,
        &vivace::autoload::platform::IgnorePlatform::None,
    )
    .await
}

#[tokio::test]
async fn a_pinned_dev_commit_the_registry_no_longer_describes_uses_its_own_composer_json() {
    let project = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let registry = tempfile::tempdir().unwrap();
    let git_dir = tempfile::tempdir().unwrap();

    let branch = build_branch_repo(git_dir.path());
    write_registry(registry.path(), &branch);

    let result = solve(
        registry.path(),
        project.path(),
        cache.path(),
        &branch,
        false,
    )
    .await
    .expect(
        "the pinned (old) commit's empty require must be used, not the head's unsatisfiable \
             acme/ghost-package one",
    );
    let pkg = result
        .non_dev
        .iter()
        .find(|p| p.name == "acme/branchy")
        .expect("acme/branchy must resolve");
    assert_eq!(pkg.pretty_version, "dev-main");
    assert_eq!(
        pkg.raw
            .pointer("/source/reference")
            .and_then(|v| v.as_str()),
        Some(branch.old_sha.as_str()),
        "the resolved package must still point at the pinned commit, not the registry's head"
    );
    assert_eq!(
        pkg.raw.get("require"),
        Some(&serde_json::json!({})),
        "must carry the pinned commit's own (empty) require, not the head's"
    );

    let cache_path = commit_meta_path(cache.path(), &branch.old_sha).unwrap();
    assert!(
        cache_path.is_file(),
        "the fetched commit's composer.json must be cached under its own sha: {}",
        cache_path.display()
    );

    // Second run: the git source is gone, so any attempt to fetch again
    // would fail outright. The solve must still succeed identically,
    // proving the store hit, not a second git fetch.
    std::fs::remove_dir_all(&git_dir).unwrap();
    let result_again = solve(
        registry.path(),
        project.path(),
        cache.path(),
        &branch,
        false,
    )
    .await
    .expect("a second solve must hit the store, never git, with the source gone");
    let pkg_again = result_again
        .non_dev
        .iter()
        .find(|p| p.name == "acme/branchy")
        .unwrap();
    assert_eq!(pkg_again.pretty_version, pkg.pretty_version);
    assert_eq!(pkg_again.raw, pkg.raw);
}

#[tokio::test]
async fn offline_with_no_store_hit_names_the_package_and_commit() {
    let project = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let registry = tempfile::tempdir().unwrap();
    let git_dir = tempfile::tempdir().unwrap();

    let branch = build_branch_repo(git_dir.path());
    write_registry(registry.path(), &branch);

    let result = solve(registry.path(), project.path(), cache.path(), &branch, true).await;
    let Err(err) = result else {
        panic!("offline with nothing cached for the pinned commit must fail")
    };
    let message = format!("{err:#}");
    assert!(
        message.contains("acme/branchy"),
        "must name the package: {message}"
    );
    assert!(
        message.contains(&branch.old_sha),
        "must name the pinned commit: {message}"
    );
}

/// #348: a pinned commit gone upstream must not abort the whole
/// `pin_dev_commits` pass — `acme/other`, pinned at a real commit in the
/// same lock, must still be fetched and stored even though `acme/branchy`'s
/// own pin (a commit that never existed) fails; a second run must fail the
/// same way without a second fetch attempt, proven by deleting
/// `acme/branchy`'s git source outright before it runs.
#[tokio::test]
async fn a_pinned_commit_missing_upstream_is_cached_and_does_not_block_other_pins() {
    let project = tempfile::tempdir().unwrap();
    let cache = tempfile::tempdir().unwrap();
    let registry = tempfile::tempdir().unwrap();
    let branchy_git_dir = tempfile::tempdir().unwrap();
    let other_git_dir = tempfile::tempdir().unwrap();

    let branchy = build_branch_repo(branchy_git_dir.path());
    let other = build_branch_repo(other_git_dir.path());
    // `acme/branchy`'s own registry fixture, unmodified.
    write_registry(registry.path(), &branchy);
    // A second package, same shape, its own provider file: the registry
    // only ever describes `new_sha`, same gap `write_registry` relies on.
    let provider_b = serde_json::json!({
        "packages": {
            "acme/other": [{
                "name": "acme/other",
                "version": "dev-main",
                "version_normalized": "dev-main",
                "require": {"acme/ghost-package": "^1.0"},
                "source": {
                    "type": "git",
                    "url": other.dir.to_string_lossy(),
                    "reference": other.new_sha,
                },
                "dist": {
                    "type": "zip",
                    "url": "https://example.test/acme/other.zip",
                    "reference": other.new_sha,
                    "shasum": "",
                },
            }],
        },
    });
    fs_err::write(
        registry.path().join("p2/acme/other.json"),
        serde_json::to_vec(&provider_b).unwrap(),
    )
    .unwrap();

    // 40 hex chars, guaranteed to name no commit in `branchy.dir`.
    let missing_sha = "f".repeat(40);

    let mut locked = locked_by_name(&branchy);
    locked
        .get_mut("acme/branchy")
        .unwrap()
        .as_object_mut()
        .unwrap()
        .insert(
            "source".to_string(),
            serde_json::json!({
                "type": "git",
                "url": branchy.dir.to_string_lossy(),
                "reference": missing_sha,
            }),
        );
    locked.insert(
        "acme/other".to_string(),
        serde_json::json!({
            "name": "acme/other",
            "version": "dev-main",
            "source": {
                "type": "git",
                "url": other.dir.to_string_lossy(),
                "reference": other.old_sha,
            },
        }),
    );

    let mut preferred_names = preferred(&branchy);
    preferred_names.insert(
        "acme/other".to_string(),
        vivace::semver::normalize("dev-main").unwrap(),
    );

    let root = serde_json::json!({
        "name": "vivace/fixture-dev-commit-pin-multi",
        "require": {"acme/branchy": "dev-main", "acme/other": "dev-main"},
    });

    let run = || {
        let transport = RegistryTransport {
            root: registry.path().to_path_buf(),
            offline: false,
        };
        let root = root.clone();
        let locked = locked.clone();
        let preferred_names = preferred_names.clone();
        let cache_path = cache.path().to_path_buf();
        let project_path = project.path().to_path_buf();
        async move {
            let repo = Repository::load("https://registry.example.test", &cache_path, &transport)
                .await
                .unwrap();
            let seed: Vec<String> = locked.keys().cloned().collect();
            solver::solve_update_seeded(
                &repo,
                &root,
                &project_path,
                false,
                false,
                &seed,
                preferred_names,
                &locked,
                None::<vivace::solver::pool_builder::AdvisoryFilter<'_, vivace::audit::NoAdvisories>>,
                Some(&cache_path),
                None,
                &vivace::autoload::platform::IgnorePlatform::None,
            )
            .await
        }
    };

    let Err(err) = run().await else {
        panic!("a solve with a pinned commit missing upstream must fail")
    };
    let message = format!("{err:#}");
    assert!(
        message.contains("acme/branchy"),
        "must name the package: {message}"
    );
    assert!(
        message.contains(&missing_sha[..7]),
        "must name the pinned commit: {message}"
    );
    assert!(
        message.contains(&branchy.dir.to_string_lossy().to_string()),
        "must name the source url: {message}"
    );

    let missing_path = commit_meta_missing_path(cache.path(), &missing_sha).unwrap();
    assert!(
        missing_path.is_file(),
        "the missing commit must be recorded: {}",
        missing_path.display()
    );
    let other_cache_path = commit_meta_path(cache.path(), &other.old_sha).unwrap();
    assert!(
        other_cache_path.is_file(),
        "acme/other's own pinned commit must still be stored on this same pass: {}",
        other_cache_path.display()
    );

    // acme/branchy's git source is gone outright, so a second live fetch
    // attempt could only fail differently (or not find anything to blame on
    // this url at all) — getting back the exact same message proves this
    // came from the missing-commit cache, not another git fetch.
    std::fs::remove_dir_all(&branchy_git_dir).unwrap();
    let Err(err_again) = run().await else {
        panic!("a second run must still fail the same way")
    };
    assert_eq!(
        format!("{err_again:#}"),
        message,
        "second run must reuse the cached error, not attempt another fetch"
    );
}
