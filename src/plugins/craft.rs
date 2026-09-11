//! `craftcms/plugin-installer`'s `Installer::addPlugin`/`savePlugins`: writes
//! `vendor/craftcms/plugins.php`, a `var_export` dump of every installed
//! `craft-plugin` package keyed by package name.
//!
//! Ported from `craftcms/plugin-installer` `1.6.0`'s `src/Installer.php`,
//! fetched 2026-09-08 — see `yii2.rs`'s doc comment for why this hooks
//! `PRE_AUTOLOAD_DUMP` rather than the real plugin's per-package
//! `Installer::install`/`update`, and for the install-order DFS's own
//! ceiling against two independent plugins.
//!
//! `Plugin::activate`'s `isRoot` branch (the root package
//! installing itself as a `craft-plugin`) isn't ported — viv never installs
//! itself as a Craft plugin. A real `craft-plugin` package's legacy,
//! non-lowercase `extra.handle` (the plugin's own `camel2id` renormalise,
//! with a warning) also isn't ported — every `craft-plugin` on Packagist
//! already ships a lowercase handle.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::{Map, Value, json};
use std::sync::LazyLock;

use super::phpstan::var_export;
use super::yii2::{normalize_path, resolve_autoload_path, tag_path};
use super::{Adapter, Ctx};
use crate::lock::Package;

const PACKAGE_TYPE: &str = "craft-plugin";

static HANDLE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^_?[a-zA-Z][\w\-]*$").expect("valid regex"));

pub(super) struct Craft;

impl Adapter for Craft {
    fn plugin_names(&self) -> &'static [&'static str] {
        &["craftcms/plugin-installer"]
    }

    fn upstream_version(&self) -> &'static str {
        "1.6.0"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/plugins/craft"
    }

    fn pre_autoload_dump(&self, ctx: &Ctx<'_>, packages: &[(&Package, PathBuf)]) -> Result<()> {
        apply(ctx.project_dir, ctx.vendor_dir, packages)
    }
}

fn apply(project_dir: &Path, vendor_dir: &Path, packages: &[(&Package, PathBuf)]) -> Result<()> {
    let mut plugins = Map::new();
    for (package, install_dir) in super::in_install_order(packages) {
        if package.r#type != PACKAGE_TYPE {
            continue;
        }
        plugins.insert(
            package.name.clone(),
            add_plugin(vendor_dir, install_dir, package)?,
        );
    }

    let path = vendor_dir.join("craftcms/plugins.php");
    if plugins.is_empty() {
        // `savePlugins` is only ever reached from `registerPlugin`, itself
        // only called by `addPlugin`: no `craft-plugin` package means this
        // file is never created in the first place.
        let _ = fs_err::remove_file(&path);
        return Ok(());
    }

    fs_err::create_dir_all(vendor_dir.join("craftcms"))?;
    fs_err::write(&path, render(project_dir, vendor_dir, &plugins))?;
    Ok(())
}

