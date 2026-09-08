//! Parsing `composer.lock` and the root `composer.json`.
//!
//! Each lock package keeps its untouched [`serde_json::Value`] (`raw`)
//! alongside typed fields, because `installed.json` re-emits lock entries in
//! their original key order (`serde_json`'s `preserve_order` feature).

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::solver::transaction::AliasEntry;

/// A parsed `composer.lock`.
#[derive(Debug, Clone)]
pub struct Lock {
    pub content_hash: Option<String>,
    pub packages: Vec<Package>,
    /// The lock's root-level `aliases` array: `composer.json` requiring
    /// `some/package: dev-master as 1.0.0` (`Locker::getLockedRepository`
    /// wraps the locked package in a `CompleteAliasPackage` for each entry
    /// here, matched on `package` alone). Separate from a package's own
    /// `extra.branch-alias`/`default-branch` metadata (`branch_alias` in
    /// `autoload/installed.rs`) — both can apply to the same package.
    pub aliases: Vec<AliasEntry>,
}

impl Lock {
    /// Iterate packages, dropping `packages-dev` entries unless `dev` is set.
    pub fn packages(&self, dev: bool) -> impl Iterator<Item = &Package> {
        self.packages
            .iter()
            .filter(move |package| dev || !package.dev)
    }
}

/// A lock entry's `dist` block.
#[derive(Debug, Clone, Deserialize)]
pub struct Dist {
    #[serde(rename = "type")]
    pub r#type: String,
    pub url: String,
    pub reference: Option<String>,
    pub shasum: Option<String>,
}

/// A lock entry's `source` block: VCS provenance, present for the
/// dist-less packages [`Package::validate_dist`] accepts on top of a zip/tar
/// `dist` (#13's "no dist at all, git source" case) and, redundantly, for
/// most zip/tar packages too (unused by vivace there).
#[derive(Debug, Clone, Deserialize)]
pub struct Source {
    #[serde(rename = "type")]
    pub r#type: String,
    pub url: String,
    pub reference: Option<String>,
}

/// A lock entry's `transport-options` block: only the two keys a path
/// repository package sets (`PathRepository`/`PathDownloader`). `None` means
/// Composer's own default for that option (symlink when possible, relative
/// when possible).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TransportOptions {
    pub symlink: Option<bool>,
    pub relative: Option<bool>,
}

/// One package from `packages` or `packages-dev`.
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    #[serde(deserialize_with = "deserialize_lowercase")]
    pub name: String,
    pub version: String,
    pub dist: Option<Dist>,
    pub source: Option<Source>,
    #[serde(rename = "transport-options", default)]
    pub transport_options: TransportOptions,
    pub autoload: Option<Value>,
    #[serde(default)]
    pub require: Map<String, Value>,
    #[serde(default)]
    pub provide: Map<String, Value>,
    #[serde(default)]
    pub replace: Map<String, Value>,
    #[serde(rename = "type", default = "default_type")]
    pub r#type: String,
    #[serde(rename = "target-dir")]
    pub target_dir: Option<String>,
    #[serde(default, deserialize_with = "deserialize_string_or_vec")]
    pub bin: Vec<String>,
    #[serde(
        rename = "include-path",
        default,
        deserialize_with = "deserialize_string_or_vec"
    )]
    pub include_path: Vec<String>,
    #[serde(skip)]
    pub dev: bool,
    /// The untouched lock entry, key order preserved.
    #[serde(skip)]
    pub raw: Value,
    /// Project-root-relative install directory (no leading/trailing slash),
    /// set by [`crate::plugins::Plugins::install_dir`] before planning when
    /// `composer/installers` or a `wordpress-core-installer` maps this
    /// package outside `vendor/`; `None` keeps the default `vendor/<name>`.
    #[serde(skip)]
    pub install_dir: Option<String>,
    /// `config.preferred-install` (#43) picked source over dist for a
    /// package that has both, set the same way `install_dir` is: by
    /// `install::run`, before planning. `false` for everything else,
    /// including the dist-less git-source packages [`Package::is_git_source`]
    /// already always checks out from source regardless of this flag.
    #[serde(skip)]
    pub install_from_source: bool,
}

impl Package {
    /// vivace v0.1 fetches zip and tar dists (tar covers `.tar`,
    /// `.tar.gz`/`.tgz` and `.tar.bz2`: Composer's own `dist.type` is `"tar"`
    /// for all three, distinguished by the archive bytes, not the type
    /// string) and symlinks/mirrors `path` dists (#13); a package with no
    /// `dist` at all is accepted only when its `source` is a git checkout
    /// (#13's other half), and rejected otherwise — error clearly, naming
    /// the package, rather than failing obscurely later in `fetch`.
    pub fn validate_dist(&self) -> Result<()> {
        match &self.dist {
            None if self.is_git_source() => Ok(()),
            None => bail!(
                "{}: no dist entry and no git source (svn/hg/fossil sources are not supported; \
                 viv installs zip/tar dists, path repositories and git sources)",
                self.name
            ),
            Some(dist) if dist.r#type == "path" => Ok(()),
            Some(dist) if dist.r#type != "zip" && dist.r#type != "tar" => bail!(
                "{}: dist type \"{}\" is not supported in vivace v0.1 (zip, tar and path only)",
                self.name,
                dist.r#type
            ),
            Some(_) => Ok(()),
        }
    }

    /// A path-repository package (#13): `dist.type` is `"path"`, never
    /// fetched or stored, symlinked/mirrored straight from `dist.url`.
    pub fn is_path(&self) -> bool {
        self.dist.as_ref().is_some_and(|d| d.r#type == "path")
    }

    /// A dist-less, git-source package (#13): cloned from `source.url`
    /// rather than fetched as an archive.
    pub fn is_git_source(&self) -> bool {
        self.dist.is_none() && self.source.as_ref().is_some_and(|s| s.r#type == "git")
    }
}

fn default_type() -> String {
    "library".to_string()
}

/// Composer allows some list-valued keys (`bin`, `include-path`) to be a
/// single string or an array of strings.
fn deserialize_string_or_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrVec {
        String(String),
        Vec(Vec<String>),
    }

    Ok(match StringOrVec::deserialize(deserializer)? {
        StringOrVec::String(s) => vec![s],
        StringOrVec::Vec(v) => v,
    })
}

