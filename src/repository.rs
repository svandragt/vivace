//! Packagist v2 repository client: `packages.json`, `/p2/` metadata
//! (`ComposerRepository::loadAsyncPackages`/`whatProvides`), minified
//! expansion (`composer/metadata-minifier`) and an HTTP cache that mirrors
//! Composer's own disk format. No solving: `docs/resolver-design.md`'s
//! stage 2, the pool builder's metadata loader.
//!
//! Skipped, with a clear error where it matters: `available-packages` and
//! `available-package-patterns` are parsed but never acted on,
//! `providers-api`, `security-advisories`, v1 provider repositories and
//! `path`/`vcs`/`artifact` repositories are not supported (a repo missing
//! `metadata-url` is treated as one of these and rejected).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use futures::stream::{self, StreamExt};
use regex::Regex;
use reqwest::Url;
use serde_json::{Map, Value};

use crate::fetch::{Conditional, Fetcher};

/// `PoolBuilder::LOAD_BATCH_SIZE`: names are loaded breadth-first in waves
/// of this many concurrent fetches.
const LOAD_BATCH_SIZE: usize = 50;

/// `PlatformRepository::PLATFORM_PACKAGE_REGEX`. Copied rather than shared
/// from `autoload::installed` (private there, and that module belongs to
/// another lane right now).
static PLATFORM_PACKAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:php(?:-64bit|-ipv6|-zts|-debug)?|hhvm|(?:ext|lib)-[a-z0-9](?:[_.-]?[a-z0-9]+)*|composer(?:-(?:plugin|runtime)-api)?)$",
    )
    .unwrap()
});

/// `PlatformRepository::isPlatformPackage`. `pub(crate)`: the lock writer
/// (`Installer::extractPlatformRequirements`, `pool_builder.rs`'s
/// `config.platform` handling) needs the same check.
pub(crate) fn is_platform_package(name: &str) -> bool {
    PLATFORM_PACKAGE.is_match(name)
}

/// Where the repository client gets bytes for a URL, conditional on a
/// cached `Last-Modified`. Production wraps [`Fetcher`]; tests serve
/// recorded fixtures and count calls, so a test can assert a warm cache
/// makes none.
pub trait Transport {
    fn get(
        &self,
        url: &Url,
        if_modified_since: Option<&str>,
    ) -> impl std::future::Future<Output = Result<Conditional>> + Send;
}

/// [`Transport`] backed by a real [`Fetcher`], for production use.
pub struct HttpTransport<'a> {
    pub fetcher: &'a Fetcher,
}

impl Transport for HttpTransport<'_> {
    async fn get(&self, url: &Url, if_modified_since: Option<&str>) -> Result<Conditional> {
        self.fetcher
            .get_conditional(url.as_str(), url, if_modified_since)
            .await
    }
}

/// Which of a package's non-dev/`~dev` provider files are worth fetching,
/// mirroring `loadAsyncPackages`'s `$acceptableStabilities` handling
/// (`ComposerRepository.php:1289-1298`): dev is skipped entirely unless
/// acceptable, and the non-dev file is skipped when only dev is acceptable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevAcceptance {
    NonDevOnly,
    DevOnly,
    Both,
}

impl DevAcceptance {
    fn wants_dev(self) -> bool {
        self != DevAcceptance::NonDevOnly
    }

    fn wants_non_dev(self) -> bool {
        self != DevAcceptance::DevOnly
    }
}

/// A single version entry's fields the solver needs; everything else stays
/// in `raw`, which the pool and lock dumper need verbatim later.
#[derive(Debug, Clone)]
pub struct PackageVersion {
    pub name: String,
    pub version: String,
    pub version_normalized: String,
    pub require: Map<String, Value>,
    pub require_dev: Map<String, Value>,
    pub replace: Map<String, Value>,
    pub provide: Map<String, Value>,
    pub conflict: Map<String, Value>,
    pub default_branch: bool,
    pub dist: Option<Value>,
    pub source: Option<Value>,
    /// `extra.branch-alias` (`dev-main` -> `3.x-dev`).
    pub branch_alias: Option<Value>,
    pub time: Option<String>,
    /// The untouched, expanded (no longer minified) version entry.
    pub raw: Value,
}

