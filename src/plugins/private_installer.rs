//! `ffraenz/private-composer-installer`'s `Plugin::handlePreDownloadEvent`:
//! before a dist archive is downloaded, `{%NAME}` placeholders in its URL
//! are replaced — `{%version}` (case-insensitive) with the package's own
//! version, every other name with an environment variable. Ported from
//! `ffraenz/private-composer-installer` `5.0.1`'s `src/Plugin.php` and
//! `src/Environment/*`, fetched 2026-09-08 (#98).
//!
//! Two things the real plugin does are deliberately *not* mirrored here,
//! having re-read `Plugin::getSubscribedEvents`: Composer 1's
//! `PRE_PACKAGE_INSTALL`/`PRE_PACKAGE_UPDATE` listener writes the
//! version-fulfilled URL back onto the package before it reaches
//! `composer.lock` — but that listener is only subscribed
//! `self::isComposer1()`, so it never runs under Composer 2, which vivace
//! matches; Composer 2's own `handlePreDownloadEvent` never calls
//! `$package->setDistUrl`, only `$event->setProcessedUrl`/
//! `setCustomCacheKey`, so a placeholder — `{%version}` included — always
//! stays literal in `composer.lock`/`installed.json`. [`resolve`] is
//! therefore only ever called on the URL actually requested over HTTP
//! (`fetch::Fetcher::fetch`), never on `Package::dist`/`Package::raw`
//! itself. Composer 2's "version not present anywhere in the URL" branch
//! (`fulfillVersionPlaceholder`'s `elseif`) appends `#v<version>` to force a
//! cache-busting refetch of *Composer's own* HTTP cache; vivace's store
//! already keys a dist download by `dist.reference`/`dist.url` from the
//! lock (see `store::Store`'s `pointer`), which already changes across a
//! version bump, so that branch has nothing to bust here and isn't ported.
//!
//! ponytail: the dotenv reader below covers `KEY=VALUE` lines, optional
//! `export `, and single/double-quoted values — not phpdotenv's `${VAR}`
//! interpolation inside double-quoted values or its backslash escapes.
//! Every local `.env` fixture in the three client projects #98 exists for
//! is a flat list of `KEY=value` lines, so this hasn't bitten yet; port
//! `Dotenv\Parser\Parser`'s interpolation pass if a project ever needs it.
//!
//! ponytail: `dotenv-path`/no-path resolution reads the *first* directory
//! (given, or found searching upward) that has the named file, not every
//! ancestor merged together the way phpdotenv's multi-path `Store` can.
//! Every project #98 exists for keeps exactly one `.env`, so this hasn't
//! bitten either; revisit if a project ever splits one across ancestors.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::{Captures, Regex};
use serde_json::Value;

use crate::lock::Root;

use super::Adapter;

pub(super) struct PrivateInstaller;

impl Adapter for PrivateInstaller {
    fn plugin_names(&self) -> &'static [&'static str] {
        &["ffraenz/private-composer-installer"]
    }

    fn upstream_version(&self) -> &'static str {
        "5.0.1"
    }

    fn fetch_env(&self, root: &Root, project_dir: &Path) -> Option<Env> {
        Some(Env::load(root, project_dir))
    }
}

/// `/{%([A-Za-z0-9-_]+)}/` (`Plugin::identifyPlaceholders`'s own regex).
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{%([A-Za-z0-9_-]+)\}").expect("valid regex"));

/// `/{%version}/i` (`Plugin::fulfillVersionPlaceholder`'s own pattern) —
/// `version` is the one placeholder name matched case-insensitively.
static VERSION_PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\{%version\}").expect("valid regex"));

/// The environment a dist URL's placeholders resolve against: the process
/// environment (wins), falling back to a project's `.env` (fills gaps).
/// Built once per install by [`Env::load`].
pub struct Env {
    dotenv: HashMap<String, String>,
}

impl Env {
    /// `extra.private-composer-installer.dotenv-path`/`.dotenv-name`
    /// resolve which `.env` file backs this environment, same as
    /// `LoaderFactory::create`; a missing file leaves `dotenv` empty, same
    /// as the real loader's `->safeLoad()`.
    pub fn load(root: &Root, project_dir: &Path) -> Env {
        let config = root.extra.get("private-composer-installer");
        let dotenv_path = config
            .and_then(|c| c.get("dotenv-path"))
            .and_then(Value::as_str);
        let dotenv_name = config
            .and_then(|c| c.get("dotenv-name"))
            .and_then(Value::as_str)
            .unwrap_or(".env");
        let dotenv = find_dotenv(project_dir, dotenv_path, dotenv_name)
            .and_then(|path| fs_err::read_to_string(path).ok())
            .map(|content| parse_dotenv(&content))
            .unwrap_or_default();
        Env { dotenv }
    }

