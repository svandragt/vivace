//! Repository clients: Packagist v2 (`packages.json`, `/p2/` metadata,
//! `ComposerRepository::loadAsyncPackages`/`whatProvides`) and the v1
//! protocol Satis and older Private Packagist still serve (`providers-url`,
//! `providers-lazy-url`, `provider-includes`, `includes`), plus the
//! multi-repository construction `RepositoryFactory`/`Config`/
//! `FilterRepository` do from `composer.json`'s `repositories` (order,
//! `exclude`/`only`/`canonical`, `packagist.org: false`). Minified
//! expansion (`composer/metadata-minifier`) and an HTTP cache that mirrors
//! Composer's own disk format. No solving: `docs/resolver-design.md`'s
//! stage 2, the pool builder's metadata loader.
//!
//! Skipped, with a clear error where it matters: `available-packages` and
//! `available-package-patterns` are ignored outright (an optimisation, not
//! a correctness concern), `providers-api`, `security-advisories`, and
//! `path`/`vcs`/`artifact` repositories are not supported (a repository
//! whose `type` isn't `"composer"` is rejected; a `"composer"` repository
//! missing every provider mechanism below is treated as empty rather than
//! erroring, matching `whatProvides`'s own `return []`).

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
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::fetch::{Conditional, Fetcher};
use crate::store::hex;

/// `Config::$defaultRepositories`: the implicit last (lowest-priority)
/// repository, unless `composer.json` disables it (`packagist.org: false`)
/// or redefines a `composer`-type repository at this same URL.
pub const PACKAGIST_URL: &str = "https://repo.packagist.org";

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
/// makes none. Every content-addressed (sha1/sha256-verified) fetch also
/// goes through this same method with `if_modified_since: None`: an
/// unconditional GET, since a hash mismatch is the only reason to ask again.
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
/// Meaningless for a v1 source (`Provider::Providers`/`Provider::Eager`):
/// those protocols never split dev out into its own file, so every version
/// is always returned and the pool builder's own stability filter
/// (`push_package_version`) is what actually discards an unacceptable one.
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

/// `FilterRepository`: which names a source is even asked about (`only`/
/// `exclude`, package-name globs compiled the way
/// `BasePackage::packageNamesToRegexp` does), and whether finding a name
/// here stops lower-priority sources from being asked about it at all
/// (`canonical`, default `true`).
#[derive(Debug)]
struct RepoFilters {
    only: Option<Regex>,
    exclude: Option<Regex>,
    canonical: bool,
}

impl Default for RepoFilters {
    fn default() -> Self {
        RepoFilters {
            only: None,
            exclude: None,
            canonical: true,
        }
    }
}

impl RepoFilters {
    fn parse(obj: &Map<String, Value>, repo_label: &str) -> Result<RepoFilters> {
        let names = |key: &str| -> Result<Option<Vec<String>>> {
            match obj.get(key) {
                None => Ok(None),
                Some(Value::Array(items)) => Ok(Some(
                    items
                        .iter()
                        .map(|v| {
                            v.as_str().map(str::to_string).with_context(|| {
                                format!("{repo_label}: {key:?} entries must be strings")
                            })
                        })
                        .collect::<Result<Vec<_>>>()?,
                )),
                Some(_) => bail!("{repo_label}: {key:?} must be an array of package names"),
            }
        };
        let only = names("only")?;
        let exclude = names("exclude")?;
        if only.is_some() && exclude.is_some() {
            bail!("{repo_label}: only one of \"only\" and \"exclude\" can be specified");
        }
        let canonical = match obj.get("canonical") {
            None => true,
            Some(Value::Bool(b)) => *b,
            Some(_) => bail!("{repo_label}: \"canonical\" must be a boolean"),
        };
        Ok(RepoFilters {
            only: only.as_deref().map(names_to_regex).transpose()?,
            exclude: exclude.as_deref().map(names_to_regex).transpose()?,
            canonical,
        })
    }

