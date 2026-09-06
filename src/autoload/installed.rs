//! `vendor/composer/installed.json` and `installed.php`, ports of
//! Composer's `FilesystemRepository::write` and `ArrayDumper`.

use std::fmt::Write as _;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::autoload::sort::natcasecmp;
use crate::lock::{Package, Root};
use crate::version;

/// `ArrayDumper::dump` key order. Anything the lock carries that is not in
/// this list (`version_normalized` from an older lock, say) is dropped.
const KEY_ORDER: &[&str] = &[
    "name",
    "version",
    "version_normalized",
    "target-dir",
    "source",
    "dist",
    "require",
    "conflict",
    "provide",
    "replace",
    "require-dev",
    "suggest",
    "time",
    "default-branch",
    "bin",
    "type",
    "extra",
    "installation-source",
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
    "install-path",
];

const KSORTED: &[&str] = &[
    "require",
    "conflict",
    "provide",
    "replace",
    "require-dev",
    "suggest",
];

/// Composer's `1.0.0+no-version-set` for a root `composer.json` without
/// `version`.
const NO_VERSION_SET: &str = "1.0.0+no-version-set";

/// `install-path` relative to `vendor/composer`; `None` for metapackages,
/// which have no directory.
fn install_path(package: &Package) -> Option<String> {
    if package.r#type == "metapackage" {
        return None;
    }
    let mut path = format!("../{}", package.name);
    if let Some(dir) = &package.target_dir {
        path.push('/');
        path.push_str(dir);
    }
    Some(path)
}

/// The `installed.json` body: lock entries in `ArrayDumper` key order plus
/// the fields Composer adds at install time.
pub fn installed_json(packages: &[&Package], dev: bool) -> Result<String> {
    let mut dumped = packages
        .iter()
        .map(|package| dump_package(package))
        .collect::<Result<Vec<_>>>()?;
    dumped.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["name"].as_str().unwrap_or_default())
    });

    let mut dev_names: Vec<&str> = packages
        .iter()
        .filter(|p| p.dev)
        .map(|p| p.name.as_str())
        .collect();
    dev_names.sort_unstable();

    let mut data = Map::new();
    data.insert("packages".into(), Value::Array(dumped));
    data.insert("dev".into(), Value::Bool(dev));
    data.insert("dev-package-names".into(), dev_names.into());
    // PHP's JSON_PRETTY_PRINT indents by four spaces; serde's default is two.
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&Value::Object(data), &mut serializer)?;
    buf.push(b'\n');
    Ok(String::from_utf8(buf)?)
}