/// Read and parse a `composer.lock` file.
pub fn read_lock(path: &Path) -> Result<Lock> {
    let content = fs_err::read_to_string(path)?;
    let raw: Value = serde_json::from_str(&content)
        .with_context(|| format!("parsing {} as JSON", path.display()))?;

    let content_hash = raw
        .get("content-hash")
        .and_then(Value::as_str)
        .map(str::to_owned);

    let mut packages = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for (key, dev) in [("packages", false), ("packages-dev", true)] {
        for entry in raw.get(key).and_then(Value::as_array).into_iter().flatten() {
            let package = parse_package(entry, dev, path)?;
            if !seen.insert(package.name.clone()) {
                bail!(
                    "{}: package \"{}\" appears more than once in packages/packages-dev",
                    path.display(),
                    package.name
                );
            }
            packages.push(package);
        }
    }

    let aliases = raw
        .get("aliases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(AliasEntry {
                package: entry.get("package")?.as_str()?.to_owned(),
                version: entry.get("version")?.as_str()?.to_owned(),
                alias: entry.get("alias")?.as_str()?.to_owned(),
                alias_normalized: entry.get("alias_normalized")?.as_str()?.to_owned(),
            })
        })
        .collect();

    Ok(Lock {
        content_hash,
        packages,
        aliases,
    })
}

fn parse_package(raw: &Value, dev: bool, lock_path: &Path) -> Result<Package> {
    let name = raw
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let mut package: Package = serde_json::from_value(raw.clone()).with_context(|| {
        format!(
            "parsing composer.lock package entry \"{name}\" in {}",
            lock_path.display()
        )
    })?;
    package.dev = dev;
    package.raw = raw.clone();
    Ok(package)
}

/// `config.platform-check`: `"php-only"` (default), `true` (all platform
/// packages checked) or `false` (no check at all).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlatformCheck {
    #[default]
    PhpOnly,
    All,
    Off,
}

impl<'de> Deserialize<'de> for PlatformCheck {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match Value::deserialize(deserializer)? {
            Value::Bool(true) => Ok(PlatformCheck::All),
            Value::Bool(false) => Ok(PlatformCheck::Off),
            Value::String(s) if s == "php-only" => Ok(PlatformCheck::PhpOnly),
            other => Err(serde::de::Error::custom(format!(
                "invalid config.platform-check value: {other}"
            ))),
        }
    }
}

/// Composer does `rtrim($vendorDir, '/')` on `config.vendor-dir`.
fn deserialize_vendor_dir<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(String::deserialize(deserializer)?
        .trim_end_matches('/')
        .to_string())
}

/// `config.allow-plugins`: Composer's `PluginManager::parseAllowedPlugins`.
/// `true`/`false` allow or deny every `composer-plugin` package; a map is
/// tried in key order, first pattern to match (`*` glob, case-insensitive,
/// `BasePackage::packageNameToRegexp`) wins. Absent entirely (Composer's
/// schema default, `[]`) behaves like an empty map: nothing matches, so
/// every plugin is disallowed — the same outcome Composer reaches
/// non-interactively (CI has no prompt to fall back on either).
#[derive(Debug, Clone, Default)]
pub enum AllowPlugins {
    All(bool),
    #[default]
    None,
    Map(Vec<(String, bool)>),
}

impl AllowPlugins {
    pub fn is_enabled(&self, package: &str) -> bool {
        match self {
            AllowPlugins::All(allow) => *allow,
            AllowPlugins::None => false,
            AllowPlugins::Map(rules) => rules
                .iter()
                .find(|(pattern, _)| glob_match(pattern, package))
                .is_some_and(|(_, allow)| *allow),
        }
    }
}

/// `BasePackage::packageNameToRegexp`: `*` expands to "anything", the rest of
/// the pattern matches literally, case-insensitively.
///
/// Plain string matching, not a compiled regex: `preferred-install`'s
/// pattern map runs this once per locked package on every `viv install`,
/// including the no-op path, and `Regex::new` compiling a fresh NFA per
/// package there was measurable (~20ms on a 101-package lock, `bench/laravel`
/// noop) next to everything else that run does.
fn glob_match(pattern: &str, name: &str) -> bool {
    let pattern = pattern.to_lowercase();
    let name = name.to_lowercase();
    let mut segments = pattern.split('*');
    let Some(first) = segments.next() else {
        return name.is_empty();
    };
    let Some(rest) = name.strip_prefix(first) else {
        return false;
    };
    let mut segments: Vec<&str> = segments.collect();
    let Some(last) = segments.pop() else {
        // No `*` in `pattern` at all: `first` must be the whole name.
        return rest.is_empty();
    };
    let mut rest = rest;
    for middle in segments {
        let Some(at) = rest.find(middle) else {
            return false;
        };
        rest = &rest[at + middle.len()..];
    }
    rest.ends_with(last)
}

/// `dist`/`source` install preference (`config.preferred-install`'s value,
/// or one pattern-map entry's value): Composer's
/// `DownloadManager::resolvePackageInstallPreference`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallPreference {
    Dist,
    Source,
    Auto,
}

impl InstallPreference {
    /// `dist` always wins; `auto` only picks `source` for a dev-stability
    /// package (a `dev-*` branch, or `*-dev`/`*.x-dev`); anything else
    /// (`source`, or `auto` on a non-dev version) picks `source`.
    fn source_for(self, is_dev: bool) -> bool {
        match self {
            InstallPreference::Dist => false,
            InstallPreference::Source => true,
            InstallPreference::Auto => is_dev,
        }
    }
}

impl<'de> Deserialize<'de> for InstallPreference {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match String::deserialize(deserializer)?.as_str() {
            "dist" => Ok(InstallPreference::Dist),
            "source" => Ok(InstallPreference::Source),
            "auto" => Ok(InstallPreference::Auto),
            other => Err(serde::de::Error::custom(format!(
                "invalid config.preferred-install value: {other}"
            ))),
        }
    }
}

/// `config.preferred-install`: a single preference for every package, or a
/// pattern map (`BasePackage::packageNameToRegexp` glob, first match wins,
/// falling back to Composer's own default — `source` for a dev-stability
/// package, `dist` otherwise — when nothing matches).
#[derive(Debug, Clone)]
pub enum PreferredInstall {
    All(InstallPreference),
    Map(Vec<(String, InstallPreference)>),
}

impl Default for PreferredInstall {
    fn default() -> Self {
        PreferredInstall::All(InstallPreference::Dist)
    }
}