impl PackageVersion {
    fn from_value(raw: &Value) -> Result<Self> {
        let obj = raw
            .as_object()
            .context("provider version entry is not an object")?;
        let name = string_field(obj, "name")?;
        let version = string_field(obj, "version")?;
        // `version_normalized === VersionParser::DEFAULT_BRANCH_ALIAS` (a
        // literal "9999999-dev") or missing entirely both mean: recompute
        // it (`ComposerRepository.php:1327-1330`).
        let version_normalized = obj
            .get("version_normalized")
            .and_then(Value::as_str)
            .filter(|v| *v != "9999999-dev")
            .map(str::to_string);
        let version_normalized = match version_normalized {
            Some(v) => v,
            None => crate::version::normalize(&version)?,
        };
        Ok(PackageVersion {
            name,
            version,
            version_normalized,
            require: map_field(obj, "require"),
            require_dev: map_field(obj, "require-dev"),
            replace: map_field(obj, "replace"),
            provide: map_field(obj, "provide"),
            conflict: map_field(obj, "conflict"),
            default_branch: obj
                .get("default-branch")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            dist: obj.get("dist").cloned(),
            source: obj.get("source").cloned(),
            branch_alias: obj
                .get("extra")
                .and_then(|extra| extra.get("branch-alias"))
                .cloned(),
            time: obj.get("time").and_then(Value::as_str).map(str::to_string),
            raw: raw.clone(),
        })
    }
}

fn string_field(obj: &Map<String, Value>, key: &str) -> Result<String> {
    obj.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("provider version entry missing {key:?}"))
}

fn map_field(obj: &Map<String, Value>, key: &str) -> Map<String, Value> {
    obj.get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `MetadataMinifier::expand`: each entry after the first inherits every
/// key of the previous *expanded* entry; a key whose value is the literal
/// string `"__unset"` is removed instead of inherited.
fn expand_minified(versions: Vec<Value>) -> Vec<Value> {
    let mut expanded: Vec<Map<String, Value>> = Vec::with_capacity(versions.len());
    let mut current: Option<Map<String, Value>> = None;
    for version in versions {
        let Value::Object(diff) = version else {
            continue;
        };
        let next = match current.take() {
            None => diff,
            Some(mut prev) => {
                for (key, value) in diff {
                    if value == "__unset" {
                        prev.remove(&key);
                    } else {
                        prev.insert(key, value);
                    }
                }
                prev
            }
        };
        expanded.push(next.clone());
        current = Some(next);
    }
    expanded.into_iter().map(Value::Object).collect()
}

/// One `require`/`require-dev` pair whose names seed a `load_closure`
/// breadth-first search: a root `composer.json`'s own requirements, since
/// the root package itself is never fetched from Packagist.
pub struct ClosureRoot<'a> {
    pub require: &'a Map<String, Value>,
    pub require_dev: &'a Map<String, Value>,
}

/// A loaded Packagist v2 repository: `packages.json` plus a cache of
/// fetched `/p2/` provider files.
pub struct Repository<T: Transport> {
    transport: T,
    base_url: Url,
    metadata_url: String,
    /// Inline `packages` from `packages.json` (`ComposerRepository.php:413`):
    /// name -> version label -> version object, the same shape a v1
    /// `providers` file uses. Cheap to support, so it's supported.
    inline_packages: Map<String, Value>,
    /// `packages.json`'s `notify-batch` (falling back to the older `notify`),
    /// canonicalized against `base_url` (`ComposerRepository::canonicalizeUrl`):
    /// every loaded version without its own `notification-url` gets this one
    /// (`ComposerRepository.php:1709-1710`), which is how a lock's package
    /// entries end up with `"notification-url":
    /// "https://packagist.org/downloads/"` despite no provider-file version
    /// entry carrying it.
    notify_url: Option<String>,
    /// Parsed but never acted on; `PoolBuilder`'s optimisation, not a
    /// correctness concern at this stage.
    #[allow(dead_code)]
    pub available_packages: Option<Vec<String>>,
    #[allow(dead_code)]
    pub available_package_patterns: Option<Vec<String>>,
    cache_dir: PathBuf,
    /// In-memory memoization keyed by lowercased provider file name
    /// (including a trailing `~dev` for the dev file): a repeat
    /// `load_package`/`load_closure` call over names already loaded this
    /// run costs zero transport calls.
    loaded: Mutex<HashMap<String, Vec<PackageVersion>>>,
    /// Count of `transport.get` calls issued for a provider file (#55):
    /// every one of these is a real request, warm cache or not — a warm
    /// metadata cache still revalidates with `If-Modified-Since`, it just
    /// gets a 304 back instead of a body.
    requests: AtomicUsize,
}