fn dump_package(package: &Package) -> Result<Value> {
    let raw = package.raw.as_object().cloned().unwrap_or_default();
    let mut out = Map::new();
    for &key in KEY_ORDER {
        let value = match key {
            "version_normalized" => Value::String(
                version::normalize(&package.version)
                    .with_context(|| format!("{}: version", package.name))?,
            ),
            "installation-source" => Value::String("dist".into()),
            // `getType()` defaults to `library`, so the key is always present.
            "type" => Value::String(package.r#type.clone()),
            "install-path" => install_path(package).map_or(Value::Null, Value::String),
            _ => match raw.get(key) {
                // `ArrayDumper::dumpValues` drops nulls and empty arrays.
                None | Some(Value::Null) => continue,
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
        out.insert(key.into(), value);
    }
    Ok(Value::Object(out))
}

/// Root `install_path` from `vendor/composer` back to the project dir.
fn root_install_path(root: &Root) -> String {
    let depth = root.config.vendor_dir.split('/').count() + 1;
    "../".repeat(depth)
}

fn reference(package: &Package) -> Value {
    package
        .dist
        .as_ref()
        .and_then(|d| d.reference.clone())
        .or_else(|| {
            package
                .raw
                .pointer("/source/reference")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .map_or(Value::Null, Value::String)
}

/// `installed.php`, `FilesystemRepository::generateInstalledVersions` shape.
pub fn installed_php(root: &Root, packages: &[&Package], dev: bool) -> Result<String> {
    let root_name = root.name.clone().unwrap_or_else(|| "__root__".into());
    let root_pretty = root
        .version
        .clone()
        .unwrap_or_else(|| NO_VERSION_SET.into());
    let root_version = version::normalize(&root_pretty).context("root version")?;
    let root_path = root_install_path(root);

    let mut versions: Map<String, Value> = Map::new();
    for package in packages {
        let mut entry = Map::new();
        entry.insert("pretty_version".into(), package.version.clone().into());
        entry.insert(
            "version".into(),
            version::normalize(&package.version)
                .with_context(|| format!("{}: version", package.name))?
                .into(),
        );
        entry.insert("reference".into(), reference(package));
        entry.insert("type".into(), package.r#type.clone().into());
        entry.insert(
            "install_path".into(),
            install_path(package).map_or(Value::Null, Value::String),
        );
        entry.insert("aliases".into(), Value::Array(vec![]));
        entry.insert("dev_requirement".into(), package.dev.into());
        versions.insert(package.name.clone(), Value::Object(entry));
    }
    let mut root_entry = Map::new();
    root_entry.insert("pretty_version".into(), root_pretty.clone().into());
    root_entry.insert("version".into(), root_version.clone().into());
    root_entry.insert("reference".into(), Value::Null);
    root_entry.insert("type".into(), root.r#type.clone().into());
    root_entry.insert("install_path".into(), root_path.clone().into());
    root_entry.insert("aliases".into(), Value::Array(vec![]));
    root_entry.insert("dev_requirement".into(), false.into());
    versions.insert(root_name.clone(), Value::Object(root_entry));

    // Virtual packages: replaces first, then provides, per package.
    // ponytail: root-level `replace`/`provide` are not in `lock::Root` yet;
    // add them there when a fixture needs them.
    let links = packages.iter().flat_map(|p| {
        [
            (&p.replace, "replaced", &p.version, p.dev),
            (&p.provide, "provided", &p.version, p.dev),
        ]
    });
    for (map, list_key, pretty_version, is_dev) in links {
        for (target, constraint) in map {
            if is_platform_package(target) {
                continue;
            }
            let entry = versions
                .entry(target.clone())
                .or_insert_with(|| Value::Object(Map::new()));
            let entry = entry.as_object_mut().expect("entries are objects");
            match entry.get_mut("dev_requirement") {
                None => {
                    entry.insert("dev_requirement".into(), is_dev.into());
                }
                Some(flag) if !is_dev => *flag = false.into(),
                Some(_) => {}
            }
            let constraint = match constraint.as_str().unwrap_or_default() {
                "self.version" => pretty_version.clone(),
                other => other.to_owned(),
            };
            let list = entry
                .entry(list_key)
                .or_insert_with(|| Value::Array(vec![]))
                .as_array_mut()
                .expect("lists are arrays");
            if !list.iter().any(|v| v.as_str() == Some(&constraint)) {
                list.push(constraint.into());
            }
        }
    }

    let mut sorted: Vec<(String, Value)> = versions.into_iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, entry) in &mut sorted {
        for key in ["aliases", "replaced", "provided"] {
            if let Some(list) = entry.get_mut(key).and_then(Value::as_array_mut) {
                // ponytail: SORT_NATURAL is case-sensitive strnatcmp; the
                // constraints here are versions, where case never differs.
                list.sort_by(|a, b| {
                    natcasecmp(
                        a.as_str().unwrap_or_default(),
                        b.as_str().unwrap_or_default(),
                    )
                });
            }
        }
    }

    let mut root_block = Map::new();
    root_block.insert("name".into(), root_name.into());
    root_block.insert("pretty_version".into(), root_pretty.into());
    root_block.insert("version".into(), root_version.into());
    root_block.insert("reference".into(), Value::Null);
    root_block.insert("type".into(), root.r#type.clone().into());
    root_block.insert("install_path".into(), root_path.into());
    root_block.insert("aliases".into(), Value::Array(vec![]));
    root_block.insert("dev".into(), dev.into());

    let mut top = Map::new();
    top.insert("root".into(), Value::Object(root_block));
    top.insert(
        "versions".into(),
        Value::Object(sorted.into_iter().collect()),
    );

    Ok(format!("<?php return {};\n", dump_to_php_code(&top, 0)))
}

/// `PlatformRepository::isPlatformPackage`.
fn is_platform_package(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.starts_with("php")
        || lower.starts_with("ext-")
        || lower.starts_with("lib-")
        || lower.starts_with("composer")
        || lower == "hhvm"
}

/// `FilesystemRepository::dumpToPhpCode`.
fn dump_to_php_code(array: &Map<String, Value>, level: usize) -> String {
    let mut lines = String::from("array(\n");
    let level = level + 1;
    let indent = "    ".repeat(level);
    for (key, value) in array {
        lines.push_str(&indent);
        lines.push_str(&var_export_str(key));
        lines.push_str(" => ");
        dump_value(&mut lines, key, value, level);
    }
    lines.push_str(&"    ".repeat(level - 1));
    lines.push(')');
    if level - 1 != 0 {
        lines.push_str(",\n");
    }
    lines
}

fn dump_value(lines: &mut String, key: &str, value: &Value, level: usize) {
    match value {
        Value::Object(map) if map.is_empty() => lines.push_str("array(),\n"),
        Value::Object(map) => lines.push_str(&dump_to_php_code(map, level)),
        Value::Array(items) if items.is_empty() => lines.push_str("array(),\n"),
        Value::Array(items) => {
            // PHP lists are arrays with integer keys.
            lines.push_str("array(\n");
            let indent = "    ".repeat(level + 1);
            for (i, item) in items.iter().enumerate() {
                lines.push_str(&indent);
                write!(lines, "{i} => ").expect("String writes cannot fail");
                dump_value(lines, "", item, level + 1);
            }
            lines.push_str(&"    ".repeat(level));
            lines.push_str("),\n");
        }
        Value::String(s) if key == "install_path" && !s.starts_with('/') => {
            lines.push_str("__DIR__ . ");
            lines.push_str(&var_export_str(&format!("/{s}")));
            lines.push_str(",\n");
        }
        Value::String(s) => {
            lines.push_str(&var_export_str(s));
            lines.push_str(",\n");
        }
        Value::Bool(b) => lines.push_str(if *b { "true,\n" } else { "false,\n" }),
        Value::Null => lines.push_str("null,\n"),
        Value::Number(n) => writeln!(lines, "{n},").expect("String writes cannot fail"),
    }
}

/// PHP `var_export` of a string: only `\`, `'` and NUL are escaped; NUL
/// becomes a double-quoted `"\0"` concatenation.
fn var_export_str(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\0', "' . \"\\0\" . '");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{installed_json, installed_php};
    use crate::lock::{Dist, Package, Root, read_lock, read_root};

    fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/monolog")
            .join(name)
    }

    fn expected(mode: &str, file: &str) -> String {
        fs_err::read_to_string(fixture(&format!("expected/{mode}/composer/{file}"))).unwrap()
    }

    fn monolog(dev: bool) -> (Root, Vec<Package>) {
        let root = read_root(&fixture("composer.json")).unwrap();
        let lock = read_lock(&fixture("composer.lock")).unwrap();
        let packages = lock.packages(dev).cloned().collect();
        (root, packages)
    }

    #[test]
    fn installed_json_matches_monolog_dev() {
        let (_, packages) = monolog(true);
        let refs: Vec<&Package> = packages.iter().collect();
        assert_eq!(
            installed_json(&refs, true).unwrap(),
            expected("dev", "installed.json")
        );
    }

    #[test]
    fn installed_json_matches_monolog_no_dev() {
        let (_, packages) = monolog(false);
        let refs: Vec<&Package> = packages.iter().collect();
        assert_eq!(
            installed_json(&refs, false).unwrap(),
            expected("no-dev", "installed.json")
        );
    }

    #[test]
    fn installed_php_matches_monolog_dev() {
        let (root, packages) = monolog(true);
        let refs: Vec<&Package> = packages.iter().collect();
        assert_eq!(
            installed_php(&root, &refs, true).unwrap(),
            expected("dev", "installed.php")
        );
    }

    #[test]
    fn installed_php_matches_monolog_no_dev() {
        let (root, packages) = monolog(false);
        let refs: Vec<&Package> = packages.iter().collect();
        assert_eq!(
            installed_php(&root, &refs, false).unwrap(),
            expected("no-dev", "installed.php")
        );
    }

    fn package(name: &str, version: &str, dist_ref: Option<&str>) -> Package {
        Package {
            name: name.into(),
            version: version.into(),
            dist: dist_ref.map(|r| Dist {
                r#type: "zip".into(),
                url: String::new(),
                reference: Some(r.into()),
                shasum: None,
            }),
            autoload: None,
            require: serde_json::Map::new(),
            provide: serde_json::Map::new(),
            replace: serde_json::Map::new(),
            r#type: "library".into(),
            target_dir: None,
            bin: vec![],
            dev: false,
            raw: json!({}),
        }
    }

    fn links(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), json!(v)))
            .collect()
    }

    /// Port of Composer's `FilesystemRepositoryTest::testRepositoryWritesInstalledPhp`
    /// against `tests/fixtures/composer/repository/installed.php`. The lock
    /// model has no aliases, no root source reference and fixed install
    /// paths, so the comparison is per block rather than whole file.
    #[test]
    fn installed_php_ports_composer_repository_test() {
        let golden = fs_err::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/composer/repository/installed.php"),
        )
        .unwrap();

        let root: Root = serde_json::from_value(json!({
            "name": "__root__",
            "version": "dev-master",
            "type": "library"
        }))
        .unwrap();

        let mut provider = package("a/provider", "1.1", Some("distref-as-no-source"));
        provider.provide = links(&[("foo/impl", "^1.1"), ("foo/impl2", "2.0")]);
        let mut provider2 = package("a/provider2", "1.2", Some("distref-as-installed-from-dist"));
        provider2.provide = links(&[("foo/impl", "self.version"), ("foo/impl2", "2.0")]);
        let mut replacer = package("b/replacer", "2.2", None);
        replacer.replace = links(&[("foo/impl2", "self.version"), ("foo/replaced", "^3.0")]);
        let mut c = package(
            "c/c",
            "3.0",
            Some("{${passthru('bash -i')}} Foo\\Bar\n\ttab\u{b}verticaltab\0"),
        );
        c.dev = true;
        let mut meta = package("meta/package", "3.0", None);
        meta.r#type = "metapackage".into();

        let packages = [&provider, &provider2, &replacer, &c, &meta];
        let out = installed_php(&root, &packages, true).unwrap();

        let block = |text: &str, name: &str| -> String {
            let start = text.find(&format!("        '{name}' => array(\n")).unwrap();
            let end = text[start..].find("\n        ),\n").unwrap() + start;
            text[start..end].to_owned()
        };
        for name in ["foo/impl2", "foo/replaced", "meta/package"] {
            assert_eq!(block(&out, name), block(&golden, name), "{name}");
        }
        // foo/impl: the golden's '1.4' comes from an alias package and '2.0'
        // from the root's own `provide`, neither of which the lock model has.
        assert_eq!(
            block(&out, "foo/impl"),
            block(&golden, "foo/impl")
                .replace("                1 => '1.4',\n", "")
                .replace("                2 => '2.0',\n", "")
                .replace("3 => '^1.1'", "1 => '^1.1'")
        );
        // c/c: escaping of the reference, dev_requirement; install path differs.
        let c_golden = block(&golden, "c/c").replace(
            "'install_path' => '/foo/bar/ven/do{}r/c/c${}',",
            "'install_path' => __DIR__ . '/../c/c',",
        );
        assert_eq!(block(&out, "c/c"), c_golden);
        // b/replacer has neither dist nor source: reference null.
        assert!(block(&out, "b/replacer").contains("'reference' => null,"));
        assert!(out.starts_with("<?php return array(\n    'root' => array(\n        'name' => '__root__',\n        'pretty_version' => 'dev-master',\n        'version' => 'dev-master',\n"));
    }

    #[test]
    fn installed_json_uses_target_dir_and_null_for_metapackages() {
        let mut lib = package("a/lib", "1.0", Some("abc"));
        lib.target_dir = Some("Acme/Lib".into());
        lib.raw = json!({"name": "a/lib", "version": "1.0", "target-dir": "Acme/Lib", "keywords": ["b", "a"], "require": {"z/z": "*", "a/a": "*"}, "extra": []});
        let mut meta = package("m/m", "2.0", None);
        meta.r#type = "metapackage".into();
        meta.raw = json!({"name": "m/m", "version": "2.0", "type": "metapackage", "version_normalized": "junk"});

        let out = installed_json(&[&meta, &lib], false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let lib_out = &parsed["packages"][0];
        assert_eq!(lib_out["install-path"], "../a/lib/Acme/Lib");
        assert_eq!(lib_out["keywords"], json!(["a", "b"]));
        let keys: Vec<&str> = lib_out
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "name",
                "version",
                "version_normalized",
                "target-dir",
                "require",
                "type",
                "installation-source",
                "keywords",
                "install-path"
            ]
        );
        let req_keys: Vec<&str> = lib_out["require"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(req_keys, ["a/a", "z/z"]);
        let meta_out = &parsed["packages"][1];
        assert_eq!(meta_out["install-path"], serde_json::Value::Null);
        assert_eq!(meta_out["version_normalized"], "2.0.0.0");
        assert!(out.ends_with("}\n"));
    }
}
