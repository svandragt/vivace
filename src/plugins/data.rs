//! Path-mapping plugin adapters as data (#340): `composer/installers` and
//! the `wordpress-core-installer` pair only ever compute *where* a package
//! lands, and both do it from data rather than code — a package-type
//! default location table (`type-template`) or a root/package `extra` key
//! whose value is used verbatim (`named-override`). One embedded
//! `include_str!`'d file per plugin package name lives under
//! `src/plugins/data/`; [`rules`] parses every one of them once per process,
//! and [`install_path`] is the single seam `installers.rs`/`wordpress_core.rs`
//! call instead of keeping a table of their own.
//!
//! `composer/installers`' own `installer-paths` selector-map override
//! (first entry whose value list contains the package's exact name,
//! `type:<type>` or `vendor:<vendor>` wins) and the `WordPress` core
//! installer pair's own string-or-by-name-map override are different
//! mechanisms, not two forms of the same one — `Rule::TypeTemplate` and
//! `Rule::NamedOverride` name that difference instead of bending one shape
//! to cover both, or special-casing a plugin name in the loader.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

use crate::lock::Package;

/// Every embedded data file. Order never matters: each rule only ever
/// answers for the package `type`(s) it names, so at most one can match a
/// given package.
const FILES: &[&str] = &[include_str!("data/composer-installers.toml")];

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
enum Rule {
    /// `composer/installers`' `BaseInstaller::getInstallPath`: a
    /// package-type default location table, plus the root `extra`'s
    /// selector-map override.
    TypeTemplate {
        /// `type`'s "framework" prefix (the part before its first `-`) ->
        /// package-type suffix -> install-path template. A template's
        /// `{$name}`/`{$vendor}`/`{$type}` are substituted
        /// (`BaseInstaller::templatePath`); any other `{$var}` (a
        /// CMS-specific one like `{$bitrix_dir}`) is left untouched, same
        /// as Composer's own `extract`-based substitution leaves an
        /// undefined variable empty — vivace only ever exercises the
        /// `WordPress` installers' three vars, so the difference never
        /// surfaces in practice.
        types: HashMap<String, HashMap<String, String>>,
        /// Root `extra` key holding the selector-map override
        /// (`installer-paths`).
        #[serde(rename = "paths-key")]
        paths_key: String,
        /// Package `extra` key overriding the `{$name}` template var alone
        /// (`installer-name`).
        #[serde(rename = "name-key")]
        name_key: String,
    },
    /// The `WordPress` core installer pair's `getInstallPath`: one package
    /// `type`, one `extra` key whose value (root, then package) is used
    /// verbatim, no template.
    NamedOverride {
        r#type: String,
        key: String,
        default: String,
    },
}

