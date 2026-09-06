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
#[derive(Debug, Clone)]
pub struct Dist {
    pub r#type: String,
    pub url: String,
    pub reference: Option<String>,
    pub shasum: Option<String>,
}

/// One package from `packages` or `packages-dev`.
#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub version: String,
    pub dist: Option<Dist>,
    pub autoload: Option<Value>,
    pub require: Map<String, Value>,
    pub provide: Map<String, Value>,
    pub replace: Map<String, Value>,
    pub r#type: String,
    pub target_dir: Option<String>,
    pub bin: Vec<String>,
    pub dev: bool,
    /// The untouched lock entry, key order preserved.
    pub raw: Value,
}

impl Package {
    /// vivace v0.1 only fetches zip dists; error clearly, naming the
    /// package, rather than failing obscurely later in `fetch`.
    pub fn validate_dist(&self) -> Result<()> {
        match &self.dist {
            None => bail!(
                "{}: no dist entry (path/git-only packages are not supported in vivace v0.1)",
                self.name
            ),
            Some(dist) if dist.r#type != "zip" => bail!(
                "{}: dist type \"{}\" is not supported in vivace v0.1 (zip only)",
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

#[derive(Debug, Deserialize)]
struct RawDist {
    r#type: String,
    url: String,
    reference: Option<String>,
    shasum: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawPackage {
    name: String,
    version: String,
    dist: Option<RawDist>,
    autoload: Option<Value>,
    #[serde(default)]
    require: Map<String, Value>,
    #[serde(default)]
    provide: Map<String, Value>,
    #[serde(default)]
    replace: Map<String, Value>,
    #[serde(rename = "type", default = "default_type")]
    r#type: String,
    #[serde(rename = "target-dir")]
    target_dir: Option<String>,
    #[serde(default)]
    bin: Vec<String>,
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
            packages.push(parse_package(entry, dev)?);
        }
    }

    Ok(Lock {
        content_hash,
        packages,
    })
}

fn parse_package(raw: &Value, dev: bool) -> Result<Package> {
    let name = raw
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let parsed: RawPackage = serde_json::from_value(raw.clone())
        .with_context(|| format!("parsing composer.lock package entry \"{name}\""))?;
    Ok(Package {
        name: parsed.name,
        version: parsed.version,
        dist: parsed.dist.map(|dist| Dist {
            r#type: dist.r#type,
            url: dist.url,
            reference: dist.reference,
            shasum: dist.shasum,
        }),
        autoload: parsed.autoload,
        require: parsed.require,
        provide: parsed.provide,
        replace: parsed.replace,
        r#type: parsed.r#type,
        target_dir: parsed.target_dir,
        bin: parsed.bin,
        dev,
        raw: raw.clone(),
    })
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

/// The root `composer.json`'s `config` block (the subset vivace reads).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(rename = "autoloader-suffix")]
    pub autoloader_suffix: Option<String>,
    #[serde(rename = "platform-check")]
    pub platform_check: PlatformCheck,
    #[serde(rename = "vendor-dir")]
    pub vendor_dir: String,
    #[serde(rename = "prepend-autoloader")]
    pub prepend_autoloader: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            autoloader_suffix: None,
            platform_check: PlatformCheck::PhpOnly,
            vendor_dir: "vendor".to_string(),
            prepend_autoloader: true,
        }
    }
}

/// The root `composer.json`, typed for the fields vivace needs.
#[derive(Debug, Clone, Deserialize)]
pub struct Root {
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
    pub config: Config,
}

/// Read and parse the root `composer.json`.
pub fn read_root(path: &Path) -> Result<Root> {
    let content = fs_err::read_to_string(path)?;
    serde_json::from_str(&content).with_context(|| format!("parsing {} as JSON", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{PlatformCheck, read_lock, read_root};
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
    fn validate_dist_rejects_non_zip() {
        let lock_json = r#"{
            "packages": [
                {
                    "name": "psr/log",
                    "version": "1.0.0",
                    "dist": { "type": "tar", "url": "https://example.test/psr-log.tar" }
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
            "psr/log: dist type \"tar\" is not supported in vivace v0.1 (zip only)"
        );
    }
}
