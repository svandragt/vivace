//! Packagist v2 and Satis/Private-Packagist v1 repository clients, hermetic
//! against recorded/hand-built fixtures under `tests/fixtures/packagist/`
//! and `tests/fixtures/satis/` (`make record-packagist`/`make record-satis`).
//! Acceptance criteria from `docs/resolver-design.md`'s stage 2, extended by
//! `#67`: a monolog closure load reproduces the package/version set Composer
//! would load, a second load on a warm cache makes zero transport calls, a
//! 304 response keeps the cached body, the v1 `providers-url`/`includes`
//! protocols parse the same way Composer 1/Satis/older Private Packagist
//! serve them, and multiple repositories merge in priority/canonical order
//! (`RepositorySet`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{Map, Value, json};
use vivace::fetch::Conditional;
use vivace::repository::{ClosureRoot, DevAcceptance, Repository, Transport};
use vivace::solver;

/// The `Last-Modified` value every fixture response claims, so a test can
/// send it back as `If-Modified-Since` and get a 304.
const FIXED_LAST_MODIFIED: &str = "Mon, 01 Jan 2024 00:00:00 GMT";

fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/repo.packagist.org")
}

fn satis_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/satis/hand")
        .join(name)
}

fn wpackagist_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wpackagist/wpackagist.org")
}

// #119: bedrock's actual wpackagist mirror is `repo.wp-packages.org`, a v2
// (`metadata-url`) repository, unlike the classic `providers-url` protocol
// `wpackagist.org` itself still serves (`wpackagist_root`, whose
// provider-includes listing already gates per-name requests for free). Its
// root `packages.json` carries `available-package-patterns` (verified
// against the live file as of 2026-09-07: `["wp-plugin/*", "wp-theme/*"]`).
fn available_patterns_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wpackagist/hand/available-patterns")
}

fn scoped_constraints_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/hand/scoped-constraints")
}

fn self_version_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/hand/self-version")
}

/// Serves recorded/hand-built fixtures by mapping a URL's host onto a root
/// directory and its path onto a file under that root, and counts every
/// call so tests can assert a warm cache makes none. A test with a single
/// repository never needs to think about the host at all
/// (`FixtureTransport::new`, keyed to `repo.packagist.org`); a multi-source
/// test builds one with [`FixtureTransport::with_roots`] instead.
struct FixtureTransport {
    roots: HashMap<String, PathBuf>,
    calls: Mutex<Vec<String>>,
}

impl FixtureTransport {
    fn new() -> Self {
        FixtureTransport::with_roots([("repo.packagist.org".to_string(), fixtures_root())])
    }

