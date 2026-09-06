//! `vendor/composer/installed.json` and `installed.php`, ports of
//! Composer's `FilesystemRepository::write` and `ArrayDumper`.

use std::fmt::Write as _;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{Map, Value};

use crate::autoload::generator::find_shortest_path;
use crate::autoload::sort::natcmp;
use crate::lock::{Package, Root};
use crate::version;

/// `PlatformRepository::PLATFORM_PACKAGE_REGEX`: an exact match against the
/// whole package name, not a prefix — `php-http/client-implementation` looks
/// like it starts with `php` but doesn't match this.
static PLATFORM_PACKAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?:php(?:-64bit|-ipv6|-zts|-debug)?|hhvm|(?:ext|lib)-[a-z0-9](?:[_.-]?[a-z0-9]+)*|composer(?:-(?:plugin|runtime)-api)?)$",
    )
    .unwrap()
});

/// `preg_replace('{(\.9999999)+}', '.x', $alias)`: every run of one or more
/// consecutive `.9999999` segments collapses to a single `.x`.
static BRANCH_ALIAS_WILDCARD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:\.9999999)+").unwrap());

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
/// which have no directory. Composer's own packages (`vendor/composer/name`)
/// share `vendor/composer` as their parent with `installed.json`/`.php`
/// themselves, so `findShortestPath` gives `./name` there instead of the
/// `../name` every other vendor gets.
///
/// `from`/`to` are given a synthetic `/vivace-root` prefix (never a real
/// filesystem path) purely so the two are guaranteed to share a common
/// ancestor other than `vendor`: `package.install_dir` (`src/plugins.rs`'s
/// native adapters) can point anywhere under the project root, not just
/// under `vendor/`, and without a shared prefix `find_shortest_path`'s
/// common-ancestor walk would never terminate for a "to" that shares no path
/// segment with `vendor/composer` at all. The prefix has to be more than
/// one component deep — plain `/` — or `find_shortest_path`'s own
/// leave-it-absolute case for a *real* top-level split (`/foo` vs `/bar`,
/// distinct Windows drives or Docker mounts) would fire here too.
fn install_path(package: &Package) -> Option<String> {
    if package.r#type == "metapackage" {
        return None;
    }
    let to = if let Some(dir) = &package.install_dir {
        dir.clone()
    } else {
        let mut to = format!("vendor/{}", package.name);
        if let Some(dir) = &package.target_dir {
            to.push('/');
            to.push_str(dir);
        }
        to
    };
    Some(find_shortest_path(
        "/vivace-root/vendor/composer",
        &format!("/vivace-root/{to}"),
        true,
    ))
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
            // Composer's `LibraryInstaller`: `dist` for every package fetched
            // as an archive (zip/tar/path — a path repo's package still has
            // a `dist` block, just `type: path`), `source` for the dist-less
            // git-source packages `Package::validate_dist` accepts (#13).
            "installation-source" => Value::String(
                if package.dist.is_none() {
                    "source"
                } else {
                    "dist"
                }
                .into(),
            ),
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
        entry.insert(
            "aliases".into(),
            Value::Array(branch_alias(package).map_or_else(Vec::new, |a| vec![a.into()])),
        );
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

    // Virtual packages: replaces first, then provides, per package, root
    // included last like `FilesystemRepository::generateInstalledVersions`
    // (the root package is appended to its own package list).
    let links = packages
        .iter()
        .map(|p| (&p.replace, &p.provide, &p.version, p.dev))
        .chain([(&root.replace, &root.provide, &root_pretty, false)])
        .flat_map(|(replace, provide, version, dev)| {
            [
                (replace, "replaced", version, dev),
                (provide, "provided", version, dev),
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
                // `sort($x, SORT_NATURAL)` is case-sensitive strnatcmp.
                list.sort_by(|a, b| {
                    natcmp(
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

/// `ArrayLoader::getBranchAlias`: a `dev-*` package following a matching
/// `extra.branch-alias` entry, or `default-branch: true` with a non-numeric
/// branch name, is loaded as an `AliasPackage` whose pretty alias shows up
/// in `installed.php`'s `aliases`. Composer's `9999999-dev` stand-in for
/// "no explicit alias" is `VersionParser::DEFAULT_BRANCH_ALIAS`.
fn branch_alias(package: &Package) -> Option<String> {
    let version = &package.version;
    if !version.starts_with("dev-") && !version.ends_with("-dev") {
        return None;
    }
    if let Some(map) = package
        .raw
        .pointer("/extra/branch-alias")
        .and_then(Value::as_object)
    {
        for (source, target) in map {
            let Some(target) = target.as_str() else {
                continue;
            };
            if !target.ends_with("-dev") || !source.eq_ignore_ascii_case(version) {
                continue;
            }
            let validated = if target == "9999999-dev" {
                target.to_owned()
            } else {
                version::normalize_branch(&target[..target.len() - "-dev".len()])
            };
            if !validated.ends_with("-dev") {
                continue;
            }
            // `ArrayLoader::getBranchAlias`: when both the source key and the
            // target are numeric branches (`2.x-dev`, `2.0.x-dev`), the
            // target's numeric prefix must extend the source's, or the alias
            // is rejected — a `2.x-dev` package can alias `2.0.x-dev` but not
            // `3.0.x-dev`.
            if let (Some(source_prefix), Some(target_prefix)) = (
                parse_numeric_alias_prefix(source),
                parse_numeric_alias_prefix(target),
            ) && !target_prefix
                .to_ascii_lowercase()
                .starts_with(&source_prefix.to_ascii_lowercase())
            {
                continue;
            }
            return Some(collapse_branch_alias(&validated));
        }
    }
    if package.raw.get("default-branch") == Some(&Value::Bool(true)) && !is_numeric_branch(version)
    {
        return Some("9999999-dev".into());
    }
    None
}

/// `VersionParser::parseNumericAliasPrefix`: matches `1.x-dev`, `2.0.x-dev`,
/// `1.2.3-dev`, but not `dev-main`.
fn is_numeric_branch(version: &str) -> bool {
    let version = version.strip_prefix('v').unwrap_or(version);
    let Some(digits) = version.strip_suffix("-dev") else {
        return false;
    };
    let digits = digits.strip_suffix(".x").unwrap_or(digits);
    !digits.is_empty()
        && digits
            .split('.')
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// `VersionParser::parseNumericAliasPrefix`: `2.0.x-dev` and `2.0-dev` both
/// give `Some("2.0.")`; a non-numeric branch (`dev-main`) gives `None`.
static NUMERIC_ALIAS_PREFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?P<version>(?:[0-9]+\.)*[0-9]+)(?:\.x)?-dev$").unwrap());

fn parse_numeric_alias_prefix(branch: &str) -> Option<String> {
    NUMERIC_ALIAS_PREFIX
        .captures(branch)
        .map(|c| format!("{}.", &c["version"]))
}

/// `PlatformRepository::isPlatformPackage`.
fn is_platform_package(name: &str) -> bool {
    PLATFORM_PACKAGE.is_match(name)
}

fn collapse_branch_alias(alias: &str) -> String {
    BRANCH_ALIAS_WILDCARD.replace_all(alias, ".x").into_owned()
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
    use crate::lock::{Dist, Package, Root, TransportOptions, read_lock, read_root};

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
            source: None,
            transport_options: TransportOptions::default(),
            autoload: None,
            require: serde_json::Map::new(),
            provide: serde_json::Map::new(),
            replace: serde_json::Map::new(),
            r#type: "library".into(),
            target_dir: None,
            bin: vec![],
            include_path: vec![],
            dev: false,
            raw: json!({}),
            install_dir: None,
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
            "type": "library",
            "provide": {"foo/impl": "2.0"}
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
        // foo/impl: the golden's '1.4' comes from an alias package, which the
        // lock model has no equivalent of; '2.0' comes from the root's own
        // `provide` (wired above) and stays.
        assert_eq!(
            block(&out, "foo/impl"),
            block(&golden, "foo/impl")
                .replace("                1 => '1.4',\n", "")
                .replace("2 => '2.0'", "1 => '2.0'")
                .replace("3 => '^1.1'", "2 => '^1.1'")
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
    fn installed_json_uses_dot_slash_install_path_for_composer_vendor() {
        // vendor/composer/installers lives in the same directory as
        // installed.json itself (vendor/composer), so the shortest path is
        // `./installers`, not `../composer/installers`.
        let mut installers = package("composer/installers", "1.0", Some("abc"));
        installers.raw = json!({"name": "composer/installers", "version": "1.0"});
        let mut normal = package("psr/log", "1.0", Some("def"));
        normal.raw = json!({"name": "psr/log", "version": "1.0"});
        let out = installed_json(&[&installers, &normal], false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
        let packages = parsed["packages"].as_array().unwrap();
        let by_name = |name: &str| {
            packages
                .iter()
                .find(|p| p["name"] == name)
                .unwrap_or_else(|| panic!("{name} missing"))
        };
        assert_eq!(
            by_name("composer/installers")["install-path"],
            "./installers"
        );
        assert_eq!(by_name("psr/log")["install-path"], "../psr/log");
    }

    #[test]
    fn installed_php_uses_dot_slash_install_path_for_composer_vendor() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let installers = package("composer/installers", "1.0", Some("abc"));
        let normal = package("psr/log", "1.0", Some("def"));
        let packages = [&installers, &normal];
        let out = installed_php(&root, &packages, false).unwrap();
        assert!(out.contains("'install_path' => __DIR__ . '/./installers',"));
        assert!(out.contains("'install_path' => __DIR__ . '/../psr/log',"));
    }

    /// Port of Composer's `ArrayLoader::getBranchAlias`: a `dev-*` package
    /// whose `extra.branch-alias` maps its own branch to a `-dev` target
    /// gets that alias verbatim in `installed.php`.
    #[test]
    fn installed_php_aliases_dev_branch_from_branch_alias() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("acme/lib", "dev-main", Some("abc"));
        pkg.raw = json!({
            "name": "acme/lib",
            "version": "dev-main",
            "extra": {"branch-alias": {"dev-main": "3.x-dev"}},
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        assert!(out.contains("        'acme/lib' => array(\n"));
        let start = out.find("'acme/lib' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(
            block
                .contains("'aliases' => array(\n                0 => '3.x-dev',\n            ),\n"),
            "{block}"
        );
    }

    /// Several `branch-alias` keys: only the one matching the package's own
    /// version applies.
    #[test]
    fn installed_php_aliases_picks_the_matching_branch_alias_key() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("nesbot/carbon", "dev-master", Some("abc"));
        pkg.raw = json!({
            "name": "nesbot/carbon",
            "version": "dev-master",
            "extra": {"branch-alias": {"dev-2.x": "2.x-dev", "dev-master": "3.x-dev"}},
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out.find("'nesbot/carbon' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(
            block
                .contains("'aliases' => array(\n                0 => '3.x-dev',\n            ),\n"),
            "{block}"
        );
    }

    /// `ArrayLoader::getBranchAlias` validates the alias through
    /// `normalizeBranch(substr($alias, 0, -4))`; the pretty alias collapses
    /// every `.9999999` run in that normalised string back to `.x`, so a
    /// two-part branch like `1.8-dev` becomes `1.8.x-dev`, not `1.8-dev`.
    #[test]
    fn installed_php_alias_pretty_form_collapses_normalized_branch() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("acme/lib", "dev-master", Some("abc"));
        pkg.raw = json!({
            "name": "acme/lib",
            "version": "dev-master",
            "extra": {"branch-alias": {"dev-master": "1.8-dev"}},
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out.find("'acme/lib' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(
            block.contains(
                "'aliases' => array(\n                0 => '1.8.x-dev',\n            ),\n"
            ),
            "{block}"
        );
    }

    /// `ArrayLoader::getBranchAlias`'s numeric-alias-prefix cross-check: a
    /// numeric source branch (`2.0-dev`) aliasing a target whose numeric
    /// prefix doesn't extend it (`3.0.x-dev`) is rejected.
    #[test]
    fn installed_php_alias_rejected_when_numeric_prefixes_conflict() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("acme/lib", "2.0-dev", Some("abc"));
        pkg.raw = json!({
            "name": "acme/lib",
            "version": "2.0-dev",
            "extra": {"branch-alias": {"2.0-dev": "3.0.x-dev"}},
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out.find("'acme/lib' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(block.contains("'aliases' => array(),\n"), "{block}");
    }

    /// Same check, but the target's numeric prefix (`2.0.x-dev` -> `2.0.`)
    /// does extend the source's (`2.x-dev` -> `2.`), so the alias is
    /// accepted.
    #[test]
    fn installed_php_alias_accepted_when_numeric_prefixes_agree() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("acme/lib", "2.x-dev", Some("abc"));
        pkg.raw = json!({
            "name": "acme/lib",
            "version": "2.x-dev",
            "extra": {"branch-alias": {"2.x-dev": "2.0.x-dev"}},
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out.find("'acme/lib' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(
            block.contains(
                "'aliases' => array(\n                0 => '2.0.x-dev',\n            ),\n"
            ),
            "{block}"
        );
    }

    /// An `extra.branch-alias` target that isn't itself `-dev`-suffixed
    /// isn't a valid alias and is ignored.
    #[test]
    fn installed_php_alias_ignored_when_target_not_dev_suffixed() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("acme/lib", "dev-master", Some("abc"));
        pkg.raw = json!({
            "name": "acme/lib",
            "version": "dev-master",
            "extra": {"branch-alias": {"dev-master": "1.8"}},
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out.find("'acme/lib' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(block.contains("'aliases' => array(),\n"), "{block}");
    }

    /// `default-branch: true` with no matching `branch-alias` falls back to
    /// Composer's `DEFAULT_BRANCH_ALIAS`.
    #[test]
    fn installed_php_aliases_default_branch_without_branch_alias() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("acme/lib", "dev-main", Some("abc"));
        pkg.raw = json!({
            "name": "acme/lib",
            "version": "dev-main",
            "default-branch": true,
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out.find("'acme/lib' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(
            block.contains(
                "'aliases' => array(\n                0 => '9999999-dev',\n            ),\n"
            ),
            "{block}"
        );
    }

    /// A numeric branch (`1.x-dev`) is never eligible for
    /// `DEFAULT_BRANCH_ALIAS`, default-branch or not.
    #[test]
    fn installed_php_no_alias_for_numeric_default_branch() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("acme/lib", "1.x-dev", Some("abc"));
        pkg.raw = json!({
            "name": "acme/lib",
            "version": "1.x-dev",
            "default-branch": true,
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out.find("'acme/lib' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(block.contains("'aliases' => array(),\n"), "{block}");
    }

    /// Non-dev versions never get an alias.
    #[test]
    fn installed_php_no_alias_for_non_dev_version() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("acme/lib", "1.2.3", Some("abc"));
        pkg.raw = json!({
            "name": "acme/lib",
            "version": "1.2.3",
            "extra": {"branch-alias": {"dev-main": "3.x-dev"}},
        });
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out.find("'acme/lib' => array(\n").unwrap();
        let block = &out[start..start + 400];
        assert!(block.contains("'aliases' => array(),\n"), "{block}");
    }

    /// `PlatformRepository::isPlatformPackage` is an exact, case-insensitive
    /// regex match, not a name prefix: `php-http/client-implementation`
    /// looks like a platform package by prefix but isn't one, so it must
    /// stay a virtual `provided` entry, natural-sorted (`*` before `1.0`).
    #[test]
    fn installed_php_virtual_package_survives_php_prefix_false_positive() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut a = package("a/provider", "1.0", Some("abc"));
        a.provide = links(&[("php-http/client-implementation", "1.0")]);
        let mut b = package("b/provider", "1.0", Some("def"));
        b.provide = links(&[("php-http/client-implementation", "*")]);
        let packages = [&a, &b];
        let out = installed_php(&root, &packages, false).unwrap();
        let start = out
            .find("'php-http/client-implementation' => array(\n")
            .unwrap();
        let end = out[start..].find("\n        ),\n").unwrap() + start;
        assert_eq!(
            &out[start..end],
            "'php-http/client-implementation' => array(\n            'dev_requirement' => false,\n            'provided' => array(\n                0 => '*',\n                1 => '1.0',\n            ),"
        );
    }

    /// Real platform packages (`php`, `php-64bit`, `ext-json`, `lib-curl`,
    /// `composer-plugin-api`, `composer-runtime-api`) are skipped as
    /// `provided`/`replaced` virtual entries.
    #[test]
    fn installed_php_skips_real_platform_packages() {
        let root: Root = serde_json::from_value(json!({"name": "vendor/root"})).unwrap();
        let mut pkg = package("a/lib", "1.0", Some("abc"));
        pkg.provide = links(&[
            ("php", "*"),
            ("php-64bit", "*"),
            ("ext-json", "*"),
            ("lib-curl", "*"),
            ("composer-plugin-api", "*"),
            ("composer-runtime-api", "*"),
        ]);
        let packages = [&pkg];
        let out = installed_php(&root, &packages, false).unwrap();
        for name in [
            "php",
            "php-64bit",
            "ext-json",
            "lib-curl",
            "composer-plugin-api",
            "composer-runtime-api",
        ] {
            assert!(
                !out.contains(&format!("'{name}' => array(\n")),
                "{name} should be skipped"
            );
        }
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