impl Rule {
    fn install_path(&self, package: &Package, root_extra: &Value) -> Option<String> {
        match self {
            Rule::TypeTemplate {
                types,
                paths_key,
                name_key,
            } => type_template_path(types, paths_key, name_key, package, root_extra),
            Rule::NamedOverride {
                r#type,
                key,
                default,
            } => (package.r#type == *r#type)
                .then(|| named_override_path(key, default, package, root_extra)),
        }
    }
}

/// `composer/installers`' `installer_path` (moved here verbatim from
/// `installers.rs`, #340): the root's `extra[paths_key]` (first pattern
/// whose value list contains the package's name, `type:<type>` or
/// `vendor:<vendor>` wins), else `types`' own default location for the
/// package's type.
fn type_template_path(
    types: &HashMap<String, HashMap<String, String>>,
    paths_key: &str,
    name_key: &str,
    package: &Package,
    root_extra: &Value,
) -> Option<String> {
    let (vendor, name) = package
        .name
        .split_once('/')
        .unwrap_or(("", package.name.as_str()));
    let name = package
        .raw
        .pointer(&format!("/extra/{name_key}"))
        .and_then(Value::as_str)
        .unwrap_or(name);
    let vars = [
        ("name", name),
        ("vendor", vendor),
        ("type", package.r#type.as_str()),
    ];

    if let Some(paths) = root_extra.get(paths_key).and_then(Value::as_object) {
        for (pattern, names) in paths {
            let names = string_or_vec(names);
            // Composer matches the *pretty* package name here
            // (`vendor/name`), not the `{$name}` template var above, which
            // may already be the `extra.installer-name` override.
            let hit = names.iter().any(|n| {
                n == &package.name
                    || n == &format!("type:{}", package.r#type)
                    || n == &format!("vendor:{vendor}")
            });
            if hit {
                return Some(template_path(pattern, &vars));
            }
        }
    }

    let (framework_type, locations) = types
        .iter()
        .filter(|(key, _)| {
            package.r#type.starts_with(key.as_str())
                && package.r#type.as_bytes().get(key.len()) == Some(&b'-')
        })
        .max_by_key(|(key, _)| key.len())?;
    let package_type = &package.r#type[framework_type.len() + 1..];
    let location = locations.get(package_type)?;
    Some(template_path(location, &vars))
}

/// The `WordPress` core installer pair's `wordpress_install_dir` (moved
/// here verbatim from `wordpress_core.rs`, #340): the root's `extra[key]`
/// (a string, or a map keyed by the package's pretty name), falling back to
/// the package's own `extra[key]`, then `default`.
fn named_override_path(key: &str, default: &str, package: &Package, root_extra: &Value) -> String {
    let from_root = root_extra.get(key).and_then(|v| {
        v.as_str().map(str::to_owned).or_else(|| {
            v.as_object()
                .and_then(|m| m.get(&package.name))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    });
    from_root
        .or_else(|| {
            package
                .raw
                .pointer(&format!("/extra/{key}"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| default.to_string())
}

/// `extra.installer-paths`' value: a single string or an array of strings.
fn string_or_vec(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// `BaseInstaller::templatePath`: replace every `{$var}` with `vars`' value
/// for that name; a var this package type doesn't supply is left untouched.
fn template_path(path: &str, vars: &[(&str, &str)]) -> String {
    let mut path = path.to_string();
    for (name, value) in vars {
        path = path.replace(&format!("{{${name}}}"), value);
    }
    path
}

/// Every embedded file, parsed once per process.
fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        FILES
            .iter()
            .map(|content| toml::from_str(content).expect("embedded plugin data file"))
            .collect()
    })
}

/// The project-relative install directory `package` maps to under `root`'s
/// `extra` (`docs/plugin-strategy.md`'s rule 1: `composer/installers` and
/// the `WordPress` core installer pair), or `None` to leave the package at
/// the default `vendor/<name>`. `installers.rs`/`wordpress_core.rs`'
/// `install_dir` are the only callers.
pub(super) fn install_path(package: &Package, root_extra: &Value) -> Option<PathBuf> {
    rules()
        .iter()
        .find_map(|rule| rule.install_path(package, root_extra))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn package(name: &str, r#type: &str) -> Package {
        let raw = json!({ "name": name, "version": "1.0.0", "type": r#type });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    /// Every `composer/installers` table entry resolves to the same path
    /// the old Rust `INSTALLER_TYPES` table gave for `acme/hello`, recorded
    /// as `tests/fixtures/plugins/composer-installers/expected-paths.tsv`
    /// (`package-type<TAB>template<TAB>expected-path`) *before*
    /// `INSTALLER_TYPES` was deleted, so a transcription slip into TOML
    /// fails loudly instead of silently losing a framework's rule.
    #[test]
    fn composer_installers_table_matches_the_old_rust_table() {
        const EXPECTED: &str =
            include_str!("../../tests/fixtures/plugins/composer-installers/expected-paths.tsv");
        let mut checked = 0;
        for line in EXPECTED.lines() {
            let mut columns = line.splitn(3, '\t');
            let package_type = columns.next().unwrap();
            let _template = columns.next().unwrap();
            let expected = columns.next().unwrap();
            let got = install_path(&package("acme/hello", package_type), &json!({}))
                .unwrap_or_else(|| panic!("{package_type}: no rule matched"));
            assert_eq!(got.to_str().unwrap(), expected, "{package_type}");
            checked += 1;
        }
        assert_eq!(checked, 288, "expected-paths.tsv row count changed");
    }

    #[test]
    fn unhandled_type_returns_none() {
        assert!(install_path(&package("acme/hello", "library"), &json!({})).is_none());
    }
}