/// `addPlugin`: `class`/`basePath` (required, `extra` first, else
/// autodetected from `psr-4`), `handle` (required, validated), then every
/// optional field in `Installer::addPlugin`'s own fixed order.
fn add_plugin(vendor_dir: &Path, install_dir: &Path, package: &Package) -> Result<Value> {
    let extra = package
        .raw
        .pointer("/extra")
        .cloned()
        .unwrap_or(Value::Null);
    let get = |key: &str| extra.get(key);
    let str_field = |key: &str| get(key).and_then(Value::as_str).map(str::to_owned);

    let mut class = str_field("class");
    let mut base_path = str_field("basePath");
    let aliases =
        generate_default_aliases(vendor_dir, install_dir, package, &mut class, &mut base_path);

    let Some(class) = class else {
        bail!("{}: unable to determine the Plugin class", package.name);
    };
    let Some(base_path) = base_path else {
        bail!("{}: unable to determine the base path", package.name);
    };
    let handle = get("handle")
        .and_then(Value::as_str)
        .filter(|h| HANDLE.is_match(h));
    let Some(handle) = handle else {
        bail!("{}: invalid or missing plugin handle", package.name);
    };

    let mut plugin = Map::new();
    plugin.insert("class".to_string(), json!(class));
    plugin.insert("basePath".to_string(), json!(base_path));
    plugin.insert("handle".to_string(), json!(handle));
    if !aliases.is_empty() {
        plugin.insert("aliases".to_string(), Value::Object(aliases));
    }

    let (vendor, name) = package
        .name
        .split_once('/')
        .map_or((None, package.name.as_str()), |(v, n)| (Some(v), n));
    plugin.insert(
        "name".to_string(),
        str_field("name").map_or_else(|| json!(name), |v| json!(v)),
    );
    plugin.insert(
        "version".to_string(),
        str_field("version").map_or_else(|| json!(package.version), |v| json!(v)),
    );
    if let Some(v) = get("schemaVersion") {
        plugin.insert("schemaVersion".to_string(), v.clone());
    }
    let description = str_field("description").or_else(|| {
        package
            .raw
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    if let Some(v) = description {
        plugin.insert("description".to_string(), json!(v));
    }
    let developer = str_field("developer")
        .or_else(|| author_property(package, "name"))
        .or_else(|| vendor.map(str::to_owned));
    if let Some(v) = developer {
        plugin.insert("developer".to_string(), json!(v));
    }
    let developer_url = str_field("developerUrl")
        .or_else(|| {
            package
                .raw
                .get("homepage")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .or_else(|| author_property(package, "homepage"));
    if let Some(v) = developer_url {
        plugin.insert("developerUrl".to_string(), json!(v));
    }
    let developer_email = str_field("developerEmail").or_else(|| {
        package
            .raw
            .pointer("/support/email")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    if let Some(v) = developer_email {
        plugin.insert("developerEmail".to_string(), json!(v));
    }
    let documentation_url = str_field("documentationUrl").or_else(|| {
        package
            .raw
            .pointer("/support/docs")
            .and_then(Value::as_str)
            .map(str::to_owned)
    });
    if let Some(v) = documentation_url {
        plugin.insert("documentationUrl".to_string(), json!(v));
    }
    if let Some(v) = get("changelogUrl") {
        plugin.insert("changelogUrl".to_string(), v.clone());
    }
    if let Some(v) = get("downloadUrl") {
        plugin.insert("downloadUrl".to_string(), v.clone());
    }
    if let Some(v) = get("t9nCategory") {
        plugin.insert("t9nCategory".to_string(), v.clone());
    }
    if let Some(v) = get("sourceLanguage") {
        plugin.insert("sourceLanguage".to_string(), v.clone());
    }
    if let Some(v) = get("hasCpSettings") {
        plugin.insert(
            "hasCpSettings".to_string(),
            json!(v.as_bool().unwrap_or_default()),
        );
    }
    if let Some(v) = get("hasCpSection") {
        plugin.insert(
            "hasCpSection".to_string(),
            json!(v.as_bool().unwrap_or_default()),
        );
    }
    if let Some(v) = get("components") {
        plugin.insert("components".to_string(), v.clone());
    }
    if let Some(v) = get("modules") {
        plugin.insert("modules".to_string(), v.clone());
    }
    if let Some(v) = get("minVersionRequired") {
        plugin.insert("minVersionRequired".to_string(), v.clone());
    }

    Ok(Value::Object(plugin))
}

/// `getAuthorProperty`: the first author's `$property`, or `None` without
/// one.
fn author_property(package: &Package, property: &str) -> Option<String> {
    package
        .raw
        .pointer("/authors/0")
        .and_then(|a| a.get(property))
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// `generateDefaultAliases`: one `@namespace` alias per `psr-4` autoload
/// entry (a multi-path entry is skipped, same as `yii2`'s), tagged
/// `<vendor-dir>` when the resolved path falls under it (`<root-dir>` never
/// applies here — that only fires for the root package's own `isRoot`
/// aliases, not ported). Also fills `class`/`base_path` in place from the
/// first namespace whose directory holds a `Plugin.php`/the class file,
/// exactly where the real plugin still needs the package's files physically
/// on disk to check — true for every real install, and for a fixture that
/// stages them the same way.
fn generate_default_aliases(
    vendor_dir: &Path,
    install_dir: &Path,
    package: &Package,
    class: &mut Option<String>,
    base_path: &mut Option<String>,
) -> Map<String, Value> {
    let mut aliases = Map::new();
    let Some(psr4) = package
        .autoload
        .as_ref()
        .and_then(|a| a.get("psr-4"))
        .and_then(Value::as_object)
    else {
        return aliases;
    };
    let vendor_dir_str = normalize_path(&vendor_dir.to_string_lossy());

    for (namespace, path) in psr4 {
        let Some(path) = path.as_str() else { continue };
        let absolute = normalize_path(&resolve_autoload_path(install_dir, path));
        let alias = format!("@{}", namespace.trim_matches('\\').replace('\\', "/"));
        aliases.insert(alias, json!(tag_path(&absolute, &vendor_dir_str)));

        if class.is_none() && Path::new(&absolute).join("Plugin.php").is_file() {
            *class = Some(format!("{namespace}Plugin"));
        }
        if base_path.is_none()
            && let Some(class) = class
            && let Some(rest) = class.strip_prefix(namespace.as_str())
        {
            let test_class_path =
                Path::new(&absolute).join(format!("{}.php", rest.replace('\\', "/")));
            if test_class_path.is_file() {
                let parent = test_class_path.parent().unwrap_or(Path::new(&absolute));
                *base_path = Some(tag_path(
                    &normalize_path(&parent.to_string_lossy()),
                    &vendor_dir_str,
                ));
            }
        }
    }
    aliases
}

/// `savePlugins`: `var_export`, then `'<vendor-dir>`/`'<root-dir>` swapped
/// for `$vendorDir . '`/`$rootDir . '` — safe for the same reason `yii2`'s
/// `render` is.
fn render(project_dir: &Path, vendor_dir: &Path, plugins: &Map<String, Value>) -> String {
    let array = var_export(&Value::Object(plugins.clone()), 0)
        .replace("'<vendor-dir>", "$vendorDir . '")
        .replace("'<root-dir>", "$rootDir . '");
    format!(
        "<?php\n\n$vendorDir = dirname(__DIR__);\n$rootDir = {};\n\nreturn {array};\n",
        root_dir_code(&vendor_dir.join("craftcms"), project_dir)
    )
}

/// `Filesystem::findShortestPathCode($vendorDir . '/craftcms', getcwd(), true)`,
/// specialised to `$directories = true`, `$staticCode = false`,
/// `$preferRelative = false` — craft's only call shape: PHP code that,
/// evaluated from a file in `from`, returns `to`. A chain of
/// `dirname(__DIR__)` calls when `to` is an ancestor of `from` (always true
/// for the default `vendor/` layout: two levels down from the project
/// root), a quoted literal otherwise.
fn root_dir_code(from: &Path, to: &Path) -> String {
    let from = normalize_path(&from.to_string_lossy());
    let to = normalize_path(&to.to_string_lossy());
    if from == to {
        return "__DIR__".to_string();
    }

    let mut common = to.clone();
    while !format!("{from}/").starts_with(&format!("{common}/")) && common != "/" && common != "." {
        common = dirname(&common);
    }
    if !from.starts_with(&common) || common == "." {
        return super::php_string(&to);
    }

    let common = format!("{}/", common.trim_end_matches('/'));
    if to.starts_with(&format!("{from}/")) {
        let rel = to.get(from.len()..).unwrap_or("");
        return format!("__DIR__ . {}", super::php_string(rel));
    }
    // `+1` for `$directories = true`: `from` is a directory, but the
    // generated code lives one level deeper, in a file inside it.
    let source_depth = from.get(common.len()..).unwrap_or("").matches('/').count() + 1;
    if common == "/" && source_depth > 1 {
        return super::php_string(&to);
    }
    let code = format!(
        "{}__DIR__{}",
        "dirname(".repeat(source_depth),
        ")".repeat(source_depth)
    );
    let rel_target = to.get(common.len()..).unwrap_or("");
    if rel_target.is_empty() {
        code
    } else {
        format!("{code} . {}", super::php_string(&format!("/{rel_target}")))
    }
}

/// PHP's `dirname` for an already-normalised absolute POSIX path (no
/// trailing separator except the root itself).
fn dirname(path: &str) -> String {
    if path == "/" {
        return "/".to_string();
    }
    match path.rfind('/') {
        Some(0) => "/".to_string(),
        Some(i) => path[..i].to_string(),
        None => ".".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::lock::read_lock;

    fn package(name: &str, raw_extra: &Value) -> Package {
        let mut raw = json!({
            "name": name,
            "version": "1.0.0",
            "type": "craft-plugin",
        });
        raw.as_object_mut()
            .unwrap()
            .extend(raw_extra.as_object().unwrap().clone());
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    #[test]
    fn root_dir_code_two_levels_down_from_project_root() {
        let from = Path::new("/project/vendor/craftcms");
        let to = Path::new("/project");
        assert_eq!(root_dir_code(from, to), "dirname(dirname(__DIR__))");
    }

    #[test]
    fn root_dir_code_falls_back_to_a_literal_without_a_common_ancestor() {
        let from = Path::new("/vendor/craftcms");
        let to = Path::new("/elsewhere");
        assert_eq!(root_dir_code(from, to), "'/elsewhere'");
    }

    #[test]
    fn add_plugin_requires_a_handle() {
        let package = package(
            "acme/widget",
            &json!({"extra": {"class": "Acme\\Plugin", "basePath": "./src"}}),
        );
        let vendor_dir = Path::new("/project/vendor");
        let err = add_plugin(vendor_dir, &vendor_dir.join("acme/widget"), &package).unwrap_err();
        assert!(err.to_string().contains("handle"), "{err}");
    }

    #[test]
    fn add_plugin_falls_back_to_the_vendor_as_developer() {
        let package = package(
            "acme/widget",
            &json!({"extra": {"class": "Acme\\Plugin", "basePath": "./src", "handle": "widget"}}),
        );
        let vendor_dir = Path::new("/project/vendor");
        let plugin = add_plugin(vendor_dir, &vendor_dir.join("acme/widget"), &package).unwrap();
        assert_eq!(plugin.get("developer").unwrap(), "acme");
        assert_eq!(plugin.get("name").unwrap(), "widget");
    }

    #[test]
    fn generate_default_aliases_detects_class_and_base_path_from_disk() {
        let dir = tempfile::tempdir().unwrap();
        let vendor_dir = dir.path().join("vendor");
        let install_dir = vendor_dir.join("acme/widget");
        fs_err::create_dir_all(install_dir.join("src")).unwrap();
        fs_err::write(install_dir.join("src/Plugin.php"), "<?php").unwrap();
        let package = package(
            "acme/widget",
            &json!({"autoload": {"psr-4": {"Acme\\Widget\\": "src/"}}}),
        );

        let mut class = None;
        let mut base_path = None;
        let aliases = generate_default_aliases(
            &vendor_dir,
            &install_dir,
            &package,
            &mut class,
            &mut base_path,
        );

        assert_eq!(class.as_deref(), Some("Acme\\Widget\\Plugin"));
        assert_eq!(base_path.as_deref(), Some("<vendor-dir>/acme/widget/src"));
        assert_eq!(
            aliases.get("@Acme/Widget").unwrap(),
            "<vendor-dir>/acme/widget/src"
        );
    }

    /// #92's fixture, byte-diffed against real `craftcms/plugin-installer`
    /// 1.6.0's own output (`tests/fixtures/plugins/craft/expected`).
    #[test]
    fn craft_fixture_matches_composers_golden() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/craft");
        let lock = read_lock(&dir.join("composer.lock")).unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let packages: Vec<(&Package, PathBuf)> = lock
            .packages(true)
            .map(|p| (p, vendor_dir.join(&p.name)))
            .collect();

        apply(project_dir.path(), &vendor_dir, &packages).unwrap();

        let got = fs_err::read_to_string(vendor_dir.join("craftcms/plugins.php")).unwrap();
        let want = fs_err::read_to_string(dir.join("expected/plugins.php")).unwrap();
        assert_eq!(got, want);
    }
}