    /// `DotenvRepository::get`: the process environment's value for `name`
    /// if non-empty, else `.env`'s, else a `MissingEnvException`-equivalent
    /// error naming `name`.
    fn get(&self, name: &str) -> Result<String> {
        if let Ok(value) = std::env::var(name)
            && !value.is_empty()
        {
            return Ok(value);
        }
        if let Some(value) = self.dotenv.get(name)
            && !value.is_empty()
        {
            return Ok(value.clone());
        }
        bail!(
            "can't resolve placeholder {{%{name}}}: environment variable '{name}' is not set \
             (see docs/plugin-strategy.md's ffraenz/private-composer-installer entry)"
        );
    }
}

/// `Plugin::handlePreDownloadEvent`'s substitution, applied to one dist
/// URL: `{%version}` (any case) becomes `version`, then every other
/// `{%NAME}` becomes `env.get(NAME)`. Errors naming the first unresolved
/// variable, same as the real plugin's `MissingEnvException`.
pub fn resolve(url: &str, version: &str, env: &Env) -> Result<String> {
    let mut url = VERSION_PLACEHOLDER
        .replace_all(url, |_: &Captures<'_>| version.to_string())
        .into_owned();

    let mut names: Vec<String> = Vec::new();
    for caps in PLACEHOLDER.captures_iter(&url) {
        let name = caps[1].to_string();
        if !names.contains(&name) {
            names.push(name);
        }
    }
    for name in names {
        let value = env.get(&name)?;
        url = url.replace(&format!("{{%{name}}}"), &value);
    }
    Ok(url)
}

/// `LoaderFactory::create`'s path resolution: `dotenv_path` (if set) is a
/// single directory to look in; otherwise every ancestor of `project_dir`
/// up to the filesystem root is tried, nearest first.
fn find_dotenv(
    project_dir: &Path,
    dotenv_path: Option<&str>,
    dotenv_name: &str,
) -> Option<PathBuf> {
    if let Some(path) = dotenv_path {
        let dir = if Path::new(path).is_absolute() {
            PathBuf::from(path)
        } else {
            project_dir.join(path)
        };
        let file = dir.join(dotenv_name);
        return file.is_file().then_some(file);
    }
    let mut dir = Some(project_dir.to_path_buf());
    while let Some(candidate) = dir {
        let file = candidate.join(dotenv_name);
        if file.is_file() {
            return Some(file);
        }
        dir = candidate.parent().map(Path::to_path_buf);
    }
    None
}

/// A minimal `KEY=VALUE` reader: blank lines and `#` comments are skipped,
/// an optional leading `export ` is stripped, and a value wrapped in
/// matching single or double quotes has them removed. See this module's own
/// doc comment for what phpdotenv features this leaves out.
fn parse_dotenv(content: &str) -> HashMap<String, String> {
    let mut vars = HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = value.trim();
        let value = strip_matching_quotes(value);
        vars.insert(key.to_string(), value.to_string());
    }
    vars
}