impl<T: Transport> Repository<T> {
    /// Fetch `packages.json` from `base_url` and build a client caching
    /// under `<cache_root>/repo/<repo-host>/`.
    pub async fn load(base_url: &str, cache_root: &Path, transport: T) -> Result<Repository<T>> {
        let base_url =
            Url::parse(base_url).with_context(|| format!("invalid repository URL {base_url:?}"))?;
        let host = base_url
            .host_str()
            .with_context(|| format!("repository URL {base_url} has no host"))?
            .to_string();
        let packages_url = base_url
            .join("packages.json")
            .context("joining packages.json to the repository URL")?;
        // Cached the same way a provider file is (below): offline (#23),
        // this is what lets a warm cache serve `packages.json` itself
        // without a request, rather than failing before a single provider
        // file is even reached.
        let cache_dir = cache_root.join("repo").join(&host);
        let packages_cache_path = cache_dir.join("packages.json");
        let root = match get_cached_json(&transport, &packages_url, &packages_cache_path).await? {
            CachedJson::NotFound => bail!("{packages_url}: not found"),
            CachedJson::Data(data) => data,
        };
        let metadata_url = root
            .get("metadata-url")
            .and_then(Value::as_str)
            .map(str::to_string)
            .with_context(|| {
                format!(
                    "{packages_url}: no metadata-url (v1 provider repositories are not \
                     supported in vivace v0.1)"
                )
            })?;
        let inline_packages = root
            .get("packages")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let available_packages = string_list(&root, "available-packages");
        let available_package_patterns = string_list(&root, "available-package-patterns");
        let notify_url = root
            .get("notify-batch")
            .or_else(|| root.get("notify"))
            .and_then(Value::as_str)
            .map(|url| canonicalize_url(&base_url, url));
        Ok(Repository {
            transport,
            base_url,
            metadata_url,
            inline_packages,
            notify_url,
            available_packages,
            available_package_patterns,
            cache_dir,
            loaded: Mutex::new(HashMap::new()),
            requests: AtomicUsize::new(0),
        })
    }

    /// Provider-file requests issued so far (#55): excludes the initial
    /// `packages.json` fetch and any name served from `inline_packages`
    /// without a request.
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    /// Fetch a package's non-dev and/or `~dev` provider file, expanding it
    /// if minified. A missing package (404) is `Ok(vec![])`, not an error
    /// (`whatProvides` treats 404/499 the same way).
    pub async fn load_package(
        &self,
        name: &str,
        dev: DevAcceptance,
    ) -> Result<Vec<PackageVersion>> {
        let key = name.to_ascii_lowercase();
        if let Some(cached) = self.loaded.lock().expect("loaded mutex").get(&key) {
            return Ok(cached.clone());
        }
        let mut versions = Vec::new();
        if dev.wants_non_dev() {
            versions.extend(self.fetch_provider(&key, false).await?);
        }
        if dev.wants_dev() {
            versions.extend(self.fetch_provider(&key, true).await?);
        }
        if let Some(notify_url) = &self.notify_url {
            for version in &mut versions {
                if let Value::Object(obj) = &mut version.raw
                    && !obj.contains_key("notification-url")
                {
                    obj.insert(
                        "notification-url".to_string(),
                        Value::String(notify_url.clone()),
                    );
                }
            }
        }
        self.loaded
            .lock()
            .expect("loaded mutex")
            .insert(key, versions.clone());
        Ok(versions)
    }

    async fn fetch_provider(&self, name: &str, dev_file: bool) -> Result<Vec<PackageVersion>> {
        if !dev_file && let Some(inline) = self.inline_packages.get(name) {
            return parse_inline_versions(name, inline);
        }
        let file_name = if dev_file {
            format!("{name}~dev")
        } else {
            name.to_string()
        };
        let url = self.provider_url(&file_name)?;
        let cache_path = self.cache_path(&file_name);
        self.requests.fetch_add(1, Ordering::Relaxed);
        match get_cached_json(&self.transport, &url, &cache_path).await? {
            CachedJson::NotFound => Ok(Vec::new()),
            CachedJson::Data(data) => parse_provider_versions(&data, name),
        }
    }

    fn provider_url(&self, file_name: &str) -> Result<Url> {
        let path = self.metadata_url.replace("%package%", file_name);
        self.base_url
            .join(&path)
            .with_context(|| format!("invalid metadata-url substitution {path:?}"))
    }

    /// `provider-<name with / replaced by $>.json` (`ComposerRepository.php:1130`).
    fn cache_path(&self, file_name: &str) -> PathBuf {
        self.cache_dir
            .join(format!("provider-{}.json", file_name.replace('/', "$")))
    }