    fn allows(&self, name: &str) -> bool {
        if let Some(only) = &self.only {
            return only.is_match(name);
        }
        if let Some(exclude) = &self.exclude {
            return !exclude.is_match(name);
        }
        true
    }
}

/// `BasePackage::packageNamesToRegexp`: each name is a literal, case
/// insensitive, glob-quoted match, except a bare `*` which becomes `.*`.
fn names_to_regex(names: &[String]) -> Result<Regex> {
    let alternation = names
        .iter()
        .map(|n| regex::escape(n).replace(r"\*", ".*"))
        .collect::<Vec<_>>()
        .join("|");
    Regex::new(&format!("(?i)^(?:{alternation})$")).context("building an only/exclude pattern")
}

/// A source's provider-lookup strategy, decided once from `packages.json`'s
/// root fields (`ComposerRepository::loadRootServerFile`'s priority: v2's
/// `metadata-url` wins outright; else v1's `providers-url` (with a
/// `provider-includes` listing built once, up front); else v1's older
/// `providers-lazy-url` alone, which behaves exactly like `metadata-url`
/// (same substitution, same tolerant-404 lazy fetch); else nothing but
/// `includes`/inline `packages` (Satis's own default output, and any other
/// legacy repo with no provider mechanism at all), loaded eagerly, once,
/// with nothing left to fetch afterwards.
enum Provider {
    Lazy {
        metadata_url: String,
    },
    Providers {
        providers_url: String,
        /// name (lowercased) -> sha256, from `provider-includes`.
        listing: HashMap<String, String>,
    },
    Eager {
        packages: HashMap<String, Vec<PackageVersion>>,
    },
}

/// One loaded `composer`-type repository: `packages.json`, its provider
/// mechanism, and the on-disk cache directory Composer itself would use
/// for this host.
struct Source {
    base_url: Url,
    provider: Provider,
    /// v2's partial inline `packages` (`hasPartialPackages`), checked
    /// before the lazy `/p2/` fetch. `Provider::Eager`'s own inline
    /// `packages` are already folded into its `packages` map; this field
    /// is only ever non-empty alongside `Provider::Lazy`.
    inline_packages: Map<String, Value>,
    /// `packages.json`'s `notify-batch` (falling back to the older
    /// `notify`), canonicalized against `base_url`: every version this
    /// source loads without its own `notification-url` gets this one
    /// (`ComposerRepository.php:1709-1710`).
    notify_url: Option<String>,
    cache_dir: PathBuf,
    filters: RepoFilters,
}

