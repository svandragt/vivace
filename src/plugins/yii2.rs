//! `yiisoft/yii2-composer`'s `Installer::addPackage`/`saveExtensions`: writes
//! `vendor/yiisoft/extensions.php`, a `var_export` dump of every installed
//! `yii2-extension` package keyed by package name.
//!
//! Ported from `yiisoft/yii2-composer` `2.0.11`'s `src/Installer.php`,
//! fetched 2026-09-08.
//!
//! ponytail: the real plugin hooks `Installer::install`/`update`/`uninstall`
//! (called once per package as Composer lands it, via
//! `Plugin::activate`'s `addInstaller`), not any script event; viv has no
//! such per-package seam here, so this instead recomputes the whole file
//! from the final package set at `PRE_AUTOLOAD_DUMP`
//! (`Plugins::apply_pre_autoload_dump`), the closest existing hook. Content
//! is deterministic from that set either way, so a fresh install's output is
//! identical to the real plugin's; the only divergence is a needless
//! rewrite on a bare `dump-autoload`, which the real plugin never touches
//! since no package is being (re)installed then.
//!
//! `apply` orders entries via `super::in_install_order`
//! (`autoload::generator::install_order`'s DFS, shared with the autoloader
//! since #130 — see that function's own doc comment for the algorithm).
//! That DFS reproduces Composer whenever two `yii2-extension` packages share
//! a real `require` edge; when they don't (`yiisoft/yii2-app-basic`'s own
//! `require`/`require-dev` split, #130), the real plugin's write order comes
//! from `LibraryInstaller::install`'s per-package async callback instead —
//! archive-extraction completion order, not the require graph — so it isn't
//! reproducible even by two runs of real Composer against the same lock;
//! confirmed by rerunning `compat/run.sh` against the pinned commit twice
//! and getting two different orders. No static sort can match that.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::{Map, Value, json};

use super::phpstan::var_export;
use crate::lock::Package;

const PACKAGE_TYPE: &str = "yii2-extension";

pub(super) fn apply(vendor_dir: &Path, packages: &[(&Package, PathBuf)]) -> Result<()> {
    let mut extensions = Map::new();
    for (package, install_dir) in super::in_install_order(packages) {
        if package.r#type != PACKAGE_TYPE {
            continue;
        }
        let mut extension = Map::new();
        extension.insert("name".to_string(), json!(package.name));
        extension.insert(
            "version".to_string(),
            json!(
                crate::version::normalize(&package.version)
                    .with_context(|| format!("{}: normalising version", package.name))?
            ),
        );
        let alias = generate_default_alias(vendor_dir, install_dir, package);
        if !alias.is_empty() {
            extension.insert("alias".to_string(), Value::Object(alias));
        }
        if let Some(bootstrap) = package.raw.pointer("/extra/bootstrap") {
            extension.insert("bootstrap".to_string(), bootstrap.clone());
        }
        extensions.insert(package.name.clone(), Value::Object(extension));
    }

    let dir = vendor_dir.join("yiisoft");
    fs_err::create_dir_all(&dir)?;
    // `Plugin::activate`'s pre-create: a fresh `vendor/` with the plugin
    // enabled but no `yii2-extension` package ever calls `saveExtensions`,
    // leaving just this stub.
    let body = if extensions.is_empty() {
        "<?php\n\nreturn [];\n".to_string()
    } else {
        render(&extensions)
    };
    fs_err::write(dir.join("extensions.php"), body)?;
    Ok(())
}

/// `saveExtensions`: `var_export`, then every `'<vendor-dir>` swapped for
/// `$vendorDir . '` — safe because `var_export` only ever emits that
/// substring as the start of a single-quoted string literal.
fn render(extensions: &Map<String, Value>) -> String {
    let array = var_export(&Value::Object(extensions.clone()), 0)
        .replace("'<vendor-dir>", "$vendorDir . '");
    format!("<?php\n\n$vendorDir = dirname(__DIR__);\n\nreturn {array};\n")
}

/// `generateDefaultAlias`: one `@Namespace/Path` alias per `psr-0`/`psr-4`
/// autoload entry, tagged `<vendor-dir>` when the resolved path falls under
/// it. A `psr-4` entry with more than one search path (`is_array($path)`) is
/// skipped — Composer's own comment: no way to turn that into one alias.
fn generate_default_alias(
    vendor_dir: &Path,
    install_dir: &Path,
    package: &Package,
) -> Map<String, Value> {
    let mut aliases = Map::new();
    let Some(autoload) = &package.autoload else {
        return aliases;
    };
    let vendor_dir = normalize_path(&vendor_dir.to_string_lossy());

    if let Some(psr0) = autoload.get("psr-0").and_then(Value::as_object) {
        for (name, path) in psr0 {
            let Some(path) = path.as_str() else { continue };
            let name = name.trim_matches('\\').replace('\\', "/");
            let path = normalize_path(&resolve_autoload_path(install_dir, path));
            let tagged = format!("{}/{name}", tag_path(&path, &vendor_dir));
            aliases.insert(format!("@{name}"), json!(tagged));
        }
    }
    if let Some(psr4) = autoload.get("psr-4").and_then(Value::as_object) {
        for (name, path) in psr4 {
            let Some(path) = path.as_str() else { continue };
            let name = name.trim_matches('\\').replace('\\', "/");
            let path = normalize_path(&resolve_autoload_path(install_dir, path));
            aliases.insert(format!("@{name}"), json!(tag_path(&path, &vendor_dir)));
        }
    }
    aliases
}

