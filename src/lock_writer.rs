//! Writes a `composer.lock`, a port of `Package\Locker::setLockData` (top-level
//! key order, `fixupJsonDataType`) and `lockPackages` (`ArrayDumper::dump`
//! reused for each package entry, then `version_normalized`/`installation-source`
//! dropped and `time` moved to the end).
//!
//! `content_hash`/`php_json_encode` below are copied from `lock.rs` rather
//! than called (they aren't `pub` there, and that module belongs to another
//! lane right now, so widening their visibility isn't this lane's call to
//! make): `Locker::getContentHash`'s md5-of-sorted-compact-JSON is a stable,
//! small piece of logic, worth duplicating rather than gated on that.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::solver::transaction::{AliasEntry, ResolvedPackage};

/// `Locker::PLUGIN_API_VERSION`/`PluginInterface::PLUGIN_API_VERSION`, this
/// Composer's bundled plugin API version: hardcoded rather than detected,
/// since it changes only with a Composer release, not per project. Guarded
/// by `plugin_api_version_matches_composer` so a Composer upgrade in
/// `devbox.json` that bumps it fails a test loudly instead of silently
/// drifting.
const PLUGIN_API_VERSION: &str = "2.9.0";

/// `ArrayDumper::dump`'s key order, minus `version_normalized` and
/// `installation-source` (`lockPackages` drops both) and `time` (moved to
/// the very end by `lockPackages`, appended separately below rather than
/// listed here).
const KEY_ORDER: &[&str] = &[
    "name",
    "version",
    "target-dir",
    "source",
    "dist",
    "require",
    "conflict",
    "provide",
    "replace",
    "require-dev",
    "suggest",
    "default-branch",
    "bin",
    "type",
    "extra",
    "autoload",
    "autoload-dev",
    "notification-url",
    "include-path",
    "php-ext",
    "archive",
    "scripts",
    "license",
    "authors",
    "description",
    "homepage",
    "keywords",
    "repositories",
    "support",
    "funding",
    "abandoned",
    "transport-options",
];

/// `ArrayDumper::dump`'s `ksort($data[$type])` per link type, plus `suggest`.
const KSORTED: &[&str] = &[
    "require",
    "conflict",
    "provide",
    "replace",
    "require-dev",
    "suggest",
];

const SOURCE_KEY_ORDER: &[&str] = &["type", "url", "reference", "mirrors"];
const DIST_KEY_ORDER: &[&str] = &["type", "url", "reference", "shasum", "mirrors"];

/// Everything `Locker::setLockData` needs beyond the two package lists
/// (`solver::solve_update`'s `UpdateResult`, minus `non_dev`/`dev` which are
/// passed separately so a caller doing only the no-dev half doesn't have to
/// hand back the rest of the struct just to move it).
pub struct LockOptions<'a> {
    pub minimum_stability: &'a str,
    pub stability_flags: &'a HashMap<String, u8>,
    pub prefer_stable: bool,
    pub prefer_lowest: bool,
    pub platform_reqs: &'a Map<String, Value>,
    pub platform_dev_reqs: &'a Map<String, Value>,
    pub platform_overrides: &'a Map<String, Value>,
    pub aliases: &'a [AliasEntry],
}