impl Source {
    /// Fetch `packages.json` from `url` and set up this source's provider
    /// mechanism, caching under `<cache_root>/repo/<repo-host>/`
    /// (`ComposerRepository::getCache`'s per-repo directory, keyed the same
    /// way this crate already keyed a single Packagist repo).
    async fn load<T: Transport>(
        url: &str,
        cache_root: &Path,
        transport: &T,
        filters: RepoFilters,
    ) -> Result<Source> {
        let configured =
            Url::parse(url).with_context(|| format!("invalid repository URL {url:?}"))?;
        let host = configured
            .host_str()
            .with_context(|| format!("repository URL {configured} has no host"))?
            .to_string();
        let cache_dir = cache_root.join("repo").join(&host);

        // `ComposerRepository::getPackagesJsonUrl`: a URL that already
        // names a `.json` file is used as-is; otherwise `/packages.json` is
        // appended (string concatenation, not URL-relative resolution, so
        // a repository whose URL has its own path component isn't
        // truncated the way `Url::join` would truncate it).
        let packages_url = if configured.path().contains(".json") {
            configured.clone()
        } else {
            let mut joined = configured.as_str().trim_end_matches('/').to_string();
            joined.push_str("/packages.json");
            Url::parse(&joined).context("joining packages.json to the repository URL")?
        };
        let base_url = configured;

        let packages_cache_path = cache_dir.join("packages.json");
        let root = match get_cached_json(transport, &packages_url, &packages_cache_path).await? {
            CachedJson::NotFound => bail!("{packages_url}: not found"),
            CachedJson::Data(data) => data,
        };
        let requests = AtomicUsize::new(0);
        let requests = &requests;

        let provider = if let Some(metadata_url) = root.get("metadata-url").and_then(Value::as_str)
        {
            Provider::Lazy {
                metadata_url: metadata_url.to_string(),
            }
        } else if let Some(providers_url) = root.get("providers-url").and_then(Value::as_str) {
            let empty = Map::new();
            let provider_includes = root
                .get("provider-includes")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            let listing = load_provider_listing(
                transport,
                requests,
                &base_url,
                &cache_dir,
                provider_includes,
            )
            .await?;
            Provider::Providers {
                providers_url: providers_url.to_string(),
                listing,
            }
        } else if let Some(providers_lazy_url) =
            root.get("providers-lazy-url").and_then(Value::as_str)
        {
            Provider::Lazy {
                metadata_url: providers_lazy_url.to_string(),
            }
        } else {
            let packages =
                load_eager_packages(transport, requests, &base_url, &cache_dir, &root).await?;
            Provider::Eager { packages }
        };

        let inline_packages = if matches!(provider, Provider::Lazy { .. }) {
            root.get("packages")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default()
        } else {
            Map::new()
        };
        let notify_url = root
            .get("notify-batch")
            .or_else(|| root.get("notify"))
            .and_then(Value::as_str)
            .map(|url| canonicalize_url(&base_url, url));

        Ok(Source {
            base_url,
            provider,
            inline_packages,
            notify_url,
            cache_dir,
            filters,
        })
    }

    /// This source's versions for `name` (already lowercased), applying its
    /// own `notify_url` to whichever entries don't already carry one. `dev`
    /// only matters for `Provider::Lazy`, which is the only mechanism that
    /// splits dev out into a separate `~dev` file.
    async fn load_versions<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
        name: &str,
        dev: DevAcceptance,
    ) -> Result<Vec<PackageVersion>> {
        let mut versions = match &self.provider {
            Provider::Lazy { metadata_url } => {
                let mut versions = Vec::new();
                if dev.wants_non_dev() {
                    versions.extend(
                        self.fetch_lazy(transport, requests, metadata_url, name, false)
                            .await?,
                    );
                }
                if dev.wants_dev() {
                    versions.extend(
                        self.fetch_lazy(transport, requests, metadata_url, name, true)
                            .await?,
                    );
                }
                versions
            }
            Provider::Providers {
                providers_url,
                listing,
            } => {
                let Some(hash) = listing.get(name) else {
                    return Ok(Vec::new());
                };
                let path = providers_url
                    .replace("%package%", name)
                    .replace("%hash%", hash);
                let url = self
                    .base_url
                    .join(&path)
                    .with_context(|| format!("invalid providers-url substitution {path:?}"))?;
                let cache_path = self
                    .cache_dir
                    .join(format!("provider-{}.json", name.replace('/', "$")));
                let data = get_hash_verified_json(
                    transport,
                    requests,
                    &url,
                    &cache_path,
                    hash,
                    HashKind::Sha256,
                )
                .await?;
                parse_provider_versions(&data, name)?
            }
            Provider::Eager { packages } => packages.get(name).cloned().unwrap_or_default(),
        };
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
        Ok(versions)
    }

    async fn fetch_lazy<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
        metadata_url: &str,
        name: &str,
        dev_file: bool,
    ) -> Result<Vec<PackageVersion>> {
        if !dev_file && let Some(inline) = self.inline_packages.get(name) {
            return parse_inline_versions(name, inline);
        }
        let file_name = if dev_file {
            format!("{name}~dev")
        } else {
            name.to_string()
        };
        let path = metadata_url.replace("%package%", &file_name);
        let url = self
            .base_url
            .join(&path)
            .with_context(|| format!("invalid metadata-url substitution {path:?}"))?;
        let cache_path = self
            .cache_dir
            .join(format!("provider-{}.json", file_name.replace('/', "$")));
        requests.fetch_add(1, Ordering::Relaxed);
        match get_cached_json(transport, &url, &cache_path).await? {
            CachedJson::NotFound => Ok(Vec::new()),
            CachedJson::Data(data) => parse_provider_versions(&data, name),
        }
    }
}