    /// Breadth-first metadata load, batching `LOAD_BATCH_SIZE` concurrent
    /// fetches per wave (`PoolBuilder::LOAD_BATCH_SIZE`). Discovers further
    /// names from each loaded version's `require` only; platform packages
    /// are never queued (`PlatformRepository::isPlatformPackage`).
    pub async fn load_closure(
        &self,
        roots: &[ClosureRoot<'_>],
        dev: DevAcceptance,
    ) -> Result<HashMap<String, Vec<PackageVersion>>> {
        self.load_closure_skipping(roots, dev, &HashSet::new())
            .await
    }

    /// Same breadth-first closure walk as [`Repository::load_closure`], but
    /// a name in `skip` (already lowercased) is never fetched or queued:
    /// `PoolBuilder::loadPackage`'s `if (isset($this->loadedPackages[$name]))
    /// continue;` fast path, which is how a partial update's locked-out
    /// packages (`solver::pool_builder::build_partial`, already loaded
    /// straight from the lock) stop the closure walk from re-fetching them
    /// or their own requirements' remote alternatives.
    pub async fn load_closure_skipping(
        &self,
        roots: &[ClosureRoot<'_>],
        dev: DevAcceptance,
        skip: &HashSet<String>,
    ) -> Result<HashMap<String, Vec<PackageVersion>>> {
        let closure_started = Instant::now();
        let requests_before = self.request_count();
        let mut discovered: HashSet<String> = skip.clone();
        let mut queue = VecDeque::new();
        for root in roots {
            for name in root.require.keys().chain(root.require_dev.keys()) {
                queue_name(name, &mut discovered, &mut queue);
            }
        }

        let mut result = HashMap::new();
        let mut batches = 0usize;
        while !queue.is_empty() {
            batches += 1;
            let batch: Vec<String> = std::iter::from_fn(|| queue.pop_front())
                .take(LOAD_BATCH_SIZE)
                .collect();
            let loaded: Vec<(String, Result<Vec<PackageVersion>>)> = stream::iter(batch)
                .map(|name| async move { (name.clone(), self.load_package(&name, dev).await) })
                .buffer_unordered(LOAD_BATCH_SIZE)
                .collect()
                .await;
            for (name, versions) in loaded {
                let versions = versions?;
                for version in &versions {
                    for req in version.require.keys() {
                        queue_name(req, &mut discovered, &mut queue);
                    }
                }
                result.insert(name, versions);
            }
        }
        tracing::debug!(
            packages = result.len(),
            batches,
            requests = self.request_count() - requests_before,
            elapsed_ms = closure_started.elapsed().as_millis(),
            "loaded metadata closure"
        );
        Ok(result)
    }
}

fn queue_name(name: &str, discovered: &mut HashSet<String>, queue: &mut VecDeque<String>) {
    let name = name.to_ascii_lowercase();
    if is_platform_package(&name) || name == "__root__" {
        return;
    }
    if discovered.insert(name.clone()) {
        queue.push_back(name);
    }
}

/// `ComposerRepository::canonicalizeUrl`: a root-relative `notify-batch`
/// (rare; Packagist's own is already absolute) resolves against `base`'s
/// scheme and host, everything else is returned unchanged.
fn canonicalize_url(base: &Url, url: &str) -> String {
    if !url.starts_with('/') {
        return url.to_string();
    }
    base.join(url)
        .map_or_else(|_| url.to_string(), |u| u.to_string())
}

fn string_list(root: &Value, key: &str) -> Option<Vec<String>> {
    root.get(key)?.as_array().map(|values| {
        values
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect()
    })
}

/// A repo's inline `packages[name]` entry (`ComposerRepository.php:413`) is
/// keyed by version label, not a plain list; the labels aren't otherwise
/// used, only the version objects they hold.
fn parse_inline_versions(name: &str, inline: &Value) -> Result<Vec<PackageVersion>> {
    let versions = inline
        .as_object()
        .with_context(|| format!("{name}: inline packages entry is not an object"))?;
    versions.values().map(PackageVersion::from_value).collect()
}

fn parse_provider_versions(data: &Value, name: &str) -> Result<Vec<PackageVersion>> {
    let Some(entry) = data
        .get("packages")
        .and_then(Value::as_object)
        .and_then(|packages| packages.get(name))
    else {
        return Ok(Vec::new());
    };
    let list = entry
        .as_array()
        .with_context(|| format!("{name}: provider entry is not a list"))?
        .clone();
    let minified = data.get("minified").and_then(Value::as_str) == Some("composer/2.0");
    let expanded = if minified {
        expand_minified(list)
    } else {
        list
    };
    expanded.iter().map(PackageVersion::from_value).collect()
}

/// Reads a cache file written by [`write_cache_file`]: the raw provider
/// JSON with a `last-modified` key merged in, mirroring Composer's own
/// `Cache` format for this file (`ComposerRepository.php:1793-1797`) so the
/// value can be sent back as `If-Modified-Since` next time.
fn read_cache_file(path: &Path) -> Result<Option<(Value, Option<String>)>> {
    match fs_err::read(path) {
        Ok(bytes) => {
            let data: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("{}: cached file is not valid JSON", path.display()))?;
            let last_modified = data
                .get("last-modified")
                .and_then(Value::as_str)
                .map(str::to_string);
            Ok(Some((data, last_modified)))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
    }
}

fn write_cache_file(path: &Path, data: &Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs_err::create_dir_all(parent)?;
    }
    fs_err::write(path, serde_json::to_vec(data)?)
        .with_context(|| format!("writing {}", path.display()))
}

/// [`get_cached_json`]'s outcome: either the resource doesn't exist (a 404;
/// the caller decides what that means for it), or its current body, from the
/// network or the disk cache.
enum CachedJson {
    NotFound,
    Data(Value),
}

/// The conditional-GET-against-a-disk-cache dance `packages.json` and every
/// provider file share: read a cached body and its `Last-Modified`, send
/// that back as `If-Modified-Since`, and cache a fresh response before
/// returning it. Offline (#23), this is also what serves a warm cache
/// without a request: [`Fetcher::get_conditional`]'s own offline branch
/// answers a conditional request (`since` is `Some`) with a synthetic
/// "not modified" instead of erroring, so the cached body here is used as
/// read, and only a truly uncached URL (`since` is `None`) surfaces that
/// transport's "network disabled" error.
async fn get_cached_json<T: Transport>(
    transport: &T,
    url: &Url,
    cache_path: &Path,
) -> Result<CachedJson> {
    let cached = read_cache_file(cache_path)?;
    let since = cached.as_ref().and_then(|(_, lm)| lm.as_deref());
    match transport.get(url, since).await? {
        Conditional::NotFound => Ok(CachedJson::NotFound),
        Conditional::NotModified => Ok(CachedJson::Data(
            cached.context("server sent 304 but nothing is cached")?.0,
        )),
        Conditional::Fresh {
            body,
            last_modified,
        } => {
            let mut data: Value =
                serde_json::from_slice(&body).with_context(|| format!("{url}: not valid JSON"))?;
            if let (Some(lm), Value::Object(obj)) = (&last_modified, &mut data) {
                obj.insert("last-modified".to_string(), Value::String(lm.clone()));
            }
            write_cache_file(cache_path, &data)?;
            Ok(CachedJson::Data(data))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(value: Value) -> Value {
        value
    }

    #[test]
    fn expand_minified_inherits_and_unsets() {
        let raw: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/packagist/hand/minified.json"
        )))
        .unwrap();
        let list = raw["packages"]["acme/minified"].as_array().unwrap().clone();
        let expanded = expand_minified(list);
        assert_eq!(expanded.len(), 2);
        // Inherited from the first entry: unchanged, so absent from the diff.
        assert_eq!(
            expanded[1]["name"],
            v(Value::String("acme/minified".into()))
        );
        assert_eq!(
            expanded[1]["require"],
            v(serde_json::json!({"php": ">=8.0"}))
        );
        // "__unset" removes a key the first entry had.
        assert!(expanded[1].get("default-branch").is_none());
        assert_eq!(expanded[0]["default-branch"], v(Value::Bool(true)));
    }

    #[test]
    fn package_version_renormalizes_default_branch_alias() {
        let raw: Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/packagist/hand/dev-branch-alias.json"
        )))
        .unwrap();
        let entry = &raw["packages"]["acme/dev-pkg"][0];
        let parsed = PackageVersion::from_value(entry).unwrap();
        // "9999999-dev" is a literal stand-in, not the real normalized form
        // of "dev-main"; it must be recomputed.
        assert_eq!(parsed.version_normalized, "dev-main");
        assert!(parsed.default_branch);
    }

    #[test]
    fn package_version_keeps_ordinary_normalized_version() {
        let entry = serde_json::json!({
            "name": "acme/pkg",
            "version": "1.0.0",
            "version_normalized": "1.0.0.0",
        });
        let parsed = PackageVersion::from_value(&entry).unwrap();
        assert_eq!(parsed.version_normalized, "1.0.0.0");
    }
}