/// The full `composer.lock` body, `JsonFile::encode`'s default flags
/// (`JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE`,
/// four-space indent) with the trailing newline `JsonFile::write` appends.
/// `serde_json`'s own pretty-printer never escapes `/` or non-ASCII, so
/// reaching for its defaults already matches those two flags; no
/// `php_json_encode`-style writer needed here (that one exists only for
/// `content_hash`'s *compact*, PHP-`json_encode`-exact encoding).
pub fn write(
    non_dev: &[ResolvedPackage],
    dev: Option<&[ResolvedPackage]>,
    options: &LockOptions,
    composer_json: &[u8],
) -> Result<String> {
    let mut lock = Map::new();
    lock.insert(
        "_readme".into(),
        Value::Array(
            [
                "This file locks the dependencies of your project to a known state",
                "Read more about it at https://getcomposer.org/doc/01-basic-usage.md#installing-dependencies",
                "This file is @generated automatically",
            ]
            .into_iter()
            .map(|s| Value::String(s.to_string()))
            .collect(),
        ),
    );
    lock.insert(
        "content-hash".into(),
        Value::String(content_hash(composer_json)?),
    );
    lock.insert("packages".into(), lock_packages(non_dev)?);
    lock.insert(
        "packages-dev".into(),
        match dev {
            Some(dev) => lock_packages(dev)?,
            None => Value::Null,
        },
    );
    lock.insert("aliases".into(), dump_aliases(options.aliases));
    lock.insert(
        "minimum-stability".into(),
        Value::String(options.minimum_stability.to_string()),
    );
    lock.insert(
        "stability-flags".into(),
        Value::Object(
            options
                .stability_flags
                .iter()
                .map(|(name, &rank)| (name.clone(), Value::from(rank)))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
    );
    lock.insert("prefer-stable".into(), Value::Bool(options.prefer_stable));
    lock.insert("prefer-lowest".into(), Value::Bool(options.prefer_lowest));
    lock.insert(
        "platform".into(),
        Value::Object(options.platform_reqs.clone()),
    );
    lock.insert(
        "platform-dev".into(),
        Value::Object(options.platform_dev_reqs.clone()),
    );
    if !options.platform_overrides.is_empty() {
        lock.insert(
            "platform-overrides".into(),
            Value::Object(options.platform_overrides.clone()),
        );
    }
    lock.insert(
        "plugin-api-version".into(),
        Value::String(PLUGIN_API_VERSION.to_string()),
    );

    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&Value::Object(lock), &mut serializer)?;
    buf.push(b'\n');
    Ok(String::from_utf8(buf)?)
}