/// `ComposerRepository::loadProviderListings`, breadth-first rather than
/// recursive (an included file may itself list further `provider-includes`):
/// each file is sha256-verified against the hash `provider-includes` named
/// it with, and only fetched at all when the cache doesn't already match
/// (`Cache::sha256`, immutable-content-addressed so a match can never be
/// stale). Cache key strips `%hash%` and `$` from the include's own key
/// (`ComposerRepository.php:1635`), since the real hash already lives in
/// the URL and would otherwise churn the cache on every release.
async fn load_provider_listing<T: Transport>(
    transport: &T,
    requests: &AtomicUsize,
    base_url: &Url,
    cache_dir: &Path,
    root_includes: &Map<String, Value>,
) -> Result<HashMap<String, String>> {
    let mut listing = HashMap::new();
    let mut queue: VecDeque<Map<String, Value>> = VecDeque::new();
    if !root_includes.is_empty() {
        queue.push_back(root_includes.clone());
    }
    while let Some(includes) = queue.pop_front() {
        for (include, metadata) in &includes {
            let sha256 = metadata
                .get("sha256")
                .and_then(Value::as_str)
                .with_context(|| format!("{include}: provider-includes entry missing sha256"))?;
            let path = include.replace("%hash%", sha256);
            let url = base_url
                .join(&path)
                .with_context(|| format!("invalid provider-includes path {path:?}"))?;
            let cache_key = include.replace("%hash%", "").replace('$', "");
            let cache_path = cache_dir.join(&cache_key);
            let data = get_hash_verified_json(
                transport,
                requests,
                &url,
                &cache_path,
                sha256,
                HashKind::Sha256,
            )
            .await?;
            if let Some(providers) = data.get("providers").and_then(Value::as_object) {
                for (name, entry) in providers {
                    if let Some(hash) = entry.get("sha256").and_then(Value::as_str) {
                        listing.insert(name.to_ascii_lowercase(), hash.to_string());
                    }
                }
            }
            if let Some(nested) = data.get("provider-includes").and_then(Value::as_object)
                && !nested.is_empty()
            {
                queue.push_back(nested.clone());
            }
        }
    }
    Ok(listing)
}

/// `ComposerRepository::loadIncludes`: eagerly loads every package this
/// source has, following `includes` breadth-first and merging every
/// `packages` map found along the way (the root file's own `packages` key
/// included, via `root` being the first item in the queue). Each include is
/// sha1-verified (`Cache::sha1`) the same content-addressed way
/// `provider-includes` is sha256-verified.
async fn load_eager_packages<T: Transport>(
    transport: &T,
    requests: &AtomicUsize,
    base_url: &Url,
    cache_dir: &Path,
    root: &Value,
) -> Result<HashMap<String, Vec<PackageVersion>>> {
    let mut packages: HashMap<String, Vec<PackageVersion>> = HashMap::new();
    let mut queue: VecDeque<Value> = VecDeque::new();
    queue.push_back(root.clone());
    while let Some(data) = queue.pop_front() {
        if let Some(obj) = data.get("packages").and_then(Value::as_object) {
            for (name, versions) in obj {
                let Some(versions) = versions.as_object() else {
                    continue;
                };
                let key = name.to_ascii_lowercase();
                let entry = packages.entry(key).or_default();
                for version in versions.values() {
                    entry.push(PackageVersion::from_value(version)?);
                }
            }
        }
        if let Some(includes) = data.get("includes").and_then(Value::as_object) {
            for (include, metadata) in includes {
                let url = base_url
                    .join(include)
                    .with_context(|| format!("invalid includes path {include:?}"))?;
                let cache_path = cache_dir.join(include.as_str());
                let included = if let Some(sha1) = metadata.get("sha1").and_then(Value::as_str) {
                    get_hash_verified_json(
                        transport,
                        requests,
                        &url,
                        &cache_path,
                        sha1,
                        HashKind::Sha1,
                    )
                    .await?
                } else {
                    requests.fetch_add(1, Ordering::Relaxed);
                    match transport.get(&url, None).await? {
                        Conditional::Fresh { body, .. } => serde_json::from_slice(&body)
                            .with_context(|| format!("{url}: not valid JSON"))?,
                        Conditional::NotFound => bail!("{url}: not found"),
                        Conditional::NotModified => {
                            bail!("{url}: unexpected 304 for an unconditional request")
                        }
                    }
                };
                queue.push_back(included);
            }
        }
    }
    Ok(packages)
}

