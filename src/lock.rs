//! Parsing `composer.lock` and the root `composer.json`.
//!
//! Each lock package keeps its untouched [`serde_json::Value`] (`raw`)
//! alongside typed fields, because `installed.json` re-emits lock entries in
//! their original key order (`serde_json`'s `preserve_order` feature).

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::{Map, Value};

/// A parsed `composer.lock`.
#[derive(Debug, Clone)]
pub struct Lock {
    pub content_hash: Option<String>,
    pub packages: Vec<Package>,
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

/// One package from `packages` or `packages-dev`.
#[derive(Debug, Clone, Deserialize)]
pub struct Package {
    #[serde(deserialize_with = "deserialize_lowercase")]
    pub name: String,
    pub version: String,
    pub dist: Option<Dist>,
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
}

impl Package {
    /// vivace v0.1 only fetches zip and tar dists (tar covers `.tar`,
    /// `.tar.gz`/`.tgz` and `.tar.bz2`: Composer's own `dist.type` is `"tar"`
    /// for all three, distinguished by the archive bytes, not the type
    /// string); error clearly, naming the package, rather than failing
    /// obscurely later in `fetch`.
    pub fn validate_dist(&self) -> Result<()> {
        match &self.dist {
            None => bail!(
                "{}: no dist entry (path/git-only packages are not supported in vivace v0.1)",
                self.name
            ),
            Some(dist) if dist.r#type != "zip" && dist.r#type != "tar" => bail!(
                "{}: dist type \"{}\" is not supported in vivace v0.1 (zip and tar only)",
                self.name,
                dist.r#type
            ),
            Some(_) => Ok(()),
        }
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
    for (key, dev) in [("packages", false), ("packages-dev", true)] {
        for entry in raw.get(key).and_then(Value::as_array).into_iter().flatten() {
            packages.push(parse_package(entry, dev, path)?);
        }
    }

    Ok(Lock {
        content_hash,
        packages,
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
    /// `vendor/bin` by default, independent of `vendor-dir`, as Composer
    /// has it (see `bin::generate`, `src/bin.rs`).
    #[serde(rename = "bin-dir")]
    pub bin_dir: String,
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
}

impl Default for Config {
    fn default() -> Self {
        Config {
            autoloader_suffix: None,
            platform_check: PlatformCheck::PhpOnly,
            vendor_dir: "vendor".to_string(),
            prepend_autoloader: true,
            bin_dir: "vendor/bin".to_string(),
            bin_compat: crate::bin::BinCompat::Auto,
            optimize_autoloader: false,
            classmap_authoritative: false,
            apcu_autoloader: false,
            apcu_autoloader_prefix: None,
            use_include_path: false,
        }
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
}

/// Read and parse the root `composer.json`.
pub fn read_root(path: &Path) -> Result<Root> {
    let content = fs_err::read_to_string(path)?;
    serde_json::from_str(&content).with_context(|| format!("parsing {} as JSON", path.display()))
}

/// Composer's `Locker::getContentHash`: an md5 of the sorted, compact JSON
/// of the `composer.json` keys that decide what a lock should contain.
fn content_hash(root_json: &[u8]) -> Result<String> {
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
    let content: Value = serde_json::from_slice(root_json).context("parsing composer.json")?;
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
    // ponytail: PHP's json_encode escapes `/`; serde_json doesn't. Harmless
    // for the freshness comparison below (both sides go through this same
    // function), but a hash computed here won't match a real Composer one
    // byte-for-byte if a value contains a literal slash.
    let encoded = serde_json::to_string(&relevant)?;
    Ok(format!("{:x}", md5::compute(encoded)))
}

/// Composer's `Locker::isFresh`: does `lock`'s `content-hash` still match
/// `root_json` (the root `composer.json` bytes)? A lock without a
/// `content-hash` at all has nothing to compare, so it is treated as fresh.
pub fn validate_against_root(lock: &Lock, root_json: &[u8]) -> Result<()> {
    let Some(locked_hash) = &lock.content_hash else {
        return Ok(());
    };
    let current = content_hash(root_json)?;
    if &current != locked_hash {
        bail!(
            "The lock file is not up to date with the latest changes in composer.json \
             (content-hash mismatch: locked {locked_hash}, current {current})"
        );
    }
    Ok(())
}

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
/// `root`'s required packages locked at all? Whether the locked version
/// actually *satisfies* the root constraint needs a semver solver, which
/// vivace's no-dependency-resolution planner deliberately doesn't have; that
/// half of Composer's check is out of scope for v0.1.
pub fn missing_requirements(lock: &Lock, root: &Root, dev: bool) -> Vec<String> {
    let locked: std::collections::HashSet<String> =
        lock.packages(dev).map(|p| p.name.clone()).collect();
    root.require
        .keys()
        .filter(|name| !is_platform_package(name))
        .filter(|name| !locked.contains(&name.to_lowercase()))
        .map(|name| format!("- Required package \"{name}\" is not present in the lock file."))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{PlatformCheck, read_lock, read_root};
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
            "psr/log: dist type \"rar\" is not supported in vivace v0.1 (zip and tar only)"
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
}