    fn with_roots(roots: impl IntoIterator<Item = (String, PathBuf)>) -> Self {
        FixtureTransport {
            roots: roots.into_iter().collect(),
            calls: Mutex::new(Vec::new()),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
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
        let Some(root) = url.host_str().and_then(|host| self.roots.get(host)) else {
            return Ok(Conditional::NotFound);
        };
        let path = root.join(url.path().trim_start_matches('/'));
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

    let mut constraint_cache = solver::ConstraintCache::new();
    let closure = repo
        .load_closure(
            &roots,
            DevAcceptance::NonDevOnly,
            &|_, _| true,
            &mut constraint_cache,
        )
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
    // #90: the root's own `^3.0` constraint narrows the closure to
    // monolog's 3.x versions only, not every version Packagist ever
    // published.
    assert!(!monolog_versions.contains(&"1.0.0"), "{monolog_versions:?}");

    let psr_log_versions: Vec<&str> = closure["psr/log"]
        .iter()
        .map(|v| v.version.as_str())
        .collect();
    assert!(psr_log_versions.contains(&"3.0.2"), "{psr_log_versions:?}");
    // monolog 3.x only ever requires `psr/log: ^2.0 || ^3.0`, so psr/log's
    // own 1.x line is never even a candidate.
    assert!(!psr_log_versions.contains(&"1.0.0"), "{psr_log_versions:?}");
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

    let mut constraint_cache = solver::ConstraintCache::new();
    repo.load_closure(
        &roots,
        DevAcceptance::NonDevOnly,
        &|_, _| true,
        &mut constraint_cache,
    )
    .await
    .unwrap();
    let calls_after_first = transport.call_count();
    assert!(calls_after_first > 0);

    let mut constraint_cache = solver::ConstraintCache::new();
    repo.load_closure(
        &roots,
        DevAcceptance::NonDevOnly,
        &|_, _| true,
        &mut constraint_cache,
    )
    .await
    .unwrap();
    assert_eq!(transport.call_count(), calls_after_first);
}

// #90: `PoolBuilder::markPackageNameForLoading`/`loadPackage` only ever load
// (and only ever queue the requires of) the versions of a name that satisfy
// the accumulated constraint every requirer of that name has contributed —
// not every stability-acceptable version of every reachable name. `acme/a`'s
// 1.x line requires `old/dep`, its 2.x line requires `new/dep`; a root
// requiring `acme/a: ^2.0` must never even fetch `old/dep`, and must return
// only `acme/a`'s 2.x versions.
#[tokio::test]
async fn constraint_narrows_the_closure_to_versions_the_root_can_actually_use() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport::with_roots([(
        "scoped-constraints".to_string(),
        scoped_constraints_root(),
    )]);
    let repo = Repository::load("https://scoped-constraints", cache.path(), &transport)
        .await
        .unwrap();

    let root_require = require(&[("acme/a", "^2.0")]);
    let root_require_dev = Map::new();
    let roots = [ClosureRoot {
        require: &root_require,
        require_dev: &root_require_dev,
    }];

    let mut constraint_cache = solver::ConstraintCache::new();
    let closure = repo
        .load_closure(
            &roots,
            DevAcceptance::NonDevOnly,
            &|_, _| true,
            &mut constraint_cache,
        )
        .await
        .unwrap();

    let a_versions: Vec<&str> = closure["acme/a"]
        .iter()
        .map(|v| v.version.as_str())
        .collect();
    assert_eq!(a_versions.len(), 2, "{a_versions:?}");
    assert!(a_versions.contains(&"2.0.0"), "{a_versions:?}");
    assert!(a_versions.contains(&"2.5.0"), "{a_versions:?}");

    assert!(
        closure.contains_key("new/dep"),
        "{:?}",
        closure.keys().collect::<Vec<_>>()
    );
    assert!(
        !closure.contains_key("old/dep"),
        "{:?}",
        closure.keys().collect::<Vec<_>>()
    );
    assert!(
        !transport
            .calls()
            .iter()
            .any(|url| url.contains("old/dep") || url.contains("old%2Fdep")),
        "{:?}",
        transport.calls()
    );
}

// #115: `acme/meta` 1.2.0's own `require` for `acme/core` is the literal
// string `self.version`, which Composer's `ArrayLoader` substitutes for
// `acme/meta`'s own pretty version at package-load time
// (`ArrayLoader::parseLinks`) — so the closure walk must never hand
// `"self.version"` to `parse_constraint_cached` itself. If it did, this
// would fail with "Could not parse version constraint self.version" instead
// of narrowing to `acme/core` 1.2.0.
#[tokio::test]
async fn self_version_require_resolves_to_the_requiring_packages_own_version() {
    let cache = tempfile::tempdir().unwrap();
    let transport =
        FixtureTransport::with_roots([("self-version".to_string(), self_version_root())]);
    let repo = Repository::load("https://self-version", cache.path(), &transport)
        .await
        .unwrap();

    let root_require = require(&[("acme/meta", "^1.0")]);
    let root_require_dev = Map::new();
    let roots = [ClosureRoot {
        require: &root_require,
        require_dev: &root_require_dev,
    }];

    let mut constraint_cache = solver::ConstraintCache::new();
    let closure = repo
        .load_closure(
            &roots,
            DevAcceptance::NonDevOnly,
            &|_, _| true,
            &mut constraint_cache,
        )
        .await
        .unwrap();

    let core_versions: Vec<&str> = closure["acme/core"]
        .iter()
        .map(|v| v.version.as_str())
        .collect();
    assert_eq!(core_versions, vec!["1.2.0"], "{core_versions:?}");
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

// --- v1 protocol: `providers-url`/`provider-includes` (sha256), `includes`
// (sha1, Satis's own default), and multi-repository construction from
// `composer.json` (#67). ---

#[tokio::test]
async fn v1_providers_url_fetches_via_the_sha256_verified_listing() {
    let cache = tempfile::tempdir().unwrap();
    let transport =
        FixtureTransport::with_roots([("satis-providers".to_string(), satis_root("providers"))]);
    let repo = Repository::load("https://satis-providers", cache.path(), &transport)
        .await
        .unwrap();

    let versions = repo
        .load_package("acme/foo", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    let versions_summary: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(versions.len(), 1, "{versions_summary:?}");
    assert_eq!(versions[0].version, "1.0.0");

    // A name absent from the provider-includes listing costs no request and
    // is simply not found.
    let missing = repo
        .load_package("acme/does-not-exist", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert!(missing.is_empty());
}

#[tokio::test]
async fn v1_providers_url_cache_hit_makes_no_further_requests() {
    let cache = tempfile::tempdir().unwrap();
    let transport =
        FixtureTransport::with_roots([("satis-providers".to_string(), satis_root("providers"))]);
    let repo = Repository::load("https://satis-providers", cache.path(), &transport)
        .await
        .unwrap();
    repo.load_package("acme/foo", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    let calls_after_first = transport.call_count();
    assert!(calls_after_first > 0);

    // A fresh `Repository` (no in-memory memoization) against the same warm
    // disk cache: every provider-includes/provider file is sha256-matched,
    // so nothing beyond `packages.json` itself is requested again... but
    // `packages.json` uses `Last-Modified`, so its one revalidation still
    // counts. The sha256-verified files must not add any further calls.
    let transport2 =
        FixtureTransport::with_roots([("satis-providers".to_string(), satis_root("providers"))]);
    let repo2 = Repository::load("https://satis-providers", cache.path(), &transport2)
        .await
        .unwrap();
    repo2
        .load_package("acme/foo", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert_eq!(
        transport2.call_count(),
        1,
        "only packages.json's own Last-Modified revalidation, no provider-includes/provider \
         file requests once their sha256 matches the cache"
    );
}

// wpackagist.org (#105) is a v1 repository whose provider files aren't
// minified: each `packages[name]` entry is an object keyed by version
// label, the same shape as an inline `packages` entry, not the plain list
// every other recorded/hand-built v1 fixture here happens to use.
#[tokio::test]
async fn v1_provider_file_object_keyed_by_version_label() {
    let cache = tempfile::tempdir().unwrap();
    let transport =
        FixtureTransport::with_roots([("wpackagist.org".to_string(), wpackagist_root())]);
    let repo = Repository::load("https://wpackagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let versions = repo
        .load_package("wpackagist-plugin/akismet", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    let versions_summary: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(versions.len(), 2, "{versions_summary:?}");
    assert!(versions_summary.contains(&"2.2.5"));
    assert!(versions_summary.contains(&"2.2.6"));
}

// #119: a `metadata-url` source whose root `packages.json` declares
// `available-package-patterns` must never be asked (a `/p2/%package%.json`
// request) about a name outside those patterns; a name inside one still
// resolves normally.
#[tokio::test]
async fn available_package_patterns_skip_names_outside_the_patterns() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport::with_roots([
        (
            "wp-available-patterns".to_string(),
            available_patterns_root(),
        ),
        ("repo.packagist.org".to_string(), fixtures_root()),
    ]);
    let root = json!({
        "repositories": [
            {"type": "composer", "url": "https://wp-available-patterns"},
        ],
    });
    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
        .await
        .unwrap();

    let monolog = repo
        .load_package("monolog/monolog", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert!(!monolog.is_empty(), "still resolves from packagist.org");
    assert!(
        !transport
            .calls()
            .iter()
            .any(|url| url.contains("wp-available-patterns") && url.contains("monolog")),
        "{:?}",
        transport.calls()
    );

    let plugin = repo
        .load_package("wp-plugin/example", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert_eq!(plugin.len(), 1, "{plugin:?}");
}

#[tokio::test]
async fn v1_includes_merges_top_level_and_included_packages() {
    let cache = tempfile::tempdir().unwrap();
    let transport =
        FixtureTransport::with_roots([("satis-includes".to_string(), satis_root("includes"))]);
    let repo = Repository::load("https://satis-includes", cache.path(), &transport)
        .await
        .unwrap();

    // `acme/baz` lives in packages.json's own top-level `packages`; `acme/bar`
    // only exists inside the sha1-verified `includes` file.
    let baz = repo
        .load_package("acme/baz", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert_eq!(baz.len(), 1);
    assert_eq!(baz[0].version, "1.0.0");

    let bar = repo
        .load_package("acme/bar", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert_eq!(bar.len(), 1);
    assert_eq!(bar[0].version, "1.0.0");
}

#[tokio::test]
async fn two_repositories_offering_the_same_version_the_first_wins() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport::with_roots([
        ("canonical-a".to_string(), satis_root("canonical-a")),
        ("canonical-b".to_string(), satis_root("canonical-b")),
    ]);
    let root = json!({
        "repositories": [
            {"type": "composer", "url": "https://canonical-a"},
            {"type": "composer", "url": "https://canonical-b"},
            {"packagist.org": false},
        ],
    });
    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
        .await
        .unwrap();

    let versions = repo
        .load_package("acme/dup", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    // `canonical-a` (declared first, canonical by default) is the only
    // source consulted: `canonical-b`'s distinguishing `require` entry must
    // not appear, proving its version was never even fetched.
    let versions_summary: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(versions.len(), 1, "{versions_summary:?}");
    assert!(versions[0].require.is_empty(), "{:?}", versions[0].require);
}

#[tokio::test]
async fn non_canonical_repository_does_not_block_lower_priority_repositories() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport::with_roots([
        ("canonical-a".to_string(), satis_root("canonical-a")),
        ("canonical-b".to_string(), satis_root("canonical-b")),
    ]);
    let root = json!({
        "repositories": [
            {"type": "composer", "url": "https://canonical-a", "canonical": false},
            {"type": "composer", "url": "https://canonical-b"},
            {"packagist.org": false},
        ],
    });
    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
        .await
        .unwrap();

    let versions = repo
        .load_package("acme/dup", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    // Both sources contributed: `canonical-a` not being canonical never
    // stopped `canonical-b` from being asked too.
    let versions_summary: Vec<&str> = versions.iter().map(|v| v.version.as_str()).collect();
    assert_eq!(versions.len(), 2, "{versions_summary:?}");
}

#[tokio::test]
async fn only_filter_hides_names_outside_the_pattern() {
    let cache = tempfile::tempdir().unwrap();
    let transport =
        FixtureTransport::with_roots([("filters-a".to_string(), satis_root("filters-a"))]);
    let root = json!({
        "repositories": [
            {"type": "composer", "url": "https://filters-a", "only": ["acme/*"]},
            {"packagist.org": false},
        ],
    });
    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
        .await
        .unwrap();

    let allowed = repo
        .load_package("acme/allowed", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert_eq!(allowed.len(), 1);

    let blocked = repo
        .load_package("other/blocked", DevAcceptance::NonDevOnly)
        .await
        .unwrap();
    assert!(
        blocked.is_empty(),
        "\"only\" excludes the name from this source, and packagist.org is disabled"
    );
}

// --- auth.json credentials on a metadata request (#67 item 3): `fetch.rs`
// already applies `Auth::header_for` inside `Fetcher::get_conditional`
// (`HttpTransport::get` calls straight through to it), so this drives the
// real production transport — not `FixtureTransport` — against a minimal
// local HTTP server that records the `Authorization` header it received. ---

/// Accepts exactly one connection, records its `Authorization` header (empty
/// string if absent), and answers with a v2 `packages.json` body. No mock-
/// server dependency: a `packages.json` GET is a handful of header lines
/// followed by `\r\n\r\n` and no body, so a raw `TcpListener` is enough.
fn spawn_recording_server() -> (std::net::SocketAddr, std::sync::mpsc::Receiver<String>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut request = Vec::new();
        let mut buf = [0u8; 4096];
        while let Ok(n) = stream.read(&mut buf) {
            if n == 0 {
                break;
            }
            request.extend_from_slice(&buf[..n]);
            if request.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let text = String::from_utf8_lossy(&request);
        let authorization = text
            .lines()
            .find(|line| line.to_ascii_lowercase().starts_with("authorization:"))
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_string())
            .unwrap_or_default();
        let _ = tx.send(authorization);

        let body = br#"{"metadata-url":"/p2/%package%.json"}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(response.as_bytes());
        let _ = stream.write_all(body);
    });
    (addr, rx)
}

#[tokio::test]
async fn metadata_request_carries_the_configured_authorization_header() {
    let (addr, header_rx) = spawn_recording_server();

    let project = tempfile::tempdir().unwrap();
    fs_err::write(
        project.path().join("auth.json"),
        r#"{"http-basic": {"127.0.0.1": {"username": "user", "password": "pass"}}}"#,
    )
    .unwrap();
    let auth = vivace::auth::Auth::load(project.path()).unwrap();
    let fetcher = vivace::fetch::Fetcher::new(auth)
        .unwrap()
        .secure_http(false);
    let transport = vivace::repository::HttpTransport { fetcher: &fetcher };

    let cache = tempfile::tempdir().unwrap();
    // The server closes after one response with no `p2` file behind it, so
    // this errors past the `packages.json` fetch this test cares about;
    // only the recorded header matters.
    let _ = Repository::load(&format!("http://{addr}"), cache.path(), transport).await;

    let authorization = header_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("server never received a request");
    // echo -n user:pass | base64
    assert_eq!(authorization, "Basic dXNlcjpwYXNz");
}

// --- Byte-diff acceptance: `viv update` against a real `composer/satis`
// build (`make record-satis`), served statically and replayed hermetically,
// reproduces the `composer.lock` real Composer 2.10.2 wrote against the
// same static files (`docs/resolver-design.md`'s Metadata section,
// extended by `#67`). Satis's own build no longer emits the classic
// `providers-url`/`provider-includes` protocol (confirmed empirically
// against `composer/satis:dev-main`; it writes `metadata-url` plus a
// legacy `includes` fallback instead), so that path is covered by the
// hand-built fixture tests above rather than a live recording. ---

fn satis_project_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/satis/psr-project")
}

#[tokio::test]
async fn viv_update_reproduces_composers_lock_against_a_real_satis_build() {
    let cache = tempfile::tempdir().unwrap();
    let repo_root =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/satis/psr-repo/127.0.0.1");
    let transport = FixtureTransport::with_roots([("127.0.0.1".to_string(), repo_root)]);

    let composer_json = fs_err::read(satis_project_root().join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let repo = Repository::from_composer_json(&root, cache.path(), &transport)
        .await
        .unwrap();
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
    let got =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();

    let want = fs_err::read_to_string(satis_project_root().join("composer.lock")).unwrap();
    assert_eq!(
        got, want,
        "viv update's lock does not byte-match Composer's"
    );
}