impl PreferredInstall {
    /// Whether `package` (a dev-stability version, or not) should be
    /// installed from source rather than dist.
    pub fn prefers_source(&self, package: &str, is_dev: bool) -> bool {
        match self {
            PreferredInstall::All(pref) => pref.source_for(is_dev),
            PreferredInstall::Map(rules) => rules
                .iter()
                .find(|(pattern, _)| glob_match(pattern, package))
                .map_or(is_dev, |(_, pref)| pref.source_for(is_dev)),
        }
    }

    /// `Config::merge`'s `preferred-install` case: `overlay` (a higher-
    /// precedence source, e.g. project `composer.json` over composer home's
    /// `config.json`) replaces `self` outright when both are a single
    /// preference; when either is a pattern map, the other is coerced to
    /// `{"*": value}` first, entries merge key-wise (`overlay` winning ties,
    /// new keys appended in `overlay`'s order), and `*` is moved back to the
    /// end — Composer always evaluates the wildcard last.
    fn merge(self, overlay: PreferredInstall) -> PreferredInstall {
        let (PreferredInstall::All(base), PreferredInstall::All(over)) = (&self, &overlay) else {
            let mut merged = self.into_map();
            for (pattern, pref) in overlay.into_map() {
                match merged.iter_mut().find(|(p, _)| *p == pattern) {
                    Some(entry) => entry.1 = pref,
                    None => merged.push((pattern, pref)),
                }
            }
            if let Some(pos) = merged.iter().position(|(p, _)| p == "*") {
                let wildcard = merged.remove(pos);
                merged.push(wildcard);
            }
            return PreferredInstall::Map(merged);
        };
        let _ = base;
        PreferredInstall::All(*over)
    }

    fn into_map(self) -> Vec<(String, InstallPreference)> {
        match self {
            PreferredInstall::All(pref) => vec![("*".to_string(), pref)],
            PreferredInstall::Map(rules) => rules,
        }
    }
}

impl<'de> Deserialize<'de> for PreferredInstall {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match Value::deserialize(deserializer)? {
            Value::Object(map) => Ok(PreferredInstall::Map(
                map.into_iter()
                    .map(|(name, pref)| {
                        InstallPreference::deserialize(pref)
                            .map(|pref| (name, pref))
                            .map_err(serde::de::Error::custom)
                    })
                    .collect::<std::result::Result<_, D::Error>>()?,
            )),
            other => Ok(PreferredInstall::All(
                InstallPreference::deserialize(other).map_err(serde::de::Error::custom)?,
            )),
        }
    }
}

/// A dev-stability version, matching `VersionParser::parseStability`'s
/// check on the pretty version Composer records in the lock: a `dev-`
/// branch prefix, or a `-dev` suffix (e.g. `1.x-dev`, `9999999-dev`) — the
/// only two shapes normalisation ever produces, so no full stability parse
/// is needed here.
pub fn is_dev_version(version: &str) -> bool {
    version.starts_with("dev-") || version.ends_with("-dev")
}

/// Composer home's `config.json`, merged under the project's own
/// `config.preferred-install` (`Config::merge`'s precedence: home config
/// first, project `composer.json` on top). A missing or unreadable home
/// config is not an error — same treatment as a missing `auth.json`
/// ([`crate::auth::Auth::load`]).
pub fn resolve_preferred_install(project: &PreferredInstall) -> Result<PreferredInstall> {
    preferred_install_at(crate::auth::composer_home().as_deref(), project)
}

/// [`resolve_preferred_install`], with the composer-home directory passed in
/// rather than resolved from the environment, so tests don't need to touch
/// process-global env vars to exercise it.
fn preferred_install_at(
    home: Option<&Path>,
    project: &PreferredInstall,
) -> Result<PreferredInstall> {
    let Some(home) = home else {
        return Ok(project.clone());
    };
    let Ok(content) = fs_err::read_to_string(home.join("config.json")) else {
        return Ok(project.clone());
    };
    let raw: Value = serde_json::from_str(&content)
        .with_context(|| format!("{}: not valid JSON", home.join("config.json").display()))?;
    let Some(value) = raw.get("config").and_then(|c| c.get("preferred-install")) else {
        return Ok(project.clone());
    };
    let global = PreferredInstall::deserialize(value.clone())
        .context("composer home's config.json: invalid preferred-install")?;
    Ok(global.merge(project.clone()))
}

impl<'de> Deserialize<'de> for AllowPlugins {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match Value::deserialize(deserializer)? {
            Value::Bool(allow) => Ok(AllowPlugins::All(allow)),
            Value::Object(map) => Ok(AllowPlugins::Map(
                map.into_iter()
                    .map(|(name, allow)| (name, allow.as_bool().unwrap_or(false)))
                    .collect(),
            )),
            other => Err(serde::de::Error::custom(format!(
                "invalid config.allow-plugins value: {other}"
            ))),
        }
    }
}

/// Composer lowercases package names throughout.
fn deserialize_lowercase<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(String::deserialize(deserializer)?.to_lowercase())
}

/// Composer lowercases package names throughout.
fn deserialize_lowercase_option<'de, D>(
    deserializer: D,
) -> std::result::Result<Option<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.map(|s| s.to_lowercase()))
}