/// Which digest a content-addressed v1 file is verified with: `sha256` for
/// `provider-includes`/`providers-url`, `sha1` for `includes`
/// (`Cache::sha256`/`Cache::sha1` upstream).
#[derive(Clone, Copy)]
enum HashKind {
    Sha1,
    Sha256,
}

fn digest_hex(bytes: &[u8], kind: HashKind) -> String {
    match kind {
        HashKind::Sha1 => hex(Sha1::digest(bytes)),
        HashKind::Sha256 => hex(Sha256::digest(bytes)),
    }
}

/// A content-addressed fetch: the cache key's expected hash is known up
/// front (unlike `get_cached_json`'s `Last-Modified` revalidation), so a
/// cache hit needs no transport call at all, and a miss is an unconditional
/// GET (`if_modified_since: None`) rather than a conditional one.
async fn get_hash_verified_json<T: Transport>(
    transport: &T,
    requests: &AtomicUsize,
    url: &Url,
    cache_path: &Path,
    expected_hash: &str,
    kind: HashKind,
) -> Result<Value> {
    if let Ok(bytes) = fs_err::read(cache_path)
        && digest_hex(&bytes, kind).eq_ignore_ascii_case(expected_hash)
    {
        return serde_json::from_slice(&bytes)
            .with_context(|| format!("{}: cached file is not valid JSON", cache_path.display()));
    }
    requests.fetch_add(1, Ordering::Relaxed);
    let body = match transport.get(url, None).await? {
        Conditional::Fresh { body, .. } => body,
        Conditional::NotFound => bail!("{url}: not found"),
        Conditional::NotModified => bail!("{url}: unexpected 304 for an unconditional request"),
    };
    if let Some(parent) = cache_path.parent() {
        fs_err::create_dir_all(parent)?;
    }
    fs_err::write(cache_path, &body)
        .with_context(|| format!("writing {}", cache_path.display()))?;
    serde_json::from_slice(&body).with_context(|| format!("{url}: not valid JSON"))
}

/// One `composer.json` `repositories[]` entry, already resolved to a
/// `"composer"`-type URL and its filters: everything else (`type`
/// dispatch, `packagist.org` defaulting/disabling) is settled by
/// [`parse_repositories`] before this is built.
#[derive(Debug)]
struct RepoEntry {
    url: String,
    filters: RepoFilters,
}

