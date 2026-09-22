//! `#294`: a `"package"`-type `repositories[]` entry — one or more packages
//! declared inline in `composer.json` itself, loaded with no network at
//! all (`vivace::repository`'s `PackageSource`). Byte-diffed against
//! Composer 2.10.2's own `composer.lock` for
//! `tests/fixtures/package-repository/`, generated with
//! `devbox run -- composer update --no-install` and committed as the
//! golden — regenerate it the same way if Composer's own output for this
//! fixture ever changes, never hand-edit it.

use std::path::Path;

use reqwest::Url;
use serde_json::Value;
use vivace::fetch::Conditional;
use vivace::repository::{Repository, Transport};

fn fixture() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/package-repository")
}

/// The fixture disables `packagist.org` and declares only `"package"`-type
/// repositories, which never fetch anything (`PackageSource::load` has no
/// network, no cache): a solve over it should never call out, so this
/// panics on any request the same way `tests/vcs.rs`'s `PanicTransport`
/// does for a vcs-only repository set.
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

/// Solves the fixture's `composer.json` and writes the lock exactly like
/// `viv update` would, the same in-process solve/write `tests/update.rs`'s
/// own golden tests use rather than the real `viv` binary (which would try
/// to install the fixture's dist zips, and they don't exist — `update
/// --no-install`/this solve never fetches a dist at all).
async fn update_lock() -> String {
    let fixture = fixture();
    let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

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
    vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
        .unwrap()
}

/// The contract test for #294: a root requiring one package declared as a
/// bare object (`acme/widget`) and one declared inside an array of two
/// (`acme/gadget`, alongside an unrequired `acme/gizmo` that proves the
/// array form is parsed without pulling in every element it holds) locks
/// byte-identically to real Composer.
#[tokio::test]
async fn package_repository_update_matches_composer_byte_for_byte() {
    let got = update_lock().await;
    let want = fs_err::read_to_string(fixture().join("composer.lock")).unwrap();
    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}