/// The root `composer.json`'s `config` block (the subset vivace reads).
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's config keys"
)]
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(rename = "autoloader-suffix")]
    pub autoloader_suffix: Option<String>,
    #[serde(rename = "platform-check")]
    pub platform_check: PlatformCheck,
    #[serde(rename = "vendor-dir", deserialize_with = "deserialize_vendor_dir")]
    pub vendor_dir: String,
    #[serde(rename = "prepend-autoloader")]
    pub prepend_autoloader: bool,
    /// `None` when `composer.json` doesn't set `bin-dir`: resolved to
    /// `{vendor-dir}/bin` by [`Config::bin_dir`], not a static
    /// `"vendor/bin"`, since Composer derives it from `vendor-dir` too.
    #[serde(rename = "bin-dir")]
    pub bin_dir: Option<String>,
    #[serde(rename = "bin-compat")]
    pub bin_compat: crate::bin::BinCompat,
    /// `composer install -o`'s default: also classmap-scan PSR-0/PSR-4 dirs.
    #[serde(rename = "optimize-autoloader")]
    pub optimize_autoloader: bool,
    /// `composer install -a`'s default: classmap-only autoloading.
    #[serde(rename = "classmap-authoritative")]
    pub classmap_authoritative: bool,
    /// `composer install --apcu-autoloader`'s default.
    #[serde(rename = "apcu-autoloader")]
    pub apcu_autoloader: bool,
    /// `composer install --apcu-autoloader-prefix`'s default.
    #[serde(rename = "apcu-autoloader-prefix")]
    pub apcu_autoloader_prefix: Option<String>,
    /// `$loader->setUseIncludePath(true)` in `autoload_real.php`.
    #[serde(rename = "use-include-path")]
    pub use_include_path: bool,
    /// `false` lets dist/repository URLs downgrade to plain `http`
    /// (`fetch::Fetcher::secure_http`); Composer defaults this to `true`.
    #[serde(rename = "secure-http")]
    pub secure_http: bool,
    /// Which `composer-plugin` packages viv treats as enabled
    /// (`docs/plugin-strategy.md`'s rule 3, and the native adapters' gate).
    #[serde(rename = "allow-plugins")]
    pub allow_plugins: AllowPlugins,
    /// `dist`/`source` per package (#43): project-level only, merged with
    /// composer home's `config.json` by [`resolve_preferred_install`], not
    /// here — [`Config`] only ever sees the project's own composer.json.
    #[serde(rename = "preferred-install")]
    pub preferred_install: PreferredInstall,
    /// `viv audit`'s config-level ignore list and abandoned-package policy
    /// (`Config.php`'s `'audit' => ['ignore' => [], 'abandoned' => 'fail']`).
    #[serde(default)]
    pub audit: AuditConfig,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            autoloader_suffix: None,
            platform_check: PlatformCheck::PhpOnly,
            vendor_dir: "vendor".to_string(),
            prepend_autoloader: true,
            bin_dir: None,
            bin_compat: crate::bin::BinCompat::Auto,
            optimize_autoloader: false,
            classmap_authoritative: false,
            apcu_autoloader: false,
            apcu_autoloader_prefix: None,
            use_include_path: false,
            secure_http: true,
            allow_plugins: AllowPlugins::None,
            preferred_install: PreferredInstall::default(),
            audit: AuditConfig::default(),
        }
    }
}

/// `config.audit`: `viv audit`'s ignore list and abandoned-package policy.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct AuditConfig {
    pub ignore: AuditIgnore,
    pub abandoned: AbandonedPolicy,
}

/// `config.audit.ignore`: either a bare list of advisory IDs/package names
/// (no reason recorded), or a map of the same keyed to a reason string
/// (`null` allowed, same as no reason).
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum AuditIgnore {
    List(Vec<String>),
    Map(std::collections::HashMap<String, Option<String>>),
}

impl Default for AuditIgnore {
    fn default() -> Self {
        AuditIgnore::List(Vec::new())
    }
}

impl AuditIgnore {
    /// `Some(reason)` when `key` (a package name, advisory ID, CVE or
    /// remote ID — `Auditor::processAdvisories` checks all four) is
    /// ignored; the inner `Option` is the configured reason, if any.
    pub fn reason_for(&self, key: &str) -> Option<Option<String>> {
        match self {
            AuditIgnore::List(ids) => ids.iter().any(|id| id == key).then_some(None),
            AuditIgnore::Map(map) => map.get(key).cloned(),
        }
    }
}

/// `config.audit.abandoned`/`--abandoned`: how `viv audit` treats an
/// abandoned package. Composer defaults this to `fail`, unlike `install`'s
/// own abandoned handling, which only ever warns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AbandonedPolicy {
    Ignore,
    Report,
    #[default]
    Fail,
}

impl AbandonedPolicy {
    /// `--abandoned`'s three accepted values; anything else is the same
    /// error message `AuditCommand::execute` throws.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "ignore" => Ok(AbandonedPolicy::Ignore),
            "report" => Ok(AbandonedPolicy::Report),
            "fail" => Ok(AbandonedPolicy::Fail),
            _ => bail!("--abandoned must be one of ignore, report, fail."),
        }
    }
}

impl<'de> Deserialize<'de> for AbandonedPolicy {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        AbandonedPolicy::parse(&value).map_err(serde::de::Error::custom)
    }
}

impl Config {
    /// `config.bin-dir`, resolved: the explicit value when set, else
    /// `{vendor-dir}/bin` (Composer's `Config::get('bin-dir')` default,
    /// which is defined relative to `vendor-dir`, not a literal
    /// `"vendor/bin"`).
    pub fn bin_dir(&self) -> String {
        self.bin_dir
            .clone()
            .unwrap_or_else(|| format!("{}/bin", self.vendor_dir))
    }
}

/// The root `composer.json`, typed for the fields vivace needs.
#[derive(Debug, Clone, Deserialize)]
pub struct Root {
    #[serde(default, deserialize_with = "deserialize_lowercase_option")]
    pub name: Option<String>,
    pub version: Option<String>,
    #[serde(rename = "type", default = "default_type")]
    pub r#type: String,
    pub autoload: Option<Value>,
    #[serde(rename = "autoload-dev")]
    pub autoload_dev: Option<Value>,
    #[serde(default)]
    pub require: Map<String, Value>,
    #[serde(rename = "require-dev", default)]
    pub require_dev: Map<String, Value>,
    #[serde(default)]
    pub replace: Map<String, Value>,
    #[serde(default)]
    pub provide: Map<String, Value>,
    #[serde(
        rename = "include-path",
        default,
        deserialize_with = "deserialize_string_or_vec"
    )]
    pub include_path: Vec<String>,
    #[serde(default)]
    pub config: Config,
    /// `extra.installer-paths`/`extra.wordpress-install-dir`
    /// (`src/plugins.rs`) live here alongside whatever else a project keeps
    /// in `extra`; kept as a raw [`Value`] like [`Package::raw`], since only
    /// those two keys are ever read.
    #[serde(default)]
    pub extra: Value,
}

/// Read and parse the root `composer.json`.
pub fn read_root(path: &Path) -> Result<Root> {
    let content = fs_err::read(path)?;
    parse_root(&content).with_context(|| format!("parsing {} as JSON", path.display()))
}

/// Parse an already-read root `composer.json`'s bytes, for callers
/// (`install::run`/`dump_autoload`) that need the same bytes for other
/// checks too (lock freshness, the `.vivace-state` hash) and shouldn't read
/// the file twice.
pub fn parse_root(bytes: &[u8]) -> Result<Root> {
    let value: Value = serde_json::from_slice(bytes).context("parsing as JSON")?;
    root_from_value(&value)
}