/// `RepositoryFactory::createRepos` + `Config::merge`'s `repositories`
/// defaulting, restricted to the one repository `type` this crate supports.
/// `composer.json`'s own repositories always win over the default
/// `packagist.org` (added last, i.e. lowest priority), matching Composer's
/// "explicit repos come first" ordering; `{"packagist.org": false}` (an
/// object key, or a single-key `{"name": false}` array entry) disables the
/// default without redeclaring it, and so does redefining a `composer`-type
/// repository whose URL already points at `packagist.org`
/// (`Config::merge`'s auto-deactivate).
fn parse_repositories(root: &Value) -> Result<Vec<RepoEntry>> {
    static PACKAGIST_URL_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^https?://(?:[a-z0-9-]+\.)?packagist\.org(/|$)").unwrap()
    });

    let mut disable_packagist = false;
    let mut entries = Vec::new();

    let disables_default = |name: &str| matches!(name, "packagist.org" | "packagist");

    if let Some(repos) = root.get("repositories") {
        match repos {
            Value::Object(map) => {
                for (name, repo) in map {
                    if repo == &Value::Bool(false) {
                        disable_packagist |= disables_default(name);
                        continue;
                    }
                    let entry = parse_repo_entry(name, repo)?;
                    disable_packagist |= PACKAGIST_URL_RE.is_match(&entry.url);
                    entries.push(entry);
                }
            }
            Value::Array(list) => {
                for repo in list {
                    if let Value::Object(map) = repo
                        && map.len() == 1
                        && let Some((name, Value::Bool(false))) = map.iter().next()
                    {
                        disable_packagist |= disables_default(name);
                        continue;
                    }
                    let entry = parse_repo_entry("(unnamed)", repo)?;
                    disable_packagist |= PACKAGIST_URL_RE.is_match(&entry.url);
                    entries.push(entry);
                }
            }
            Value::Null => {}
            _ => bail!("composer.json's \"repositories\" must be an array or object"),
        }
    }
    if !disable_packagist {
        entries.push(RepoEntry {
            url: PACKAGIST_URL.to_string(),
            filters: RepoFilters::default(),
        });
    }
    Ok(entries)
}

fn parse_repo_entry(name: &str, repo: &Value) -> Result<RepoEntry> {
    let obj = repo
        .as_object()
        .with_context(|| format!("repository {name:?} must be an object"))?;
    let repo_type = obj
        .get("type")
        .and_then(Value::as_str)
        .with_context(|| format!("repository {name:?} must have a \"type\""))?;
    if repo_type != "composer" {
        bail!(
            "repository {name:?}: type {repo_type:?} is not supported in vivace v0.1 (only \
             \"composer\" repositories; path/vcs/artifact are not supported)"
        );
    }
    let url = obj
        .get("url")
        .and_then(Value::as_str)
        .with_context(|| format!("repository {name:?} (type \"composer\") must have a \"url\""))?
        .to_string();
    let filters = RepoFilters::parse(obj, &url)?;
    Ok(RepoEntry { url, filters })
}

/// A loaded set of repositories: one or more `composer`-type sources,
/// consulted in priority order, plus a cache of fetched provider files.
pub struct Repository<T: Transport> {
    transport: T,
    sources: Vec<Source>,
    /// In-memory memoization keyed by lowercased name: a repeat
    /// `load_package`/`load_closure` call over names already loaded this
    /// run costs zero transport calls.
    loaded: Mutex<HashMap<String, Vec<PackageVersion>>>,
    /// Count of `transport.get` calls issued for a provider file (#55):
    /// every one of these is a real request, warm cache or not, for a
    /// `Last-Modified`-revalidated fetch — a warm metadata cache still
    /// revalidates, it just gets a 304 back instead of a body. A
    /// content-addressed (hash-verified) fetch is the exception: a cache
    /// hit there never calls `transport.get` at all, so it doesn't count.
    requests: AtomicUsize,
}

impl<T: Transport> Repository<T> {
    /// Load a single Packagist-shaped repository at `base_url`, with no
    /// filters and nothing else in front of or behind it. Used directly by
    /// callers that only ever talk to one repository (`require.rs`); `viv
    /// update`'s multi-repository construction goes through
    /// [`Repository::from_composer_json`] instead.
    pub async fn load(base_url: &str, cache_root: &Path, transport: T) -> Result<Repository<T>> {
        let source = Source::load(base_url, cache_root, &transport, RepoFilters::default()).await?;
        Ok(Repository {
            transport,
            sources: vec![source],
            loaded: Mutex::new(HashMap::new()),
            requests: AtomicUsize::new(0),
        })
    }