fn strip_matching_quotes(value: &str) -> &str {
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'"' || bytes[0] == b'\'')
        && bytes[0] == bytes[bytes.len() - 1]
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn root(extra: &Value) -> Root {
        serde_json::from_value(json!({"extra": extra})).unwrap()
    }

    fn env_with_dotenv(pairs: &[(&str, &str)]) -> Env {
        Env {
            dotenv: pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn resolve_substitutes_version_case_insensitively() {
        let env = env_with_dotenv(&[]);
        let got = resolve("https://example.test/pkg-{%VERSION}.zip", "1.2.3", &env).unwrap();
        assert_eq!(got, "https://example.test/pkg-1.2.3.zip");
    }

    #[test]
    fn resolve_substitutes_env_placeholder_from_dotenv() {
        let env = env_with_dotenv(&[("ACME_KEY", "secret")]);
        let got = resolve("https://example.test/dist-{%ACME_KEY}.zip", "1.0.0", &env).unwrap();
        assert_eq!(got, "https://example.test/dist-secret.zip");
    }

    #[test]
    #[allow(
        unsafe_code,
        reason = "a var name unique to this test, restored before it returns"
    )]
    fn resolve_prefers_process_env_over_dotenv() {
        // SAFETY: name unique to this test; no other test reads or writes it.
        unsafe {
            std::env::set_var("VIV_TEST_PRIVATE_INSTALLER_KEY", "from-process");
        }
        let env = env_with_dotenv(&[("VIV_TEST_PRIVATE_INSTALLER_KEY", "from-dotenv")]);
        let got = resolve(
            "https://example.test/dist-{%VIV_TEST_PRIVATE_INSTALLER_KEY}.zip",
            "1.0.0",
            &env,
        )
        .unwrap();
        // SAFETY: matches the set_var above.
        unsafe {
            std::env::remove_var("VIV_TEST_PRIVATE_INSTALLER_KEY");
        }
        assert_eq!(got, "https://example.test/dist-from-process.zip");
    }

    #[test]
    fn resolve_errors_naming_a_missing_variable() {
        let env = env_with_dotenv(&[]);
        let err = resolve("https://example.test/dist-{%ACME_KEY}.zip", "1.0.0", &env)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ACME_KEY"), "{err}");
    }

    #[test]
    fn resolve_errors_on_an_empty_variable() {
        let env = env_with_dotenv(&[("ACME_KEY", "")]);
        let err = resolve("https://example.test/dist-{%ACME_KEY}.zip", "1.0.0", &env)
            .unwrap_err()
            .to_string();
        assert!(err.contains("ACME_KEY"), "{err}");
    }

    #[test]
    fn resolve_is_a_no_op_without_placeholders() {
        let env = env_with_dotenv(&[]);
        let got = resolve("https://example.test/dist.zip", "1.0.0", &env).unwrap();
        assert_eq!(got, "https://example.test/dist.zip");
    }

    #[test]
    fn parse_dotenv_reads_export_and_quoted_values() {
        let vars = parse_dotenv(
            "# a comment\n\nexport ACME_KEY=\"quoted value\"\nOTHER='single quoted'\nPLAIN=bare\n",
        );
        assert_eq!(
            vars.get("ACME_KEY").map(String::as_str),
            Some("quoted value")
        );
        assert_eq!(vars.get("OTHER").map(String::as_str), Some("single quoted"));
        assert_eq!(vars.get("PLAIN").map(String::as_str), Some("bare"));
    }

    #[test]
    fn find_dotenv_searches_upward_from_project_dir() {
        let root_dir = tempfile::tempdir().unwrap();
        fs_err::write(root_dir.path().join(".env"), "ACME_KEY=root\n").unwrap();
        let project_dir = root_dir.path().join("a/b");
        fs_err::create_dir_all(&project_dir).unwrap();
        let found = find_dotenv(&project_dir, None, ".env").unwrap();
        assert_eq!(found, root_dir.path().join(".env"));
    }

    #[test]
    fn find_dotenv_prefers_a_configured_path_over_upward_search() {
        let root_dir = tempfile::tempdir().unwrap();
        fs_err::write(root_dir.path().join(".env"), "ACME_KEY=root\n").unwrap();
        let configured_dir = root_dir.path().join("config");
        fs_err::create_dir_all(&configured_dir).unwrap();
        fs_err::write(configured_dir.join(".env"), "ACME_KEY=configured\n").unwrap();
        let found = find_dotenv(root_dir.path(), Some("config"), ".env").unwrap();
        assert_eq!(found, configured_dir.join(".env"));
    }

    #[test]
    fn env_load_reads_dotenv_name_and_path_from_root_extra() {
        let project_dir = tempfile::tempdir().unwrap();
        let configured_dir = project_dir.path().join("secrets");
        fs_err::create_dir_all(&configured_dir).unwrap();
        fs_err::write(configured_dir.join("custom.env"), "ACME_KEY=from-file\n").unwrap();
        let root = root(&json!({
            "private-composer-installer": {
                "dotenv-path": "secrets",
                "dotenv-name": "custom.env"
            }
        }));
        let env = Env::load(&root, project_dir.path());
        assert_eq!(env.get("ACME_KEY").unwrap(), "from-file");
    }
}
