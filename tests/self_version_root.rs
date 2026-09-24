//! `#304`: `self.version` in the *root* `composer.json`'s own `require`
//! (`#115` already covers a dependency's own require of `self.version` via
//! the closure walk; the root require hit the same literal-string parse
//! failure, untouched by that fix). Offline, no-network fixtures, same
//! `"package"`-type repository and in-process solve/write as
//! `tests/package_repository.rs`.
//!
//! `default-version/composer.lock` was captured with `devbox run --
//! composer -d <copy outside any git checkout> update --no-install`: run
//! inside this repo, Composer's `VersionGuesser` would find the enclosing
//! vivace checkout's own tags and guess a version from those instead of
//! falling back to `1.0.0`, so it was generated from a copy in a directory
//! with no `.git` above it. Regenerate the same way if this fixture's
//! `composer.lock` ever needs to change.

use std::path::Path;

use reqwest::Url;
use serde_json::Value;
use vivace::fetch::Conditional;
use vivace::repository::{Repository, Transport};

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/self-version-root")
        .join(name)
}

/// Same panic-on-any-request transport as `tests/package_repository.rs`: a
/// `"package"`-type repository set never fetches anything.
struct PanicTransport;

impl Transport for &PanicTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &Url,
        _if_modified_since: Option<&str>,
    ) -> anyhow::Result<Conditional> {
        panic!(
            "unexpected HTTP request to {url}: a package-only repository set should never need it"
        );
    }
}

/// Solves `name`'s `composer.json` and writes the lock exactly like `viv
/// update --no-install` would.
async fn update_lock(name: &str) -> String {
    let fixture = fixture(name);
    let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let cache = tempfile::tempdir().unwrap();
    let transport = PanicTransport;
    let repo = Repository::from_composer_json(&fixture, &root, cache.path(), &transport)
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
    vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
        .unwrap()
}

/// The root has an explicit `version` (`2.3.0`) and requires its sibling at
/// `self.version`, with the sibling published at both `2.3.0` and a newer
/// `2.4.0`: `self.version` must pin the older, root-matching release, not
/// whatever the solver would otherwise prefer.
#[tokio::test]
async fn root_self_version_locks_the_roots_own_version() {
    let got = update_lock("with-version").await;
    let want = fs_err::read_to_string(fixture("with-version").join("composer.lock")).unwrap();
    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}

/// The root has no `version` at all: `self.version` falls back to
/// `RootPackageLoader`'s own `1.0.0` default, the same one `root_package`
/// already applies with no VCS checkout to guess from.
#[tokio::test]
async fn root_self_version_falls_back_to_the_default_root_version() {
    let got = update_lock("default-version").await;
    let want = fs_err::read_to_string(fixture("default-version").join("composer.lock")).unwrap();
    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}