/// `Locker::lockPackages`: dump each package in `ArrayDumper` order, drop
/// `version_normalized`/`installation-source`, move `time` to the end, sort
/// by name then version.
fn lock_packages(packages: &[ResolvedPackage]) -> Result<Value> {
    let mut dumped = packages
        .iter()
        .map(|p| dump_package(&p.raw))
        .collect::<Result<Vec<_>>>()?;
    dumped.sort_by(|a, b| {
        let name_cmp = a["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["name"].as_str().unwrap_or_default());
        if name_cmp != std::cmp::Ordering::Equal {
            return name_cmp;
        }
        a["version"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["version"].as_str().unwrap_or_default())
    });
    Ok(Value::Array(dumped))
}

fn dump_package(raw: &Value) -> Result<Value> {
    let raw = raw.as_object().cloned().unwrap_or_default();
    let mut out = Map::new();
    for &key in KEY_ORDER {
        let value = match key {
            "name" | "version" => raw
                .get(key)
                .cloned()
                .with_context(|| format!("provider entry missing {key:?}"))?,
            "source" => match raw.get("source") {
                Some(Value::Object(s)) => reorder(s, SOURCE_KEY_ORDER),
                _ => continue,
            },
            "dist" => match raw.get("dist") {
                Some(Value::Object(d)) => reorder(d, DIST_KEY_ORDER),
                _ => continue,
            },
            // `ArrayLoader::configureObject`'s `!empty($config[$key])` gate
            // (rather than the plain `isset()` most keys get): PHP's
            // `empty()` treats `""`/`"0"`/`0`/`false` the same as absent, so
            // a provider file's `"homepage": ""` (real Packagist data) never
            // reaches `CompletePackage::setHomepage`, and the getter
            // `ArrayDumper::dumpValues` reads back is never anything but
            // its unset default.
            "time" | "notification-url" | "description" | "homepage" | "keywords" | "license"
            | "authors" | "funding"
                if raw.get(key).is_some_and(is_empty_for_composer) =>
            {
                continue;
            }
            _ => match raw.get(key) {
                None | Some(Value::Null | Value::Bool(false)) => continue,
                Some(Value::Array(a)) if a.is_empty() => continue,
                Some(Value::Object(o)) if o.is_empty() => continue,
                Some(v) => v.clone(),
            },
        };
        let value = match (key, value) {
            (k, Value::Object(map)) if KSORTED.contains(&k) => {
                let mut entries: Vec<(String, Value)> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Value::Object(entries.into_iter().collect())
            }
            ("keywords", Value::Array(mut words)) => {
                words.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                Value::Array(words)
            }
            (_, v) => v,
        };
        if (key == "source" || key == "dist")
            && let Value::Object(o) = &value
            && o.is_empty()
        {
            continue;
        }
        out.insert(key.into(), value);
    }
    // `lockPackages`: `time` is unset then re-added, moving it to the end.
    if let Some(time) = raw.get("time").filter(|v| !is_empty_for_composer(v)) {
        out.insert("time".into(), time.clone());
    }
    Ok(Value::Object(out))
}

/// PHP's `empty($value)`: null, `false`, `0`, `"0"`, `""` and an empty
/// array/object are all "empty", unlike this module's usual null/empty-array
/// check.
fn is_empty_for_composer(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty() || s == "0",
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// Picks `keys` from `map` in order, skipping any that are absent or null.
fn reorder(map: &Map<String, Value>, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for &key in keys {
        match map.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Array(a)) if a.is_empty() => {}
            Some(v) => {
                out.insert(key.into(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// `LockTransaction::getAliases`'s result, then `Locker::setLockData`'s
/// legacy `dev-master`/`dev-trunk`/`dev-default` -> `9999999-dev` BC fixup
/// on the `version` field (Composer 1's lock shape; harmless to keep since
/// `VersionParser::normalize` never actually produces those three literals
/// for `dev-*` input today, but ported anyway for fidelity).
fn dump_aliases(aliases: &[AliasEntry]) -> Value {
    Value::Array(
        aliases
            .iter()
            .map(|alias| {
                let version = match alias.version.as_str() {
                    "dev-master" | "dev-trunk" | "dev-default" => "9999999-dev".to_string(),
                    _ => alias.version.clone(),
                };
                let mut entry = Map::new();
                entry.insert("package".into(), Value::String(alias.package.clone()));
                entry.insert("version".into(), Value::String(version));
                entry.insert("alias".into(), Value::String(alias.alias.clone()));
                entry.insert(
                    "alias_normalized".into(),
                    Value::String(alias.alias_normalized.clone()),
                );
                Value::Object(entry)
            })
            .collect(),
    )
}

/// `Locker::getContentHash`: an md5 of the sorted, compact JSON of the
/// `composer.json` keys that decide what a lock should contain. Duplicated
/// from `lock.rs`'s private `content_hash` (see the module doc).
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
    let encoded = php_json_encode(&Value::Object(
        relevant.into_iter().collect::<Map<String, Value>>(),
    ));
    Ok(format!("{:x}", md5::compute(encoded)))
}

/// PHP's `json_encode($value, 0)`: like `serde_json`'s compact encoding, but
/// `/` is escaped as `\/` and non-ASCII characters are escaped as `\uXXXX`.
/// Duplicated from `lock.rs` alongside `content_hash` (see the module doc).
fn php_json_encode(value: &Value) -> String {
    let mut out = String::new();
    write_php_json(value, &mut out);
    out
}

fn write_php_json(value: &Value, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::print_stderr)]
    fn plugin_api_version_matches_composer() {
        // `devbox run -- composer show --platform` prints the bundled
        // `composer-plugin-api`'s version; if a Composer upgrade in
        // devbox.json bumps it, this constant must move too.
        let Ok(output) = std::process::Command::new("composer")
            .args(["show", "--platform", "--no-ansi"])
            .current_dir(std::env::temp_dir())
            .output()
        else {
            eprintln!("skipping plugin_api_version_matches_composer: composer is not on PATH");
            return;
        };
        if !output.status.success() {
            eprintln!("skipping plugin_api_version_matches_composer: composer show failed");
            return;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| l.trim_start().starts_with("composer-plugin-api"))
            .unwrap_or_else(|| {
                panic!("composer-plugin-api not in `composer show --platform`: {stdout}")
            });
        assert!(
            line.contains(PLUGIN_API_VERSION),
            "PLUGIN_API_VERSION {PLUGIN_API_VERSION:?} does not match the installed Composer's \
             bundled plugin API: {line:?}"
        );
    }
}
