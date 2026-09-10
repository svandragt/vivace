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
//! Skipped, with a clear error where it matters: `providers-api`,
//! `security-advisories`, and `path`/`artifact` repositories are not
//! supported (a repository whose `type` isn't `"composer"`, `"vcs"`, `"git"`
//! or `"github"` is rejected; a `"composer"` repository missing every
//! provider mechanism below is treated as empty rather than erroring,
//! matching `whatProvides`'s own `return []`).
//! `"vcs"`/`"git"`/`"github"` repositories are handled by
//! [`crate::vcs`] (`VcsRepository`/`Vcs\GitDriver`/`Vcs\GitHubDriver`);
//! every other VCS driver (GitLab, Bitbucket, Forgejo, Mercurial,
//! Perforce, Fossil, SVN) is not supported.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use futures::stream::{FuturesUnordered, StreamExt};
use regex::Regex;
use reqwest::Url;
use serde_json::{Map, Value};
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::fetch::{Conditional, Fetcher};
use crate::semver::{self, Constraint};
use crate::solver::pool_builder::link_constraint_text;
use crate::solver::{ConstraintCache, parse_constraint_cached};
use crate::store::hex;
use crate::vcs;

/// `Config::$defaultRepositories`: the implicit last (lowest-priority)
/// repository, unless `composer.json` disables it (`packagist.org: false`)
/// or redefines a `composer`-type repository at this same URL.
pub const PACKAGIST_URL: &str = "https://repo.packagist.org";

