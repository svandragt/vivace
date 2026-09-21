//! Writes `viv.lock`, chapter 1's research lock format
//! (`docs/research.md`, #272): a second, TOML serialisation of the same
//! resolution `lock_writer::write` already turns into `composer.lock`. The
//! per-record fields and sort order are specified in `docs/research.md`'s
//! "Chapter 1" section; this module is the implementation of that spec, not
//! a second source of truth for it.
//!
//! Reading `viv.lock` back (`install`/`update` accepting it in place of
//! `composer.lock`) is a separate piece of work, not implemented here.

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;

use crate::solver::transaction::ResolvedPackage;

#[derive(Serialize)]
struct Document {
    package: Vec<Record>,
}

/// One `[[package]]` block. Deliberately excludes every field
/// `composer.lock` keeps at the file level (`content-hash`,
/// `plugin-api-version`, `platform`) and the `packages`/`packages-dev`
/// split (`dev` carries that instead): see `docs/research.md` chapter 1 for
/// why.
#[derive(Serialize)]
#[serde(rename_all = "kebab-case")]
struct Record {
    name: String,
    version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    dist_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    dist_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_ref: Option<String>,
    dev: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    root_requirement: Option<String>,
}

/// `viv.lock`'s body: one record per resolved package (`non_dev` and `dev`
/// folded into a single sorted list), no aggregate fields.
pub fn write(non_dev: &[ResolvedPackage], dev: &[ResolvedPackage], root: &Value) -> Result<String> {
    let root_requirements = root_requirements(root);
    let mut records: Vec<Record> = non_dev
        .iter()
        .map(|package| record(package, false, &root_requirements))
        .chain(
            dev.iter()
                .map(|package| record(package, true, &root_requirements)),
        )
        .collect();
    records.sort_by(|a, b| a.name.cmp(&b.name));
    toml::to_string_pretty(&Document { package: records }).context("serialising viv.lock")
}

/// One record: `dist`/`source` come off `ResolvedPackage::raw`, the same
/// provider-file JSON `lock_writer::dump_package` reads for
/// `composer.lock`, so this never re-fetches or re-derives anything the
/// resolution didn't already produce.
fn record(
    package: &ResolvedPackage,
    dev: bool,
    root_requirements: &HashMap<String, String>,
) -> Record {
    let dist = package.raw.get("dist");
    let source = package.raw.get("source");
    Record {
        name: package.name.clone(),
        version: package.pretty_version.clone(),
        dist_url: str_field(dist, "url"),
        dist_hash: str_field(dist, "shasum").filter(|s| !s.is_empty()),
        source_ref: str_field(source, "reference"),
        dev,
        root_requirement: root_requirements
            .get(&package.name.to_ascii_lowercase())
            .cloned(),
    }
}

fn str_field(object: Option<&Value>, key: &str) -> Option<String> {
    object
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The root `composer.json`'s `require`/`require-dev` names, lowercased,
/// mapped to their constraint string: the honest "root requirement that
/// selected it" for a record is exactly the one root composer.json already
/// states for that package's name, nothing inferred from the solve. A
/// package pulled in transitively (never named by root) has no root
/// requirement and the field is left off its record.
fn root_requirements(root: &Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for key in ["require", "require-dev"] {
        let Some(entries) = root.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (name, constraint) in entries {
            if let Some(constraint) = constraint.as_str() {
                map.insert(name.to_ascii_lowercase(), constraint.to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn package(name: &str, version: &str, raw: Value) -> ResolvedPackage {
        ResolvedPackage {
            name: name.to_string(),
            pretty_version: version.to_string(),
            raw,
        }
    }

    #[test]
    fn records_are_sorted_by_name_and_carry_the_root_requirement() {
        let root = json!({"require": {"acme/widget": "^2.0"}});
        let non_dev = vec![
            package(
                "acme/widget",
                "2.3.0",
                json!({
                    "dist": {"url": "https://example.test/widget.zip", "shasum": "abc"},
                    "source": {"reference": "deadbeef"},
                }),
            ),
            package("acme/aardvark", "1.0.0", json!({})),
        ];
        let got = write(&non_dev, &[], &root).unwrap();
        let parsed: toml::Table = toml::from_str(&got).unwrap();
        let packages = parsed["package"].as_array().unwrap();
        assert_eq!(packages[0]["name"].as_str(), Some("acme/aardvark"));
        assert_eq!(packages[1]["name"].as_str(), Some("acme/widget"));
        assert_eq!(packages[1]["root-requirement"].as_str(), Some("^2.0"));
        assert!(packages[0].get("root-requirement").is_none());
        assert_eq!(
            packages[1]["dist-url"].as_str(),
            Some("https://example.test/widget.zip")
        );
        assert_eq!(packages[1]["source-ref"].as_str(), Some("deadbeef"));
    }

    #[test]
    fn an_empty_dist_shasum_is_omitted() {
        let root = json!({});
        let non_dev = vec![package(
            "acme/widget",
            "1.0.0",
            json!({"dist": {"url": "https://example.test/widget.zip", "shasum": ""}}),
        )];
        let got = write(&non_dev, &[], &root).unwrap();
        assert!(!got.contains("dist-hash"));
    }

    #[test]
    fn dev_flag_matches_which_list_a_package_came_from() {
        let root = json!({});
        let non_dev = vec![package("acme/prod", "1.0.0", json!({}))];
        let dev = vec![package("acme/dev", "1.0.0", json!({}))];
        let got = write(&non_dev, &dev, &root).unwrap();
        let parsed: toml::Table = toml::from_str(&got).unwrap();
        let packages = parsed["package"].as_array().unwrap();
        let by_name = |name: &str| {
            packages
                .iter()
                .find(|p| p["name"].as_str() == Some(name))
                .unwrap()
        };
        assert_eq!(by_name("acme/prod")["dev"].as_bool(), Some(false));
        assert_eq!(by_name("acme/dev")["dev"].as_bool(), Some(true));
    }
}