/// `!$fs->isAbsolutePath($path) ? $this->vendorDir . '/' . $package->getPrettyName() . '/' . $path : $path`,
/// POSIX-only (no drive letters/UNC — vivace only ever runs one platform's
/// paths through this).
pub(super) fn resolve_autoload_path(install_dir: &Path, path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("{}/{path}", install_dir.to_string_lossy())
    }
}

/// `Filesystem::normalizePath`, POSIX paths only: backslashes to forward
/// slashes, `.`/`..` segments collapsed, no trailing separator.
pub(super) fn normalize_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    let absolute = path.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    let mut up = false;
    for chunk in path.trim_start_matches('/').split('/') {
        if chunk == ".." && (absolute || up) {
            parts.pop();
            up = !(parts.is_empty() || parts.last() == Some(&".."));
        } else if chunk != "." && !chunk.is_empty() {
            parts.push(chunk);
            up = chunk != "..";
        }
    }
    format!("{}{}", if absolute { "/" } else { "" }, parts.join("/"))
}

/// `strpos($path . '/', $vendorDir . '/') === 0 ? '<vendor-dir>' . substr($path, strlen($vendorDir)) : $path`.
pub(super) fn tag_path(path: &str, vendor_dir: &str) -> String {
    if format!("{path}/").starts_with(&format!("{vendor_dir}/")) {
        format!("<vendor-dir>{}", &path[vendor_dir.len()..])
    } else {
        path.to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::lock::read_lock;

    fn package(name: &str, extra: &Value, autoload: &Value) -> Package {
        let raw = json!({
            "name": name,
            "version": "1.0.0",
            "type": "yii2-extension",
            "extra": extra,
            "autoload": autoload,
        });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    #[test]
    fn normalize_path_collapses_dot_segments() {
        assert_eq!(
            normalize_path("/vendor/acme/pkg/../pkg2/./src"),
            "/vendor/acme/pkg2/src"
        );
    }

    #[test]
    fn tag_path_prefixes_a_path_under_vendor_dir() {
        assert_eq!(
            tag_path("/project/vendor/acme/pkg/src", "/project/vendor"),
            "<vendor-dir>/acme/pkg/src"
        );
    }

    #[test]
    fn tag_path_leaves_a_path_outside_vendor_dir_untouched() {
        assert_eq!(
            tag_path("/elsewhere/src", "/project/vendor"),
            "/elsewhere/src"
        );
    }

    #[test]
    fn generate_default_alias_tags_a_psr4_path_under_vendor() {
        let package = package(
            "acme/widget",
            &json!({}),
            &json!({"psr-4": {"Acme\\Widget\\": "src/"}}),
        );
        let vendor_dir = Path::new("/project/vendor");
        let install_dir = vendor_dir.join("acme/widget");
        let alias = generate_default_alias(vendor_dir, &install_dir, &package);
        assert_eq!(
            alias.get("@Acme/Widget").unwrap(),
            "<vendor-dir>/acme/widget/src"
        );
    }

    #[test]
    fn generate_default_alias_skips_a_multi_path_psr4_entry() {
        let package = package(
            "acme/widget",
            &json!({}),
            &json!({"psr-4": {"Acme\\Widget\\": ["src/", "lib/"]}}),
        );
        let vendor_dir = Path::new("/project/vendor");
        let install_dir = vendor_dir.join("acme/widget");
        let alias = generate_default_alias(vendor_dir, &install_dir, &package);
        assert!(alias.is_empty());
    }

    #[test]
    fn apply_writes_the_empty_array_stub_without_any_extension() {
        let vendor_dir = tempfile::tempdir().unwrap();
        let packages: Vec<(&Package, PathBuf)> = Vec::new();
        apply(vendor_dir.path(), &packages).unwrap();
        let got = fs_err::read_to_string(vendor_dir.path().join("yiisoft/extensions.php")).unwrap();
        assert_eq!(got, "<?php\n\nreturn [];\n");
    }

    /// #92's fixture, byte-diffed against real `yiisoft/yii2-composer`
    /// 2.0.11's own output (`tests/fixtures/plugins/yii2/expected`).
    #[test]
    fn yii2_fixture_matches_composers_golden() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/yii2");
        let lock = read_lock(&dir.join("composer.lock")).unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let packages: Vec<(&Package, PathBuf)> = lock
            .packages(true)
            .map(|p| (p, vendor_dir.join(&p.name)))
            .collect();

        apply(&vendor_dir, &packages).unwrap();

        let got = fs_err::read_to_string(vendor_dir.join("yiisoft/extensions.php")).unwrap();
        let want = fs_err::read_to_string(dir.join("expected/extensions.php")).unwrap();
        assert_eq!(got, want);
    }
}