/// `PoolBuilder::LOAD_BATCH_SIZE`: names are loaded breadth-first in waves
/// of this many concurrent fetches, now also the cap on
/// [`Repository::load_closure_seeded`]'s seed burst (#120). Packagist
/// advertises `SETTINGS_MAX_CONCURRENT_STREAMS: 128` on its pooled HTTP/2
/// connection; above that, requests queue client-side and per-request
/// latency climbs, so this needs to sit comfortably under 128 without
/// leaving the connection under-used. Swept 24/40/64/100 on warm caches, 3
/// runs each, median closure `elapsed_ms` (`RUST_LOG=vivace=debug`):
///
/// | cap | symfony/demo (160 names) | bench/laravel (~110 names) |
/// |----:|-------------------------:|----------------------------:|
/// |  24 |                    1611  |                        736  |
/// |  40 |                    1667  |                        737  |
/// |  64 |                    1577  |                        701  |
/// | 100 |                    1387  |                        644  |
///
/// 100 won on both: fewer, larger waves beat more, smaller ones as long as
/// the connection stays under its stream limit (100 vs the unpatched
/// all-at-once seed burst, 153 streams on one connection: 1692 median on
/// symfony/demo, a ~18% regression from queuing past the limit). A second
/// connection (`pool_max_idle_per_host(2)`) at the same cap made no
/// measurable difference — expected, since 100 in flight never needs a
/// second connection to begin with — so it wasn't worth the extra client
/// plumbing.
const LOAD_BATCH_SIZE: usize = 100;

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
    /// `pub(crate)`: [`crate::vcs`] builds a provider-file-shaped `Value`
    /// from a VCS ref's `composer.json` (`name`/`version`/
    /// `version_normalized`/`dist`/`source`/`time` overridden onto the
    /// parsed file) and feeds it
    /// through this same constructor rather than duplicating its field
    /// extraction.
    pub(crate) fn from_value(raw: &Value) -> Result<Self> {
        Self::from_owned_value(raw.clone())
    }

    /// Same field extraction as [`PackageVersion::from_value`], but takes
    /// `raw` by value so a caller that already owns it (freshly
    /// deserialized JSON, never aliased elsewhere: #120) moves it into the
    /// `raw` field instead of cloning the whole document a second time on
    /// top of the per-field clones just below — one clone dropped per
    /// version instead of two, and Packagist's own provider files run every
    /// version through this on every closure fetch.
    pub(crate) fn from_owned_value(raw: Value) -> Result<Self> {
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
            None => {
                VERSION_NORMALIZE_CALLS.fetch_add(1, Ordering::Relaxed);
                crate::version::normalize(&version)?
            }
        };
        let require = map_field(obj, "require");
        let require_dev = map_field(obj, "require-dev");
        let replace = map_field(obj, "replace");
        let provide = map_field(obj, "provide");
        let conflict = map_field(obj, "conflict");
        let default_branch = obj
            .get("default-branch")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let dist = obj.get("dist").cloned();
        let source = obj.get("source").cloned();
        let branch_alias = obj
            .get("extra")
            .and_then(|extra| extra.get("branch-alias"))
            .cloned();
        let time = obj.get("time").and_then(Value::as_str).map(str::to_string);
        Ok(PackageVersion {
            name,
            version,
            version_normalized,
            require,
            require_dev,
            replace,
            provide,
            conflict,
            default_branch,
            dist,
            source,
            branch_alias,
            time,
            raw,
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

/// `ComposerRepository::$hasAvailablePackageList`/`$availablePackages`/
/// `$availablePackagePatterns` (#119): a v2 (`metadata-url`) source's root
/// `packages.json` can name (`available-packages`) or glob-match
/// (`available-package-patterns`, compiled the same way `only`/`exclude` are
/// — `names_to_regex` doubles as `BasePackage::packageNameToRegexp` here)
/// every package it can possibly answer for; a name outside both is never
/// even asked about (`findPackage`/`findPackages`'s own
/// `lazyProvidersRepoContains` short-circuit,
/// `ComposerRepository.php:281-283`/`326-328`), so `wpackagist`'s real
/// mirror (`repo.wp-packages.org`, `available-package-patterns:
/// ["wp-plugin/*", "wp-theme/*"]`) never costs a `/p2/%package%.json`
/// request for a non-WordPress name. Only parsed for the `metadata-url`
/// protocol (`ComposerRepository.php:1499-1531`): the older
/// `providers-lazy-url`-only branch never sets `hasAvailablePackageList`,
/// and classic `providers-url` doesn't need this at all — its
/// `provider-includes` listing already gates a per-name miss for free
/// (`Provider::Providers`'s `listing.get`).
struct AvailablePackages {
    /// Lowercased verbatim names (`available-packages`).
    names: HashSet<String>,
    patterns: Option<Regex>,
}

impl AvailablePackages {
    /// `None` when `root` declares neither key (a source with no available
    /// list behaves as today: every name is worth asking about).
    fn parse(root: &Value) -> Result<Option<AvailablePackages>> {
        // `!empty($data[...])`: absent, `null`, or an empty array are all
        // "not provided", matching every other falsy-array PHP root field
        // this crate already treats the same way (`notify-batch`, `search`).
        let string_list = |key: &str| -> Result<Option<Vec<String>>> {
            match root.get(key) {
                None | Some(Value::Null) => Ok(None),
                Some(Value::Array(items)) if items.is_empty() => Ok(None),
                Some(Value::Array(items)) => Ok(Some(
                    items
                        .iter()
                        .map(|v| {
                            v.as_str()
                                .map(str::to_string)
                                .with_context(|| format!("{key:?} entries must be strings"))
                        })
                        .collect::<Result<Vec<_>>>()?,
                )),
                Some(_) => bail!("{key:?} must be an array of package names"),
            }
        };
        let names = string_list("available-packages")?;
        let patterns = string_list("available-package-patterns")?;
        if names.is_none() && patterns.is_none() {
            return Ok(None);
        }
        Ok(Some(AvailablePackages {
            names: names
                .unwrap_or_default()
                .into_iter()
                .map(|n| n.to_ascii_lowercase())
                .collect(),
            patterns: patterns.as_deref().map(names_to_regex).transpose()?,
        }))
    }

    fn allows(&self, name: &str) -> bool {
        self.names.contains(name) || self.patterns.as_ref().is_some_and(|p| p.is_match(name))
    }
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
struct ComposerSource {
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
    /// `available-packages`/`available-package-patterns` (#119), parsed only
    /// for the `metadata-url` protocol ([`AvailablePackages`]'s own doc).
    available: Option<AvailablePackages>,
    cache_dir: PathBuf,
}

impl ComposerSource {
    /// Fetch `packages.json` from `url` and set up this source's provider
    /// mechanism, caching under `<cache_root>/repo/<repo-host>/`
    /// (`ComposerRepository::getCache`'s per-repo directory, keyed the same
    /// way this crate already keyed a single Packagist repo).
    async fn load<T: Transport>(
        url: &str,
        cache_root: &Path,
        transport: &T,
    ) -> Result<ComposerSource> {
        let configured =
            Url::parse(url).with_context(|| format!("invalid repository URL {url:?}"))?;
        // A `file://` mirror (a local Satis build or a recorded Packagist
        // mirror, #161) has no host to key the cache directory by; its path
        // is already a unique filesystem location, so slugify that instead,
        // the same way `vcs::GitDriver` keys a bare/`git@` remote that has
        // no host either.
        let host = if configured.scheme() == "file" {
            vcs::slugify(configured.path())
        } else {
            configured
                .host_str()
                .with_context(|| format!("repository URL {configured} has no host"))?
                .to_string()
        };
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

        // `available-packages`/`available-package-patterns` only take effect
        // for the `metadata-url` protocol (`AvailablePackages`'s own doc
        // comment); parsed here, alongside the branch that reads
        // `metadata-url` itself, rather than unconditionally below.
        let mut available = None;
        let provider = if let Some(metadata_url) = root.get("metadata-url").and_then(Value::as_str)
        {
            available = AvailablePackages::parse(&root)?;
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

        Ok(ComposerSource {
            base_url,
            provider,
            inline_packages,
            notify_url,
            available,
            cache_dir,
        })
    }

    /// `ComposerRepository::lazyProvidersRepoContains`: `true` when this
    /// source has no `available-packages`/`available-package-patterns` list
    /// at all (every name is worth asking about, today's behaviour), or when
    /// `name` is on/matches the one it does have.
    fn allows(&self, name: &str) -> bool {
        self.available.as_ref().is_none_or(|a| a.allows(name))
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
                parse_provider_versions(data, name).await?
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
            CachedJson::Data(data) => parse_provider_versions(data, name).await,
        }
    }
}

/// One entry from `composer.json`'s `repositories[]`, loaded: either a
/// `"composer"`-type [`ComposerSource`], or a `"vcs"`/`"git"`/`"github"`
/// [`vcs::VcsSource`] (one package, whose name is resolved lazily —
/// [`Source::load_versions`] asks the VCS source for `name`'s versions and
/// gets back an empty list until `name` matches the package the VCS repo
/// actually holds).
struct Source {
    filters: RepoFilters,
    kind: SourceKind,
}

enum SourceKind {
    Composer(ComposerSource),
    Vcs(vcs::VcsSource),
}

impl Source {
    async fn load<T: Transport>(
        entry: RepoEntry,
        cache_root: &Path,
        transport: &T,
    ) -> Result<Source> {
        let kind = match &entry.kind {
            RepoKind::Composer => {
                SourceKind::Composer(ComposerSource::load(&entry.url, cache_root, transport).await?)
            }
            RepoKind::Vcs { repo_type } => SourceKind::Vcs(
                vcs::VcsSource::load(&entry.url, repo_type, cache_root, transport).await?,
            ),
        };
        Ok(Source {
            filters: entry.filters,
            kind,
        })
    }

    /// Whether this source is worth asking about `name` (already lowercased)
    /// at all: `only`/`exclude` (`RepoFilters`), layered with a `"composer"`
    /// source's own `available-packages`/`available-package-patterns`
    /// (`ComposerSource::allows`, #119). A VCS source has neither of the
    /// latter — it's always worth asking, since it only ever holds the one
    /// package it was configured for.
    fn allows(&self, name: &str) -> bool {
        if !self.filters.allows(name) {
            return false;
        }
        match &self.kind {
            SourceKind::Composer(source) => source.allows(name),
            SourceKind::Vcs(_) => true,
        }
    }

    /// This source's versions for `name` (already lowercased); `dev` only
    /// affects a `"composer"` source (`ComposerSource::load_versions`'s own
    /// doc comment). A VCS source ignores it: it has no separate dev file,
    /// and returns either the one package it holds (every version) or
    /// nothing, depending on whether `name` matches that package.
    async fn load_versions<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
        name: &str,
        dev: DevAcceptance,
    ) -> Result<Vec<PackageVersion>> {
        match &self.kind {
            SourceKind::Composer(source) => {
                source.load_versions(transport, requests, name, dev).await
            }
            SourceKind::Vcs(source) => source.load_versions(transport, requests, name).await,
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
    if let Ok(bytes) = fs_err::read(cache_path) {
        let context = format!("{}: cached file is not valid JSON", cache_path.display());
        let verify = |bytes: &[u8]| {
            digest_hex(bytes, kind)
                .eq_ignore_ascii_case(expected_hash)
                .then(|| serde_json::from_slice::<Value>(bytes))
        };
        let verified = if bytes.len() <= INLINE_PARSE_MAX_BYTES {
            verify(&bytes)
        } else {
            let expected_hash = expected_hash.to_string();
            tokio::task::spawn_blocking(move || {
                digest_hex(&bytes, kind)
                    .eq_ignore_ascii_case(&expected_hash)
                    .then(|| serde_json::from_slice::<Value>(&bytes))
            })
            .await
            .context("cache verify task panicked")?
        };
        if let Some(parsed) = verified {
            return parsed.with_context(|| context);
        }
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
    parse_json_blocking(body, format!("{url}: not valid JSON")).await
}

/// One `composer.json` `repositories[]` entry, resolved to a supported
/// `type` and its filters: `packagist.org` defaulting/disabling is settled
/// by [`parse_repositories`] before this is built.
#[derive(Debug)]
struct RepoEntry {
    url: String,
    filters: RepoFilters,
    kind: RepoKind,
}

/// Which loader a [`RepoEntry`] needs: a `"composer"`-type `packages.json`
/// source, or a VCS one, keyed by the `type` string as written in
/// `composer.json` (`"vcs"`, `"git"` or `"github"`) since that's what
/// decides which driver [`vcs::VcsSource::load`] picks — `"git"`/`"github"`
/// force a driver outright the way Composer's own `$this->drivers[$type]`
/// does, `"vcs"` autodetects.
#[derive(Debug)]
enum RepoKind {
    Composer,
    Vcs { repo_type: String },
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
            kind: RepoKind::Composer,
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
    let kind = match repo_type {
        "composer" => RepoKind::Composer,
        "vcs" | "git" | "github" => RepoKind::Vcs {
            repo_type: repo_type.to_string(),
        },
        _ => bail!(
            "repository {name:?}: type {repo_type:?} is not supported (composer, vcs and git \
             repositories are)"
        ),
    };
    let url = obj
        .get("url")
        .and_then(Value::as_str)
        .with_context(|| format!("repository {name:?} (type {repo_type:?}) must have a \"url\""))?
        .to_string();
    let filters = RepoFilters::parse(obj, &url)?;
    Ok(RepoEntry { url, filters, kind })
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
        let entry = RepoEntry {
            url: base_url.to_string(),
            filters: RepoFilters::default(),
            kind: RepoKind::Composer,
        };
        let source = Source::load(entry, cache_root, &transport).await?;
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
            sources.push(Source::load(entry, cache_root, &transport).await?);
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
            if !source.allows(&key) {
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

    /// `load_package(&name, dev)`, but keyed by its own `name`: a free
    /// function so both the seed wave and the BFS fill loop in
    /// [`Repository::load_closure_seeded`] push the *same* concrete future
    /// type into one `FuturesUnordered` (two `async move` blocks written at
    /// different call sites, even identical ones, are distinct types).
    async fn fetch_named(
        &self,
        name: String,
        dev: DevAcceptance,
    ) -> (String, Result<Vec<PackageVersion>>) {
        let versions = self.load_package(&name, dev).await;
        (name, versions)
    }

    /// Breadth-first metadata load, up to `LOAD_BATCH_SIZE` concurrent
    /// fetches in flight at any time (`PoolBuilder::LOAD_BATCH_SIZE`'s
    /// concurrency figure, not its wave-by-wave batching: #90 found waiting
    /// for a whole wave to land before starting the next one serialises one
    /// round trip per BFS level for no reason). Discovers further names from
    /// each *loaded* version's `require` only (`is_acceptable`/`accept`); a
    /// version outside the accumulated constraint or stability for its name
    /// never contributes its own requires, and never lands in the returned
    /// closure (#90: this is the difference between fetching Composer's own
    /// discovery breadth and every version of every name reachable at all).
    /// Platform packages are never queued
    /// (`PlatformRepository::isPlatformPackage`).
    pub async fn load_closure(
        &self,
        roots: &[ClosureRoot<'_>],
        dev: DevAcceptance,
        accept: &dyn Fn(&str, &str) -> bool,
        constraint_cache: &mut ConstraintCache,
    ) -> Result<HashMap<String, Vec<PackageVersion>>> {
        self.load_closure_seeded(roots, dev, &HashSet::new(), &[], accept, constraint_cache)
            .await
    }

    /// Same breadth-first closure walk as [`Repository::load_closure`], but
    /// a name in `skip` (already lowercased) is never fetched or queued:
    /// `PoolBuilder::loadPackage`'s `if (isset($this->loadedPackages[$name]))
    /// continue;` fast path, which is how a partial update's locked-out
    /// packages (`solver::pool_builder::build_partial`, already loaded
    /// straight from the lock and effectively carrying a `MatchAllConstraint`)
    /// stop the closure walk from re-fetching them or their own
    /// requirements' remote alternatives.
    pub async fn load_closure_skipping(
        &self,
        roots: &[ClosureRoot<'_>],
        dev: DevAcceptance,
        skip: &HashSet<String>,
        accept: &dyn Fn(&str, &str) -> bool,
        constraint_cache: &mut ConstraintCache,
    ) -> Result<HashMap<String, Vec<PackageVersion>>> {
        self.load_closure_seeded(roots, dev, skip, &[], accept, constraint_cache)
            .await
    }

    /// Same walk as [`Repository::load_closure_skipping`], but every
    /// (already lowercased) name in `seed` queues alongside `roots`' own
    /// names, rather than waiting to be discovered through a require chain
    /// (#90: a warm `viv update` re-walks a closure that's almost always the
    /// previous `composer.lock` again, so seeding with that lock's package
    /// names turns the BFS's ~9 sequential round-trip levels into ~2 —
    /// everything the lock already knew about queues from the first wave,
    /// dispatched `LOAD_BATCH_SIZE` at a time same as any other name (#120:
    /// firing every seed at once overran Packagist's HTTP/2 stream limit),
    /// and only genuinely new names still wait on a parent's response). A
    /// seed is a *prefetch*, never a pool
    /// change: its fetch is started and cached here, but it only contributes
    /// versions to the returned closure if the walk below actually reaches
    /// it from `roots` with a constraint some of its versions satisfy; a
    /// seed name no longer required by `roots` (removed from
    /// `composer.json`) is fetched for nothing and then dropped, exactly as
    /// if it had never been seeded. `skip` wins over `seed`: a locked-out
    /// name is never fetched either way.
    ///
    /// `accept(name, stability)` is `PoolBuilder`'s stability filter
    /// (`StabilityFilter::isPackageAcceptable`, already folding in
    /// `minimum-stability` and any per-package `@stability` flag): only a
    /// version whose own stability, or its branch alias's, passes this
    /// *and* matches some constraint every requirer of `name` has
    /// contributed is loaded at all (`ComposerRepository::isVersionAcceptable`)
    /// — this is what narrows the closure to Composer's own discovery
    /// breadth (#90) rather than every stability-acceptable version of
    /// every reachable name. `constraint_cache` is the same
    /// `solver::ConstraintCache` `pool_builder` parses `Link`s through, so a
    /// require string repeated across thousands of versions parses once.
    pub async fn load_closure_seeded(
        &self,
        roots: &[ClosureRoot<'_>],
        dev: DevAcceptance,
        skip: &HashSet<String>,
        seed: &[String],
        accept: &dyn Fn(&str, &str) -> bool,
        constraint_cache: &mut ConstraintCache,
    ) -> Result<HashMap<String, Vec<PackageVersion>>> {
        let closure_started = Instant::now();
        let requests_before = self.request_count();
        let files_before = CACHE_FILES_PARSED.load(Ordering::Relaxed);
        let bytes_before = CACHE_BYTES_PARSED.load(Ordering::Relaxed);
        let read_ns_before = STAGE_READ_NS.load(Ordering::Relaxed);
        let json_parse_ns_before = STAGE_JSON_PARSE_NS.load(Ordering::Relaxed);
        let expand_ns_before = STAGE_EXPAND_NS.load(Ordering::Relaxed);
        let convert_ns_before = STAGE_CONVERT_NS.load(Ordering::Relaxed);
        let versions_before = VERSIONS_PRODUCED.load(Ordering::Relaxed);
        let normalize_calls_before = VERSION_NORMALIZE_CALLS.load(Ordering::Relaxed);

        let mut walk = ClosureWalk {
            skip,
            accept,
            constraint_cache,
            states: HashMap::new(),
            versions_by_name: HashMap::new(),
            stashed: HashMap::new(),
            queue: VecDeque::new(),
            result: HashMap::new(),
        };
        // `PoolBuilder::buildPool`'s `foreach ($request->getRequires() ...)`
        // loop plus `maxExtendedReqs`: every root require/require-dev
        // (`build_partial_seeded` passes locked-out packages' own requires
        // as a second `ClosureRoot` here too, per this port's
        // simplification of `getFixedOrLockedPackages`) is marked with
        // exactly its own constraint, and that mark never widens no matter
        // what a later-discovered dependant requires of the same name.
        for root in roots {
            for (name, value) in root.require.iter().chain(root.require_dev.iter()) {
                let text = value
                    .as_str()
                    .with_context(|| format!("require {name}: constraint is not a string"))?;
                walk.discover(name, text, true)?;
            }
        }

        // Waves used to be collected wholesale (`collect().await` on the
        // whole batch) before starting the next one, so one slow response in
        // a wave delayed every discovery it would otherwise have unblocked,
        // stacking round-trip latency once per BFS *level* rather than once
        // per critical-path *edge* (#90: ~9 levels serialised even though no
        // level filled all `LOAD_BATCH_SIZE` slots). `FuturesUnordered` keeps
        // up to `LOAD_BATCH_SIZE` fetches in flight at all times and queues a
        // name's own requires the moment *that* fetch lands, so an
        // independent branch's fetch starts as soon as a slot frees up
        // instead of waiting for the slowest sibling in its wave.
        let mut in_flight = FuturesUnordered::new();

        // A seed name queues exactly like a discovered one (#120: firing all
        // of them at once — 153 on symfony/demo — blew past Packagist's
        // 128-stream HTTP/2 limit and the excess queued client-side at
        // rising per-request latency). `prefetching` tracks a name from the
        // moment its fetch is actually dispatched (below) to the moment its
        // future lands, so a name reached twice — once as a seed, once
        // through a require chain — is never queued for a second fetch.
        let mut prefetching: HashSet<String> = HashSet::new();
        for name in seed {
            let name = name.to_ascii_lowercase();
            if skip.contains(&name) || is_platform_package(&name) {
                continue;
            }
            walk.queue.push_back(name);
        }

        let mut waves = 0;
        loop {
            let was_empty = in_flight.is_empty();
            while in_flight.len() < LOAD_BATCH_SIZE {
                let Some(name) = walk.queue.pop_front() else {
                    break;
                };
                if !prefetching.insert(name.clone()) {
                    // Already dispatched (a seed name duplicated in `seed`
                    // itself, or a seed name also queued by `discover`
                    // through a require chain); its completion lands via
                    // `ClosureWalk::land`, which finds `name` already
                    // discovered and folds it in without a second fetch.
                    continue;
                }
                in_flight.push(self.fetch_named(name, dev));
            }
            if was_empty && !in_flight.is_empty() {
                waves += 1;
            }
            let Some((name, versions)) = in_flight.next().await else {
                break;
            };
            let versions = versions?;
            prefetching.remove(&name);
            walk.land(name, versions)?;
        }
        tracing::debug!(
            packages = walk.result.len(),
            requests = self.request_count() - requests_before,
            waves,
            files_parsed = CACHE_FILES_PARSED.load(Ordering::Relaxed) - files_before,
            bytes_parsed = CACHE_BYTES_PARSED.load(Ordering::Relaxed) - bytes_before,
            elapsed_ms = closure_started.elapsed().as_millis(),
            "loaded metadata closure"
        );
        tracing::debug!(
            read_ms = (STAGE_READ_NS.load(Ordering::Relaxed) - read_ns_before) / 1_000_000,
            json_parse_ms =
                (STAGE_JSON_PARSE_NS.load(Ordering::Relaxed) - json_parse_ns_before) / 1_000_000,
            expand_ms = (STAGE_EXPAND_NS.load(Ordering::Relaxed) - expand_ns_before) / 1_000_000,
            convert_ms = (STAGE_CONVERT_NS.load(Ordering::Relaxed) - convert_ns_before) / 1_000_000,
            versions_produced = VERSIONS_PRODUCED.load(Ordering::Relaxed) - versions_before,
            normalize_calls =
                VERSION_NORMALIZE_CALLS.load(Ordering::Relaxed) - normalize_calls_before,
            "#176: read+parse cached metadata stage split"
        );
        Ok(walk.result)
    }
}

/// Per-name bookkeeping for [`ClosureWalk`]: `PoolBuilder`'s
/// `$loadedPackages`/`$packagesToLoad` constraint tracking, minus
/// `Intervals::isSubsetOf` (#90's design note: a plain "any of these
/// matches" `Vec`, deduped by constraint *text* rather than by interval
/// subset, is enough — the union semantics are what narrows the closure,
/// the subset check is only Composer's own optimisation against re-parsing
/// work this port already avoids via `constraint_cache`).
struct NameState {
    /// Constraint text already folded in, so a require repeating the exact
    /// same string (extremely common: `"php": "^7.2.5 || ^8.0.0"` across
    /// thousands of versions) doesn't grow `constraints` or trigger a
    /// rescan for nothing.
    texts: HashSet<String>,
    /// Every distinct constraint text's parsed form; a version is loaded if
    /// it matches *any* of these (`MultiConstraint::create([...], false)`,
    /// composer's own union-not-intersection semantics for "two packages
    /// require the same dependency differently").
    constraints: Vec<Arc<Constraint>>,
    /// `PoolBuilder::maxExtendedReqs`: once true (a root require, or —
    /// per this port's simplification — either `ClosureRoot`), no later
    /// discovery ever extends `constraints` past its first entry.
    locked: bool,
    /// `version_normalized` values already folded into `result[name]`
    /// (`ComposerRepository::loadAsyncPackages`'s `$alreadyLoaded`), so a
    /// widened constraint only rescans genuinely new candidates rather than
    /// reprocessing (and re-queueing the requires of) versions already
    /// loaded under a narrower one.
    scanned: HashSet<String>,
    /// How many of `constraints`'s entries every not-yet-`scanned` version
    /// has already been tested against. A widen only appends to
    /// `constraints` (union/OR semantics: an already-failed constraint never
    /// starts passing), so [`ClosureWalk::process`] only needs to test a
    /// still-rejected version against the *new* tail on each rescan rather
    /// than the whole growing `Vec` again — without this, a heavily
    /// required package (`symfony/http-foundation` at 38 distinct requirer
    /// constraints on statamic/statamic) re-tests every one of its rejected
    /// versions against every constraint on every widen, an O(versions ×
    /// widens²) cost that measured as `ClosureWalk::process`'s single
    /// largest contributor to the closure's serial time.
    constraints_tested: usize,
}

/// The constraint-narrowed breadth-first walk [`Repository::load_closure_seeded`]
/// drives. Not `Send`/shared across the async fetches themselves — every
/// method here is synchronous bookkeeping the driving loop calls between
/// `.await` points, on data already in hand (a landed fetch's full,
/// unfiltered version list, or a require's own constraint text).
struct ClosureWalk<'a> {
    skip: &'a HashSet<String>,
    accept: &'a dyn Fn(&str, &str) -> bool,
    constraint_cache: &'a mut ConstraintCache,
    states: HashMap<String, NameState>,
    /// A name's full, unfiltered fetch result, once landed
    /// (`ComposerRepository::loadAsyncPackages`'s own per-repo cache of the
    /// raw provider file: `Repository::load_package` already fetches every
    /// version over the wire, this just remembers them across a later
    /// widen so a widen never re-fetches).
    versions_by_name: HashMap<String, Vec<PackageVersion>>,
    /// A landed fetch for a name not yet discovered (a seed prefetch that
    /// raced ahead of the require chain reaching it, or one `roots` never
    /// actually needs): held here until [`ClosureWalk::discover`] reaches
    /// it, or dropped unread if the walk finishes first.
    stashed: HashMap<String, Vec<PackageVersion>>,
    queue: VecDeque<String>,
    result: HashMap<String, Vec<PackageVersion>>,
}

impl ClosureWalk<'_> {
    /// `PoolBuilder::markPackageNameForLoading`: `name` (as some requirer's
    /// link target) must now also satisfy `text`. Platform packages,
    /// `__root__` and a `skip`-listed name (already loaded straight from
    /// the lock, `MatchAllConstraint`-equivalent) are a no-op. A brand-new
    /// name is queued for fetching, unless a seed prefetch already
    /// [`ClosureWalk::land`]ed it — then its stash is claimed straight into
    /// `versions_by_name` instead of fetching it twice. Either way, once
    /// `name`'s constraint set actually grows (new, or a genuinely new
    /// constraint text on one already `versions_by_name`-resident),
    /// [`ClosureWalk::process`] rescans it for newly matching versions.
    fn discover(&mut self, name: &str, text: &str, locked: bool) -> Result<()> {
        let name = name.to_ascii_lowercase();
        if is_platform_package(&name) || name == "__root__" || self.skip.contains(&name) {
            return Ok(());
        }
        let constraint = parse_constraint_cached(self.constraint_cache, text)?;
        let grew = match self.states.get_mut(&name) {
            None => {
                self.states.insert(
                    name.clone(),
                    NameState {
                        texts: std::iter::once(text.to_string()).collect(),
                        constraints: vec![constraint],
                        locked,
                        scanned: HashSet::new(),
                        constraints_tested: 0,
                    },
                );
                if let Some(versions) = self.stashed.remove(&name) {
                    self.versions_by_name.insert(name.clone(), versions);
                } else {
                    self.queue.push_back(name.clone());
                }
                true
            }
            Some(state) => {
                if state.locked || !state.texts.insert(text.to_string()) {
                    false
                } else {
                    state.constraints.push(constraint);
                    true
                }
            }
        };
        if grew {
            self.process(&name)?;
        }
        Ok(())
    }

    /// Rescans `name`'s landed (`versions_by_name`) versions for ones not
    /// yet in `scanned` that now match its current constraint set, folds
    /// each into `result`, and [`ClosureWalk::discover`]s its own
    /// `require` links in turn (`PoolBuilder::loadPackage`'s own walk over
    /// `$package->getRequires()`, done once per *version*, run here once
    /// per newly accepted [`PackageVersion`]). A no-op if `name` hasn't
    /// landed yet, or has landed but nothing new is accepted.
    fn process(&mut self, name: &str) -> Result<()> {
        let mut newly_matched = Vec::new();
        if let Some(versions) = self.versions_by_name.get(name) {
            // Only the constraints added since this name's last scan: an
            // already-rejected version was already tested against every
            // earlier one (union/OR semantics mean that verdict can't
            // change), so re-testing the whole `Vec` on every widen would
            // be wasted work that grows with the number of widens. Only
            // advance `constraints_tested` here, once versions actually
            // exist to test against — a `discover()`-triggered `process`
            // ahead of the fetch landing must leave it untouched, or
            // `land`'s later call would see nothing left to test at all.
            let tested = self.states[name].constraints_tested;
            let new_constraints = &self.states[name].constraints[tested..];
            for pv in versions {
                if self.states[name].scanned.contains(&pv.version_normalized) {
                    continue;
                }
                if is_version_loaded(pv, name, new_constraints, self.accept)? {
                    newly_matched.push(pv.clone());
                }
            }
            let state = self.states.get_mut(name).expect("marked before process");
            state.constraints_tested = state.constraints.len();
        }
        if newly_matched.is_empty() {
            return Ok(());
        }
        let state = self.states.get_mut(name).expect("marked before process");
        for pv in &newly_matched {
            state.scanned.insert(pv.version_normalized.clone());
        }
        for pv in &newly_matched {
            for (req_name, value) in &pv.require {
                let Some(raw) = value.as_str() else {
                    continue;
                };
                let text = link_constraint_text(&pv.version, raw);
                self.discover(req_name, text, false)?;
            }
        }
        self.result
            .entry(name.to_string())
            .or_default()
            .extend(newly_matched);
        Ok(())
    }

    /// A fetch has landed: `name` reached via `roots`/a require chain
    /// already has a [`NameState`] to filter against, so store its raw
    /// versions and [`ClosureWalk::process`] them immediately; otherwise
    /// it's a seed prefetch racing ahead of discovery, so [`stash`
    /// it][`ClosureWalk::stashed`] for `discover` to claim later.
    fn land(&mut self, name: String, versions: Vec<PackageVersion>) -> Result<()> {
        if self.states.contains_key(&name) {
            self.versions_by_name.insert(name.clone(), versions);
            self.process(&name)
        } else {
            self.stashed.insert(name, versions);
            Ok(())
        }
    }
}

/// `ComposerRepository::isVersionAcceptable`: `pv` is loaded if either its
/// `version_normalized` or its branch alias (`branch_alias_target`) is both
/// stability-acceptable for `name` (`accept`) and matches at least one of
/// `constraints` — the same version can pass on its alias even when its own
/// (`dev-*`) stability wouldn't, which is why a plain `is_acceptable` check
/// on `pv.version_normalized` alone isn't enough.
fn is_version_loaded(
    pv: &PackageVersion,
    name: &str,
    constraints: &[Arc<Constraint>],
    accept: &dyn Fn(&str, &str) -> bool,
) -> Result<bool> {
    let mut candidates = vec![pv.version_normalized.clone()];
    candidates.extend(branch_alias_target(pv));
    for candidate in candidates {
        // Both candidates are already in `semver::normalize`'s canonical
        // form (`version_normalized` straight from the provider file, or
        // `branch_alias_target`'s own `normalize_branch` output): wrap
        // rather than re-run the regex pipeline on a value that would only
        // parse back to itself (#120).
        let normalized = semver::from_normalized(candidate)?;
        if !accept(name, semver::stability(normalized.as_str())) {
            continue;
        }
        if constraints.iter().any(|c| c.matches(&normalized)) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// `ArrayLoader::getBranchAlias`: `extra.branch-alias` names, for a `dev-*`
/// version, the normalized target branch it stands in for. `pub(crate)`:
/// both `pool_builder::push_package_version` (the pool package it also
/// pushes for the alias) and this module's own [`is_version_loaded`] (the
/// alias's stability/constraint match) need it.
pub(crate) fn branch_alias_target(pv: &PackageVersion) -> Option<String> {
    branch_alias_target_of(&pv.version, pv.branch_alias.as_ref())
}

/// [`branch_alias_target`], but reading `extra.branch-alias` straight from a
/// pool [`crate::solver::pool::Package`]'s own `raw` entry instead of a
/// [`PackageVersion`]: `solver::resolve`'s dev-split second solve rebuilds
/// its pool from already-`Package`-shaped first-solve winners
/// (`pool_builder::clone_package`), which never carried a `PackageVersion`
/// to begin with, but still needs the *same* alias re-derived — Composer's
/// own second `PoolBuilder` pass re-derives it from each reloaded package's
/// metadata too, it never carries the first solve's `AliasPackage` object
/// across (`pool_builder::clone_package`'s own doc comment's "never carries
/// `AliasPackage` entries either" is only true of the *objects*, not of
/// whether an alias exists in the rebuilt pool).
pub(crate) fn branch_alias_target_from_raw(pretty_version: &str, raw: &Value) -> Option<String> {
    let branch_alias = raw.get("extra").and_then(|extra| extra.get("branch-alias"));
    branch_alias_target_of(pretty_version, branch_alias)
}

fn branch_alias_target_of(pretty_version: &str, branch_alias: Option<&Value>) -> Option<String> {
    if !(pretty_version.starts_with("dev-") || pretty_version.ends_with("-dev")) {
        return None;
    }
    let target = branch_alias?.as_object()?.get(pretty_version)?.as_str()?;
    if !target.ends_with("-dev") {
        return None;
    }
    if target == "9999999-dev" {
        return Some(target.to_string());
    }
    let branch_name = &target[..target.len() - 4];
    let normalized = semver::normalize_branch(branch_name);
    if !normalized.ends_with("-dev") {
        return None;
    }
    Some(normalized)
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

/// `data` is a freshly deserialized `Value` from [`get_cached_json`]/
/// [`get_hash_verified_json`] with no other reference to it anywhere
/// (every call re-reads and re-parses; #120), so every version entry it
/// holds is moved into a [`PackageVersion`] via
/// [`PackageVersion::from_owned_value`] rather than cloned out of a
/// borrowed `data`.
///
/// The one seam every source's lazy/v1 branch funnels through
/// (`ComposerSource::fetch_lazy`, `ComposerSource::load_versions`'s
/// `Provider::Providers` arm) to turn a provider file into
/// `Vec<PackageVersion>`: `expand_minified` plus one
/// `PackageVersion::from_owned_value` per entry is real CPU work — a big
/// provider file (laravel/framework.json's ~1000 versions) runs it
/// inline on the single-threaded fetch loop, blocking every other
/// in-flight request behind it. `spawn_blocking` moves it to the runtime's
/// blocking pool so the loop keeps polling while it runs.
async fn parse_provider_versions(data: Value, name: &str) -> Result<Vec<PackageVersion>> {
    let name = name.to_string();
    tokio::task::spawn_blocking(move || parse_provider_versions_sync(data, &name))
        .await
        .context("provider parse task panicked")?
}

fn parse_provider_versions_sync(mut data: Value, name: &str) -> Result<Vec<PackageVersion>> {
    let Some(entry) = data
        .get_mut("packages")
        .and_then(Value::as_object_mut)
        .and_then(|packages| packages.remove(name))
    else {
        return Ok(Vec::new());
    };
    // A v1 (non-minified) provider file, wpackagist's included, keys each
    // entry by version label rather than a plain list, the same shape as an
    // inline `packages[name]` entry (`ComposerRepository.php`'s foreach over
    // `$packages['packages']` iterates a PHP array either way, so this split
    // is only needed because JSON objects and arrays aren't interchangeable
    // in Rust).
    if let Value::Object(versions) = entry {
        let convert_started = Instant::now();
        let result: Result<Vec<PackageVersion>> = versions
            .into_values()
            .map(PackageVersion::from_owned_value)
            .collect();
        STAGE_CONVERT_NS.fetch_add(
            convert_started.elapsed().as_nanos() as u64,
            Ordering::Relaxed,
        );
        if let Ok(versions) = &result {
            VERSIONS_PRODUCED.fetch_add(versions.len(), Ordering::Relaxed);
        }
        return result;
    }
    let Value::Array(list) = entry else {
        bail!("{name}: provider entry is not a list");
    };
    let minified = data.get("minified").and_then(Value::as_str) == Some("composer/2.0");
    let expand_started = Instant::now();
    let expanded = if minified {
        expand_minified(list)
    } else {
        list
    };
    STAGE_EXPAND_NS.fetch_add(
        expand_started.elapsed().as_nanos() as u64,
        Ordering::Relaxed,
    );
    let convert_started = Instant::now();
    let result: Result<Vec<PackageVersion>> = expanded
        .into_iter()
        .map(PackageVersion::from_owned_value)
        .collect();
    STAGE_CONVERT_NS.fetch_add(
        convert_started.elapsed().as_nanos() as u64,
        Ordering::Relaxed,
    );
    if let Ok(versions) = &result {
        VERSIONS_PRODUCED.fetch_add(versions.len(), Ordering::Relaxed);
    }
    result
}

/// The other big-body CPU still on the fetch loop after `parse_provider_versions`:
/// every reader of a provider file's raw bytes — a fresh
/// network response or a warm disk cache — pays for `serde_json::from_slice`
/// itself, and a provider file the size of `laravel/framework.json` (990 KB)
/// makes that real work. One `spawn_blocking` here, called from
/// [`read_cache_file`], [`get_cached_json`] and [`get_hash_verified_json`],
/// covers every one of them.
/// Below this, `spawn_blocking`'s own thread-hop costs more than the parse it
/// would hide: most provider files are a few KB (`bench/laravel`'s median is
/// ~20 KB) and inlining those, rather than queuing every one of them onto the
/// blocking pool, is what keeps a corpus of small files from getting slower
/// even as a `laravel/framework.json`-sized one gets faster.
/// ponytail: a fixed size cutoff, not a measured per-file cost; revisit if a
/// profile ever shows a file just above it still worth deferring.
const INLINE_PARSE_MAX_BYTES: usize = 64 * 1024;

async fn parse_json_blocking(bytes: Vec<u8>, context: String) -> Result<Value> {
    if bytes.len() <= INLINE_PARSE_MAX_BYTES {
        let started = Instant::now();
        let parsed = serde_json::from_slice(&bytes);
        STAGE_JSON_PARSE_NS.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        return parsed.with_context(|| context);
    }
    tokio::task::spawn_blocking(move || {
        let started = Instant::now();
        let parsed = serde_json::from_slice::<Value>(&bytes);
        STAGE_JSON_PARSE_NS.fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        parsed
    })
    .await
    .context("JSON parse task panicked")?
    .with_context(|| context)
}

/// #159: how much of a closure load is disk-read-plus-parse rather than
/// network wait, sampled before/after [`Repository::load_closure_seeded`]
/// the same way `requests`/`request_count` already brackets request counts.
/// Global rather than a `Repository` field since [`read_cache_file`] is a
/// free function with no `&self` to carry one on.
static CACHE_FILES_PARSED: AtomicUsize = AtomicUsize::new(0);
static CACHE_BYTES_PARSED: AtomicUsize = AtomicUsize::new(0);

/// #176's split of the 299 ms `read + JSON-parse cached metadata` phase
/// §6.2 already named: same idiom as the pair above, one accumulator per
/// stage, sampled before/after [`Repository::load_closure_seeded`].
static STAGE_READ_NS: AtomicU64 = AtomicU64::new(0);
static STAGE_JSON_PARSE_NS: AtomicU64 = AtomicU64::new(0);
static STAGE_EXPAND_NS: AtomicU64 = AtomicU64::new(0);
static STAGE_CONVERT_NS: AtomicU64 = AtomicU64::new(0);
static VERSIONS_PRODUCED: AtomicUsize = AtomicUsize::new(0);
static VERSION_NORMALIZE_CALLS: AtomicUsize = AtomicUsize::new(0);

/// Reads a cache file written by [`write_cache_file`]: the raw provider
/// JSON with a `last-modified` key merged in, mirroring Composer's own
/// `Cache` format for this file (`ComposerRepository.php:1793-1797`) so the
/// value can be sent back as `If-Modified-Since` next time.
async fn read_cache_file(path: &Path) -> Result<Option<(Value, Option<String>)>> {
    let read_started = Instant::now();
    let read = fs_err::read(path);
    STAGE_READ_NS.fetch_add(read_started.elapsed().as_nanos() as u64, Ordering::Relaxed);
    match read {
        Ok(bytes) => {
            CACHE_FILES_PARSED.fetch_add(1, Ordering::Relaxed);
            CACHE_BYTES_PARSED.fetch_add(bytes.len(), Ordering::Relaxed);
            let data = parse_json_blocking(
                bytes,
                format!("{}: cached file is not valid JSON", path.display()),
            )
            .await?;
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
    let cached = read_cache_file(cache_path).await?;
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
            let mut data = parse_json_blocking(body, format!("{url}: not valid JSON")).await?;
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
            "repositories": [{"type": "path", "url": "../acme/pkg"}],
        });
        let err = parse_repositories(&root).unwrap_err().to_string();
        assert!(err.contains("path"), "{err}");
        assert!(!err.contains("v0.1"), "{err}");
    }

    #[test]
    fn parse_repositories_accepts_vcs_git_and_github_types() {
        let root = serde_json::json!({
            "repositories": [
                {"type": "vcs", "url": "https://example.com/acme/pkg.git"},
                {"type": "git", "url": "https://example.com/acme/other.git"},
                {"type": "github", "url": "https://github.com/acme/third"},
            ],
        });
        let entries = parse_repositories(&root).unwrap();
        assert!(matches!(
            entries[0].kind,
            RepoKind::Vcs { ref repo_type } if repo_type == "vcs"
        ));
        assert!(matches!(
            entries[1].kind,
            RepoKind::Vcs { ref repo_type } if repo_type == "git"
        ));
        assert!(matches!(
            entries[2].kind,
            RepoKind::Vcs { ref repo_type } if repo_type == "github"
        ));
        // packagist.org is still appended last, unaffected by a vcs entry.
        assert!(matches!(entries[3].kind, RepoKind::Composer));
    }
}