    /// Builds every source named in the root `composer.json`'s
    /// `repositories` (plus the implicit `packagist.org`, unless disabled),
    /// in priority order (`docs/resolver-design.md`'s Metadata section,
    /// extended by `#67` to more than one repository).
    pub async fn from_composer_json(
        root: &Value,
        cache_root: &Path,
        transport: T,
    ) -> Result<Repository<T>> {
        let entries = parse_repositories(root)?;
        let mut sources = Vec::with_capacity(entries.len());
        for entry in entries {
            sources.push(Source::load(&entry.url, cache_root, &transport, entry.filters).await?);
        }
        Ok(Repository {
            transport,
            sources,
            loaded: Mutex::new(HashMap::new()),
            requests: AtomicUsize::new(0),
        })
    }

    /// Provider-file requests issued so far (#55): excludes the initial
    /// `packages.json` fetch(es) and any name served from a cache/inline
    /// map without a request.
    pub fn request_count(&self) -> usize {
        self.requests.load(Ordering::Relaxed)
    }

    /// Fetch a package's versions across every source in priority order
    /// (`RepositorySet`/`PoolBuilder::loadPackagesMarkedForLoading`): a
    /// source whose `only`/`exclude` filter rejects `name` is skipped
    /// entirely; otherwise its versions are merged in, and if it's
    /// `canonical` (the default) and found at least one, no further,
    /// lower-priority source is even asked about this name — a
    /// non-canonical source's versions are added but never stop the
    /// search. A missing package everywhere is `Ok(vec![])`, not an error.
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
        for source in &self.sources {
            if !source.filters.allows(&key) {
                continue;
            }
            let found = source
                .load_versions(&self.transport, &self.requests, &key, dev)
                .await?;
            let canonical_hit = source.filters.canonical && !found.is_empty();
            versions.extend(found);
            if canonical_hit {
                break;
            }
        }
        self.loaded
            .lock()
            .expect("loaded mutex")
            .insert(key, versions.clone());
        Ok(versions)
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
/// v2/lazy provider file share: read a cached body and its `Last-Modified`,
/// send that back as `If-Modified-Since`, and cache a fresh response before
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

    #[test]
    fn names_to_regex_matches_globs_case_insensitively() {
        let re = names_to_regex(&["acme/*".to_string(), "foo/bar".to_string()]).unwrap();
        assert!(re.is_match("acme/anything"));
        assert!(re.is_match("FOO/BAR"));
        assert!(!re.is_match("other/pkg"));
    }

    #[test]
    fn repo_filters_only_and_exclude_are_mutually_exclusive() {
        let obj: Map<String, Value> =
            serde_json::from_str(r#"{"only": ["a/b"], "exclude": ["c/d"]}"#).unwrap();
        let err = RepoFilters::parse(&obj, "test").unwrap_err().to_string();
        assert!(err.contains("only one of"), "{err}");
    }

    #[test]
    fn parse_repositories_disables_default_packagist_by_name() {
        let root = serde_json::json!({"repositories": {"packagist.org": false}});
        let entries = parse_repositories(&root).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_repositories_disables_default_via_array_form() {
        let root = serde_json::json!({"repositories": [{"packagist.org": false}]});
        let entries = parse_repositories(&root).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn parse_repositories_appends_packagist_last_by_default() {
        let root = serde_json::json!({
            "repositories": [{"type": "composer", "url": "https://satis.example/"}],
        });
        let entries = parse_repositories(&root).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].url, "https://satis.example/");
        assert_eq!(entries[1].url, PACKAGIST_URL);
    }

    #[test]
    fn parse_repositories_rejects_unsupported_type() {
        let root = serde_json::json!({
            "repositories": [{"type": "vcs", "url": "https://github.com/acme/pkg"}],
        });
        let err = parse_repositories(&root).unwrap_err().to_string();
        assert!(err.contains("vcs"), "{err}");
    }
}
