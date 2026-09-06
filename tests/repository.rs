//! Packagist v2 repository client, hermetic against recorded fixtures under
//! `tests/fixtures/packagist/repo.packagist.org/` (refreshed by `make
//! record-packagist`). Acceptance criteria from `docs/resolver-design.md`'s
//! stage 2: a monolog closure load reproduces the package/version set
//! Composer would load, a second load on a warm cache makes zero transport
//! calls, and a 304 response keeps the cached body.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};
use vivace::fetch::Conditional;
use vivace::repository::{ClosureRoot, DevAcceptance, Repository, Transport};

/// The `Last-Modified` value every fixture response claims, so a test can
/// send it back as `If-Modified-Since` and get a 304.
const FIXED_LAST_MODIFIED: &str = "Mon, 01 Jan 2024 00:00:00 GMT";

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/repo.packagist.org")
}

/// Serves recorded fixtures by mapping a URL's path onto a file under
/// `root`, and counts every call so tests can assert a warm cache makes
/// none.
struct FixtureTransport {
    root: PathBuf,
    calls: Mutex<Vec<String>>,
}

impl FixtureTransport {
    fn new() -> Self {
        FixtureTransport {
            root: fixtures_root(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

// `Repository` owns its transport by value; implementing `Transport` for
// `&FixtureTransport` (`&T` is a "fundamental" type, so this is allowed
// under the orphan rules even though both `Transport` and `FixtureTransport`
// live outside this crate's control) lets a test keep its own handle to
// [`FixtureTransport::call_count`] after handing a reference to
// `Repository::load`.
impl Transport for &FixtureTransport {
    // The fixture transport reads a local file synchronously; no `.await`
    // is needed, but the trait signature is async for the real transport.
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &reqwest::Url,
        if_modified_since: Option<&str>,
    ) -> anyhow::Result<Conditional> {
        self.calls.lock().unwrap().push(url.to_string());
        if if_modified_since == Some(FIXED_LAST_MODIFIED) {
            return Ok(Conditional::NotModified);
        }
        let path = self.root.join(url.path().trim_start_matches('/'));
        match fs_err::read(&path) {
            Ok(body) => Ok(Conditional::Fresh {
                body,
                last_modified: Some(FIXED_LAST_MODIFIED.to_string()),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Conditional::NotFound),
            Err(err) => Err(err.into()),
        }
    }
}

fn require(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(name, constraint)| (name.to_string(), json!(constraint)))
        .collect()
}

#[tokio::test]
async fn monolog_closure_reproduces_composers_package_set() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport::new();
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let root_require = require(&[("monolog/monolog", "^3.0")]);
    let root_require_dev = Map::new();
    let roots = [ClosureRoot {
        require: &root_require,
        require_dev: &root_require_dev,
    }];

    let closure = repo
        .load_closure(&roots, DevAcceptance::NonDevOnly)
        .await
        .unwrap();

    // monolog/monolog is the seed, psr/log is discovered transitively via
    // its `require`; platform packages (php) are never queued.
    assert!(
        closure.contains_key("monolog/monolog"),
        "{:?}",
        closure.keys().collect::<Vec<_>>()
    );
    assert!(
        closure.contains_key("psr/log"),
        "{:?}",
        closure.keys().collect::<Vec<_>>()
    );
    assert!(!closure.contains_key("php"));

    let monolog_versions: Vec<&str> = closure["monolog/monolog"]
        .iter()
        .map(|v| v.version.as_str())
        .collect();
    assert!(monolog_versions.contains(&"3.11.0"), "{monolog_versions:?}");
    assert!(monolog_versions.contains(&"1.0.0"), "{monolog_versions:?}");

    let psr_log_versions: Vec<&str> = closure["psr/log"]
        .iter()
        .map(|v| v.version.as_str())
        .collect();
    assert!(psr_log_versions.contains(&"3.0.2"), "{psr_log_versions:?}");
}

#[tokio::test]
async fn second_load_closure_on_warm_cache_makes_no_transport_calls() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport::new();
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let root_require = require(&[("monolog/monolog", "^3.0")]);
    let root_require_dev = Map::new();
    let roots = [ClosureRoot {
        require: &root_require,
        require_dev: &root_require_dev,
    }];

    repo.load_closure(&roots, DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    let calls_after_first = transport.call_count();
    assert!(calls_after_first > 0);

    repo.load_closure(&roots, DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert_eq!(transport.call_count(), calls_after_first);
}

#[tokio::test]
async fn a_304_response_keeps_the_cached_body() {
    let cache = tempfile::tempdir().unwrap();

    // First process: populates the disk cache with monolog/monolog's data
    // and the fixture's Last-Modified.
    let transport1 = FixtureTransport::new();
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport1)
        .await
        .unwrap();
    let first = repo
        .load_package("monolog/monolog", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert!(!first.is_empty());

    // Second process: a fresh `Repository` (no in-memory memoization) reads
    // the disk cache, sends its Last-Modified back, and the fixture
    // transport returns 304 — the parsed result must still match.
    let transport2 = FixtureTransport::new();
    let repo2 = Repository::load("https://repo.packagist.org", cache.path(), &transport2)
        .await
        .unwrap();
    let second = repo2
        .load_package("monolog/monolog", DevAcceptance::NonDevOnly)
        .await
        .unwrap();

    let first_versions: Vec<&str> = first.iter().map(|v| v.version.as_str()).collect();
    let second_versions: Vec<&str> = second.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(first_versions, second_versions);
}

#[tokio::test]
async fn missing_package_is_no_versions_not_an_error() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport::new();
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let versions = repo
        .load_package("acme/does-not-exist", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert!(versions.is_empty());
}