/// [`parse_root`], for a caller (`install::run_impl`, #122) that already
/// parsed the root `composer.json` into a `Value` for another reason (the
/// content-hash, the scripts runner) and shouldn't parse the same bytes
/// again just for `Root`.
pub fn root_from_value(value: &Value) -> Result<Root> {
    Root::deserialize(value).context("parsing as JSON")
}

/// Composer's `Locker::getContentHash`: an md5 of the sorted, compact JSON
/// of the `composer.json` keys that decide what a lock should contain.
pub(crate) fn content_hash(root_json: &[u8]) -> Result<String> {
    let content: Value = serde_json::from_slice(root_json).context("parsing composer.json")?;
    Ok(content_hash_from_value(&content))
}

/// [`content_hash`], for a caller that already holds the parsed `Value`
/// (#122's install-path single-parse).
pub(crate) fn content_hash_from_value(content: &Value) -> String {
    const RELEVANT: &[&str] = &[
        "name",
        "version",
        "require",
        "require-dev",
        "conflict",
        "replace",
        "provide",
        "minimum-stability",
        "prefer-stable",
        "repositories",
        "extra",
    ];
    let mut relevant = std::collections::BTreeMap::new();
    if let Some(root) = content.as_object() {
        for key in RELEVANT {
            if let Some(value) = root.get(*key) {
                relevant.insert((*key).to_string(), value.clone());
            }
        }
        if let Some(platform) = root.get("config").and_then(|c| c.get("platform")) {
            relevant.insert(
                "config".to_string(),
                serde_json::json!({ "platform": platform }),
            );
        }
    }
    let encoded = php_json_encode(&Value::Object(
        relevant.into_iter().collect::<Map<String, Value>>(),
    ));
    format!("{:x}", md5::compute(encoded))
}

/// PHP's `json_encode($value, 0)`: like `serde_json`'s compact encoding, but
/// `/` is escaped as `\/` and non-ASCII characters are escaped as `\uXXXX`
/// (UTF-16 code units, surrogate pairs above `U+FFFF`). Key order within an
/// object is left untouched; the caller sorts before calling this.
fn php_json_encode(value: &Value) -> String {
    let mut out = String::new();
    write_php_json(value, &mut out);
    out
}

fn write_php_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        // serde_json's `Number` already renders the shortest round-trip form
        // PHP's `serialize_precision = -1` produces (e.g. `1.0`), so reuse it.
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_php_json_string(s, out),
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_php_json(item, out);
            }
            out.push(']');
        }
        Value::Object(map) => {
            out.push('{');
            for (i, (key, item)) in map.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_php_json_string(key, out);
                out.push(':');
                write_php_json(item, out);
            }
            out.push('}');
        }
    }
}

fn write_php_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                write!(out, "\\u{:04x}", c as u32).expect("write! to String never fails");
            }
            c if c.is_ascii() => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    use std::fmt::Write as _;
                    write!(out, "\\u{unit:04x}").expect("write! to String never fails");
                }
            }
        }
    }
    out.push('"');
}

/// Composer's `Locker::isFresh`: does `lock`'s `content-hash` still match
/// `root_json` (the root `composer.json` bytes)? A lock without a
/// `content-hash` at all has nothing to compare, so it is treated as fresh.
///
/// Kept alongside [`is_fresh`] (rather than replaced by it) because a stale
/// hash is only a warning in real Composer (`Installer::doInstall`), but
/// `tests/support/mod.rs`'s installer-fixture rig uses this `Err`-on-stale
/// shape to stand in for a fixture's real (solver) rejection reason.
pub fn validate_against_root(lock: &Lock, root_json: &[u8]) -> Result<()> {
    let content: Value = serde_json::from_slice(root_json).context("parsing composer.json")?;
    validate_against_root_value(lock, &content)
}

fn validate_against_root_value(lock: &Lock, content: &Value) -> Result<()> {
    let Some(locked_hash) = &lock.content_hash else {
        return Ok(());
    };
    let current = content_hash_from_value(content);
    if &current != locked_hash {
        bail!(
            "The lock file is not up to date with the latest changes in composer.json \
             (content-hash mismatch: locked {locked_hash}, current {current})"
        );
    }
    Ok(())
}

/// `validate_against_root` as a plain bool, for callers (`install::run`)
/// that only need to decide whether to print Composer's exact stale-lock
/// warning, not to bail.
pub fn is_fresh(lock: &Lock, root_json: &[u8]) -> Result<bool> {
    Ok(validate_against_root(lock, root_json).is_ok())
}

/// [`is_fresh`], for a caller that already holds the parsed `Value`
/// (#122's install-path single-parse).
pub fn is_fresh_from_value(lock: &Lock, content: &Value) -> Result<bool> {
    Ok(validate_against_root_value(lock, content).is_ok())
}

/// Composer's exact wording (`Installer::doInstall`) for a stale
/// `content-hash`, printed to stderr and continued past, never fatal.
pub const STALE_LOCK_WARNING: &str = "Warning: The lock file is not up to date with the \
     latest changes in composer.json. You may be getting outdated dependencies. It is \
     recommended that you run `composer update` or `composer update <package name>`.";

/// Composer's root package names that are platform, not real packages
/// (`PlatformRepository::isPlatformPackage`, the subset vivace cares about).
fn is_platform_package(name: &str) -> bool {
    name == "php"
        || name.starts_with("php-")
        || name == "hhvm"
        || name.starts_with("ext-")
        || name.starts_with("lib-")
        || name.starts_with("composer-")
}

/// Composer's `Locker::getMissingRequirementInfo`, presence-only: is each of
/// `root`'s required packages (and, when `dev`, `require-dev` too) locked at
/// all? Whether the locked version actually *satisfies* the root constraint
/// needs a semver solver, which vivace's no-dependency-resolution planner
/// deliberately doesn't have; that half of Composer's check is out of scope
/// for v0.1.
pub fn missing_requirements(lock: &Lock, root: &Root, dev: bool) -> Vec<String> {
    let locked: std::collections::HashSet<String> =
        lock.packages(dev).map(|p| p.name.clone()).collect();
    let mut sets: Vec<(&str, &Map<String, Value>)> = vec![("Required", &root.require)];
    if dev {
        sets.push(("Required (in require-dev)", &root.require_dev));
    }
    sets.into_iter()
        .flat_map(|(description, requires)| {
            requires
                .keys()
                .filter(|name| !is_platform_package(name))
                .filter(|name| !locked.contains(&name.to_lowercase()))
                .map(move |name| {
                    format!("- {description} package \"{name}\" is not present in the lock file.")
                })
        })
        .collect()
}

/// Composer's three trailing lines (`Locker::getMissingRequirementInfo`),
/// appended after `missing_requirements`'s own lines when it isn't empty.
/// Kept separate so `tests/support/mod.rs`'s installer-fixture rig, which
/// compares `missing_requirements`'s raw lines against a fixture's
/// `EXPECT-OUTPUT`, doesn't have to filter these out.
pub const MISSING_REQUIREMENTS_HINT: [&str; 3] = [
    "This usually happens when composer files are incorrectly merged or the composer.json \
     file is manually edited.",
    "Read more about correctly resolving merge conflicts \
     https://getcomposer.org/doc/articles/resolving-merge-conflicts.md",
    "and prefer using the \"require\" command over editing the composer.json file directly \
     https://getcomposer.org/doc/03-cli.md#require-r",
];

#[cfg(test)]
mod tests {
    use super::{
        InstallPreference, PlatformCheck, PreferredInstall, glob_match, is_dev_version, is_fresh,
        missing_requirements, preferred_install_at, read_lock, read_root,
    };
    use serde_json::Value;
    use std::io::Write as _;
    use std::path::Path;

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/monolog")
            .join(name)
    }

    #[test]
    fn counts_and_names_packages() {
        let lock = read_lock(&fixture("composer.lock")).unwrap();

        let all: Vec<&str> = lock.packages(true).map(|p| p.name.as_str()).collect();
        assert_eq!(all, ["monolog/monolog", "psr/log", "psr/container"]);

        let no_dev: Vec<&str> = lock.packages(false).map(|p| p.name.as_str()).collect();
        assert_eq!(no_dev, ["monolog/monolog", "psr/log"]);
    }

    #[test]
    fn flags_packages_dev_entries() {
        let lock = read_lock(&fixture("composer.lock")).unwrap();
        for package in lock.packages(true) {
            assert_eq!(
                package.dev,
                package.name == "psr/container",
                "{}",
                package.name
            );
        }
    }

    #[test]
    fn parses_monolog_dist() {
        let lock = read_lock(&fixture("composer.lock")).unwrap();
        let monolog = lock
            .packages(true)
            .find(|p| p.name == "monolog/monolog")
            .unwrap();
        let dist = monolog.dist.as_ref().unwrap();
        assert_eq!(
            dist.reference.as_deref(),
            Some("147f303310f06334f03f409e49d7ad1e275ff05a")
        );
        assert_eq!(dist.shasum.as_deref(), Some(""));
    }

    #[test]
    fn parses_provide_map() {
        let lock = read_lock(&fixture("composer.lock")).unwrap();
        let monolog = lock
            .packages(true)
            .find(|p| p.name == "monolog/monolog")
            .unwrap();
        assert!(monolog.provide.contains_key("psr/log-implementation"));
    }

    #[test]
    fn raw_preserves_key_order() {
        let lock = read_lock(&fixture("composer.lock")).unwrap();
        let monolog = lock
            .packages(true)
            .find(|p| p.name == "monolog/monolog")
            .unwrap();
        let keys: Vec<&str> = monolog
            .raw
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys[0], "name");
        assert_eq!(keys[1], "version");
        let source_index = keys.iter().position(|k| *k == "source").unwrap();
        assert!(source_index > 1, "version should come before source");
    }

    #[test]
    fn reads_root_config() {
        let root = read_root(&fixture("composer.json")).unwrap();
        assert_eq!(
            root.config.autoloader_suffix.as_deref(),
            Some("VivaceFixture")
        );
        assert_eq!(root.config.platform_check, PlatformCheck::PhpOnly);
        assert_eq!(root.config.vendor_dir, "vendor");
        assert!(root.config.prepend_autoloader);
        assert!(root.autoload.is_some());
        assert!(root.autoload_dev.is_none());
    }

    #[test]
    fn bin_accepts_string_or_array() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "acme/tool",
                    "version": "1.0.0",
                    "bin": "bin/foo"
                }
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let lock = read_lock(file.path()).unwrap();
        let package = lock.packages(true).next().unwrap();
        assert_eq!(package.bin, ["bin/foo"]);
    }

    #[test]
    fn include_path_accepts_string_or_array_on_package_and_root() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "pear/console_getopt",
                    "version": "1.0.0",
                    "include-path": ["./"]
                }
            ],
            "packages-dev": []
        }"#;
        let mut lock_file = tempfile::NamedTempFile::new().unwrap();
        lock_file.write_all(lock_json.as_bytes()).unwrap();

        let lock = read_lock(lock_file.path()).unwrap();
        let package = lock.packages(true).next().unwrap();
        assert_eq!(package.include_path, ["./"]);

        let root_json = r#"{"include-path": "./lib"}"#;
        let mut root_file = tempfile::NamedTempFile::new().unwrap();
        root_file.write_all(root_json.as_bytes()).unwrap();

        let root = read_root(root_file.path()).unwrap();
        assert_eq!(root.include_path, ["./lib"]);
    }

    #[test]
    fn vendor_dir_trims_trailing_slashes() {
        let json = r#"{"config": {"vendor-dir": "vendor/"}}"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(json.as_bytes()).unwrap();

        let root = read_root(file.path()).unwrap();
        assert_eq!(root.config.vendor_dir, "vendor");
    }

    #[test]
    fn read_lock_rejects_a_duplicate_package_name() {
        let lock_json = r#"{
            "packages": [
                {"name": "acme/tool", "version": "1.0.0"},
                {"name": "acme/tool", "version": "2.0.0"}
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let err = read_lock(file.path()).unwrap_err();
        assert!(
            err.to_string().contains("acme/tool"),
            "error should name the duplicate package: {err}"
        );
    }

    #[test]
    fn read_lock_rejects_a_duplicate_across_packages_and_packages_dev() {
        let lock_json = r#"{
            "packages": [
                {"name": "acme/tool", "version": "1.0.0"}
            ],
            "packages-dev": [
                {"name": "Acme/Tool", "version": "2.0.0"}
            ]
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let err = read_lock(file.path()).unwrap_err();
        assert!(
            err.to_string().contains("acme/tool"),
            "error should name the duplicate package: {err}"
        );
    }

    #[test]
    fn secure_http_defaults_true_and_is_overridable() {
        let root = read_root(&fixture("composer.json")).unwrap();
        assert!(root.config.secure_http);

        let json = r#"{"config": {"secure-http": false}}"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(json.as_bytes()).unwrap();
        let root = read_root(file.path()).unwrap();
        assert!(!root.config.secure_http);
    }

    #[test]
    fn bin_dir_defaults_to_vendor_dir_slash_bin() {
        // Real repro (#30): a project with a custom vendor-dir wrote bin
        // proxies to a stray top-level vendor/bin instead of following it.
        let json = r#"{"config": {"vendor-dir": "site/www/vendor"}}"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(json.as_bytes()).unwrap();

        let root = read_root(file.path()).unwrap();
        assert_eq!(root.config.bin_dir(), "site/www/vendor/bin");
    }

    #[test]
    fn bin_dir_explicit_stays_project_relative() {
        let json = r#"{"config": {"vendor-dir": "site/www/vendor", "bin-dir": "scripts/bin"}}"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(json.as_bytes()).unwrap();

        let root = read_root(file.path()).unwrap();
        assert_eq!(root.config.bin_dir(), "scripts/bin");
    }

    #[test]
    fn root_name_is_lowercased() {
        let json = r#"{"name": "Vendor/Package"}"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(json.as_bytes()).unwrap();

        let root = read_root(file.path()).unwrap();
        assert_eq!(root.name.as_deref(), Some("vendor/package"));
    }

    #[test]
    fn lock_package_name_is_lowercased_but_raw_is_untouched() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "Vendor/Package",
                    "version": "1.0.0"
                }
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let lock = read_lock(file.path()).unwrap();
        let package = lock.packages(true).next().unwrap();
        assert_eq!(package.name, "vendor/package");
        assert_eq!(
            package.raw.get("name").and_then(Value::as_str),
            Some("Vendor/Package")
        );
    }

    #[test]
    fn package_parse_error_includes_lock_path() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "acme/tool",
                    "version": 1
                }
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let err = read_lock(file.path()).unwrap_err();
        assert!(
            err.to_string().contains(&file.path().display().to_string()),
            "{err}"
        );
    }

    #[test]
    fn validate_dist_rejects_unsupported_type() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "psr/log",
                    "version": "1.0.0",
                    "dist": { "type": "rar", "url": "https://example.test/psr-log.rar" }
                }
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let lock = read_lock(file.path()).unwrap();
        let err = lock
            .packages(true)
            .next()
            .unwrap()
            .validate_dist()
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "psr/log: dist type \"rar\" is not supported in vivace v0.1 (zip, tar and path only)"
        );
    }

    #[test]
    fn validate_dist_accepts_tar() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "psr/log",
                    "version": "1.0.0",
                    "dist": { "type": "tar", "url": "https://example.test/psr-log.tar.gz" }
                }
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let lock = read_lock(file.path()).unwrap();
        lock.packages(true).next().unwrap().validate_dist().unwrap();
    }

    #[test]
    fn validate_dist_accepts_path() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "acme/hello",
                    "version": "1.0.0",
                    "dist": { "type": "path", "url": "packages/hello", "reference": "abc" },
                    "transport-options": { "relative": true }
                }
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let lock = read_lock(file.path()).unwrap();
        let package = lock.packages(true).next().unwrap();
        package.validate_dist().unwrap();
        assert!(package.is_path());
        assert!(!package.is_git_source());
        assert_eq!(package.transport_options.relative, Some(true));
    }

    #[test]
    fn validate_dist_accepts_a_dist_less_git_source() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "acme/vcslib",
                    "version": "dev-main",
                    "source": { "type": "git", "url": "https://example.test/acme/vcslib.git", "reference": "abc123" }
                }
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let lock = read_lock(file.path()).unwrap();
        let package = lock.packages(true).next().unwrap();
        package.validate_dist().unwrap();
        assert!(package.is_git_source());
        assert!(!package.is_path());
    }

    #[test]
    fn validate_dist_rejects_a_dist_less_non_git_source() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "acme/svnlib",
                    "version": "1.0.0",
                    "source": { "type": "svn", "url": "svn://example.test/acme/svnlib", "reference": "1" }
                }
            ],
            "packages-dev": []
        }"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();

        let lock = read_lock(file.path()).unwrap();
        let err = lock
            .packages(true)
            .next()
            .unwrap()
            .validate_dist()
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "acme/svnlib: no dist entry and no git source (svn/hg/fossil sources are not \
             supported; viv installs zip/tar dists, path repositories and git sources)"
        );
    }

    /// `Locker::getContentHash` on real `composer.json` files, values taken
    /// from a devbox PHP run of the exact same logic (see
    /// `docs/resolver-design.md`'s content-hash section).
    #[test]
    fn content_hash_matches_composer() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let file_cases = [
            (
                "tests/fixtures/monolog/composer.json",
                "23cc364856190039acdde2f5f07291f9",
            ),
            (
                "tests/fixtures/legacy/composer.json",
                "0493df0e1b7e8e3a9030a96ba7aa67ca",
            ),
            (
                "bench/laravel/composer.json",
                "65e3f5fe6eb7c640ae15bbb9ab9071b9",
            ),
        ];
        for (relative, expected) in file_cases {
            let bytes = fs_err::read(manifest.join(relative)).unwrap();
            assert_eq!(super::content_hash(&bytes).unwrap(), expected, "{relative}");
        }

        // A non-ASCII value in `require` and `extra`, including a character
        // outside the BMP (surrogate pair) and a slash in the package name.
        let unicode_json = r#"{
            "name": "vivace/fixture-hash-unicode",
            "require": {
                "acme/héllo": "^1.0",
                "psr/log": "^3.0"
            },
            "extra": {
                "note": "café 😀 emoji"
            }
        }"#;
        assert_eq!(
            super::content_hash(unicode_json.as_bytes()).unwrap(),
            "36a6bd7eab755fa44e566d6f484f3006"
        );

        // Nested `extra`/`config.platform` values: nested keys keep their
        // original order even though the top level is ksorted, and numbers
        // (int, float, array) round-trip.
        let nested_json = br#"{
            "name": "vivace/fixture-hash-nested",
            "version": "2.3.1",
            "require": { "monolog/monolog": "^3.0" },
            "conflict": { "foo/bar": "<1.0" },
            "minimum-stability": "dev",
            "prefer-stable": true,
            "extra": {
                "zeta": "z",
                "alpha": { "nested-b": 2, "nested-a": 1.5 },
                "beta": [1, 2, 3]
            },
            "config": {
                "platform": { "php": "8.2.0", "ext-mbstring": "1.0" },
                "sort-packages": true
            }
        }"#;
        assert_eq!(
            super::content_hash(nested_json).unwrap(),
            "91715c32a7cd35f43be62e33f01ba1a0"
        );
    }

    #[test]
    fn is_fresh_true_when_hash_matches_and_false_on_mismatch() {
        let root_json = fs_err::read(fixture("composer.json")).unwrap();
        let lock = read_lock(&fixture("composer.lock")).unwrap();
        assert!(is_fresh(&lock, &root_json).unwrap());

        let mut stale = lock.clone();
        stale.content_hash = Some("0".repeat(32));
        assert!(!is_fresh(&stale, &root_json).unwrap());
    }

    #[test]
    fn is_fresh_treats_a_lock_without_a_content_hash_as_fresh() {
        let lock_json = r#"{"packages": [], "packages-dev": []}"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.as_bytes()).unwrap();
        let lock = read_lock(file.path()).unwrap();
        assert!(is_fresh(&lock, b"{}").unwrap());
    }

    #[test]
    fn missing_requirements_names_absent_root_and_dev_requires() {
        let lock_json = r#"{
            "packages": [{"name": "psr/log", "version": "1.0.0"}],
            "packages-dev": []
        }"#;
        let mut lock_file = tempfile::NamedTempFile::new().unwrap();
        lock_file.write_all(lock_json.as_bytes()).unwrap();
        let lock = read_lock(lock_file.path()).unwrap();

        let root_json = r#"{
            "require": {"psr/log": "^1.0", "acme/missing": "^1.0", "php": "^8.2"},
            "require-dev": {"acme/missing-dev": "^1.0"}
        }"#;
        let mut root_file = tempfile::NamedTempFile::new().unwrap();
        root_file.write_all(root_json.as_bytes()).unwrap();
        let root = read_root(root_file.path()).unwrap();

        let no_dev = missing_requirements(&lock, &root, false);
        assert_eq!(no_dev.len(), 1);
        assert!(no_dev[0].contains("Required package \"acme/missing\""));

        let with_dev = missing_requirements(&lock, &root, true);
        assert!(
            with_dev
                .iter()
                .any(|line| line.contains("Required (in require-dev) package \"acme/missing-dev\""))
        );
    }

    #[test]
    fn missing_requirements_is_empty_when_everything_is_locked() {
        let lock = read_lock(&fixture("composer.lock")).unwrap();
        let root = read_root(&fixture("composer.json")).unwrap();
        assert_eq!(
            missing_requirements(&lock, &root, true),
            Vec::<String>::new()
        );
    }

    #[test]
    fn glob_match_covers_exact_wildcard_and_multi_wildcard_patterns() {
        assert!(glob_match("acme/lib", "acme/lib"));
        assert!(!glob_match("acme/lib", "acme/libx"));
        assert!(!glob_match("acme/lib", "xacme/lib"));
        assert!(glob_match("*", "anything/at-all"));
        assert!(glob_match("acme/*", "acme/lib"));
        assert!(!glob_match("acme/*", "other/lib"));
        assert!(glob_match("a*b*c", "axbyc"));
        assert!(!glob_match("a*b*c", "axbyd"));
        // `packageNameToRegexp` is case-insensitive.
        assert!(glob_match("ACME/*", "acme/lib"));
    }

    #[test]
    fn is_dev_version_matches_dev_prefix_and_suffix() {
        assert!(is_dev_version("dev-main"));
        assert!(is_dev_version("1.x-dev"));
        assert!(is_dev_version("9999999.9999999.9999999.9999999-dev"));
        assert!(!is_dev_version("1.2.3.0"));
        assert!(!is_dev_version("1.0.0.0-beta2"));
    }

    #[test]
    fn preferred_install_all_dist_never_prefers_source() {
        let pref = PreferredInstall::All(InstallPreference::Dist);
        assert!(!pref.prefers_source("acme/lib", true));
        assert!(!pref.prefers_source("acme/lib", false));
    }

    #[test]
    fn preferred_install_auto_prefers_source_only_for_dev_versions() {
        let pref = PreferredInstall::All(InstallPreference::Auto);
        assert!(pref.prefers_source("acme/lib", true));
        assert!(!pref.prefers_source("acme/lib", false));
    }

    #[test]
    fn preferred_install_map_matches_first_pattern_falling_back_to_auto() {
        let pref = PreferredInstall::Map(vec![
            ("acme/*".to_string(), InstallPreference::Source),
            ("*".to_string(), InstallPreference::Dist),
        ]);
        assert!(pref.prefers_source("acme/lib", false));
        assert!(!pref.prefers_source("other/lib", false));
        // No pattern matches: Composer's own default, source for dev.
        let unmatched =
            PreferredInstall::Map(vec![("acme/*".to_string(), InstallPreference::Dist)]);
        assert!(unmatched.prefers_source("other/lib", true));
        assert!(!unmatched.prefers_source("other/lib", false));
    }

    #[test]
    fn preferred_install_at_defaults_to_project_config_without_a_home_dir() {
        let project = PreferredInstall::All(InstallPreference::Source);
        let resolved = preferred_install_at(None, &project).unwrap();
        assert!(resolved.prefers_source("anything/at-all", false));
    }

    #[test]
    fn preferred_install_at_merges_global_map_under_project_map() {
        let home = tempfile::tempdir().unwrap();
        fs_err::write(
            home.path().join("config.json"),
            r#"{"config": {"preferred-install": {"global/*": "source", "*": "dist"}}}"#,
        )
        .unwrap();
        // The project only overrides one pattern; the global map's other
        // entries, and its wildcard, still apply, with `*` moved last.
        let project =
            PreferredInstall::Map(vec![("project/*".to_string(), InstallPreference::Source)]);
        let resolved = preferred_install_at(Some(home.path()), &project).unwrap();

        assert!(resolved.prefers_source("global/lib", false));
        assert!(resolved.prefers_source("project/lib", false));
        assert!(!resolved.prefers_source("other/lib", false));
    }

    #[test]
    fn preferred_install_at_project_string_replaces_global_string_outright() {
        let home = tempfile::tempdir().unwrap();
        fs_err::write(
            home.path().join("config.json"),
            r#"{"config": {"preferred-install": "source"}}"#,
        )
        .unwrap();
        let project = PreferredInstall::All(InstallPreference::Dist);
        let resolved = preferred_install_at(Some(home.path()), &project).unwrap();

        assert!(!resolved.prefers_source("anything/at-all", true));
    }
}
