//! `altis/cms-installer`'s `Plugin::install_files`/`generate_module_manifest`
//! (`inc/composer/class-plugin.php`): on every install/update, copies two
//! fixed files out of `vendor/altis/cms/` into the project root
//! unconditionally (`index.php`, `wp-config.php`), copies a third
//! (`.build-script`) only if the project doesn't already have one, writes a
//! starter `.gitignore` if none exists, and creates `content/`,
//! `content/plugins/`, `content/themes/` if missing; then, once the
//! autoload dump runs, writes `vendor/modules.php` requiring every
//! installed package's `load.php` that declares an `extra.altis` section,
//! plus every `extra.altis.modules.*.entrypoint` the root `composer.json`
//! itself names (filtered to the ones that exist) — so Altis's runtime
//! module loader can `require` one generated file instead of walking the
//! installed-packages list itself.
//!
//! Ported from `altis/cms-installer` `0.4.4` (`5186116`, fetched
//! 2026-10-02, issue #355). Not `Rule::Scaffold`
//! (`drupal_scaffold.rs`/`data.rs`): every path and filename here is fixed,
//! with no `extra`-driven `allowed-packages`/`file-mapping` section of its
//! own to bind a data file to, so a small adapter is the better fit.
//!
//! Two phases, like `drupal_scaffold.rs`/`patches.rs`: the file copy runs
//! at `post_link` (packages are landed, same timing as the real plugin's
//! `post-install-cmd`/`post-update-cmd`); the manifest at
//! `pre_autoload_dump`, standing in for the real `post-autoload-dump` event
//! the same way `yii2.rs`'s own doc comment explains — the content depends
//! only on the final package set, not on dump order.
//!
//! Composer's `copy()` never throws on a missing source (just an
//! `E_WARNING`); neither does this — a project that doesn't `require
//! altis/cms` leaves the three files untouched rather than failing the
//! install.
//!
//! `generate_module_manifest` recomputes every package's `load.php` path as
//! `vendor_dir . '/' . name`, not the package's actual resolved install
//! directory, so this does too — a module remapped elsewhere by another
//! installer would still be `require`d from its default vendor path in both
//! the real plugin and this port, a `load.php` neither ever wrote there.
//!
//! `getCanonicalPackages()`'s own order (what the real plugin loops) is
//! Composer's install order, reproduced here via `super::in_install_order`
//! the same way `yii2.rs`/`craft.rs` do — and, confirmed against a real
//! Altis site's own `vendor/modules.php` (#355), not reproducible from the
//! lock alone when the ordered packages share no `require` edge with each
//! other (every Altis module is a sibling the root requires directly, none
//! requiring another), the same already-documented gap `yii2.rs`'s own doc
//! comment calls out for `yiisoft/yii2-app-basic`. The entries themselves
//! are never wrong, only their relative order — a site whose modules do
//! have real `require` edges between them reproduces exactly.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;

use crate::lock::{Package, Root};

use super::{Adapter, Ctx, in_install_order};

const PACKAGE_NAME: &str = "altis/cms-installer";
const CMS_PACKAGE: &str = "altis/cms";
const GITIGNORE: &str =
    "# Altis\n/wordpress\n/index.php\n/wp-config.php\n/chassis\n/vendor\n/content/uploads";

pub(super) struct AltisCmsInstaller;

impl Adapter for AltisCmsInstaller {
    fn plugin_names(&self) -> &'static [&'static str] {
        &[PACKAGE_NAME]
    }

    fn upstream_version(&self) -> &'static str {
        "0.4.4"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/plugins/altis"
    }

    fn post_link(
        &self,
        ctx: &Ctx<'_>,
        _newly_linked: &[Package],
        _kept: &[&Package],
        _store: &crate::store::Store,
    ) -> Result<()> {
        install_files(ctx.project_dir, ctx.vendor_dir)
    }

    fn pre_autoload_dump(&self, ctx: &Ctx<'_>, packages: &[(&Package, PathBuf)]) -> Result<()> {
        generate_module_manifest(ctx.root, ctx.project_dir, ctx.vendor_dir, packages)
    }
}

fn install_files(project_dir: &Path, vendor_dir: &Path) -> Result<()> {
    let source = vendor_dir.join(CMS_PACKAGE);

    copy_if_source_exists(&source.join("index.php"), &project_dir.join("index.php"))?;
    copy_if_source_exists(
        &source.join("wp-config.php"),
        &project_dir.join("wp-config.php"),
    )?;

    let build_script = project_dir.join(".build-script");
    if !build_script.exists() {
        copy_if_source_exists(&source.join(".build-script"), &build_script)?;
    }

    let gitignore = project_dir.join(".gitignore");
    if !gitignore.exists() {
        fs_err::write(&gitignore, GITIGNORE).context("altis/cms-installer: writing .gitignore")?;
    }

    for dir in ["content", "content/plugins", "content/themes"] {
        let dir = project_dir.join(dir);
        if !dir.is_dir() {
            fs_err::create_dir(&dir)
                .with_context(|| format!("altis/cms-installer: creating {}", dir.display()))?;
        }
    }
    Ok(())
}

fn copy_if_source_exists(source: &Path, dest: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    fs_err::copy(source, dest)
        .with_context(|| format!("altis/cms-installer: copying {}", source.display()))?;
    Ok(())
}

fn generate_module_manifest(
    root: &Root,
    project_dir: &Path,
    vendor_dir: &Path,
    packages: &[(&Package, PathBuf)],
) -> Result<()> {
    let mut manifest =
        String::from("<?php\n/**\n * Altis Module Loader.\n *\n * DO NOT EDIT THIS FILE.\n */\n");

    for (package, _) in in_install_order(packages) {
        if package.raw.pointer("/extra/altis").is_none() {
            continue;
        }
        let load_php = vendor_dir.join(package.pretty_name()).join("load.php");
        if !load_php.exists() {
            continue;
        }
        let _ = write!(
            manifest,
            "\n// Load {}.\nrequire_once __DIR__ . '/{}/load.php';",
            package.name,
            package.pretty_name()
        );
    }

    if let Some(modules) = root
        .extra
        .pointer("/altis/modules")
        .and_then(Value::as_object)
    {
        for (name, config) in modules {
            let Some(entrypoint) = config.get("entrypoint") else {
                continue;
            };
            let entrypoints: Vec<String> = match entrypoint {
                Value::Array(items) => items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                Value::String(s) => vec![s.clone()],
                _ => Vec::new(),
            };
            let existing: Vec<&String> = entrypoints
                .iter()
                .filter(|path| project_dir.join(path).exists())
                .collect();
            if existing.is_empty() {
                continue;
            }
            let _ = write!(manifest, "\n// Load {name}.\n");
            let lines: Vec<String> = existing
                .iter()
                .map(|path| {
                    format!("require_once dirname( __DIR__ ) . DIRECTORY_SEPARATOR . '{path}';")
                })
                .collect();
            manifest.push_str(&lines.join("\n"));
        }
    }

    manifest.push('\n');
    fs_err::write(vendor_dir.join("modules.php"), manifest)
        .context("altis/cms-installer: writing vendor/modules.php")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, extra: Value) -> Package {
        let mut raw = serde_json::json!({ "name": name, "version": "1.0.0", "type": "library" });
        raw["extra"] = extra;
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    fn root(extra: &Value) -> Root {
        serde_json::from_value(serde_json::json!({ "extra": extra })).unwrap()
    }

    #[test]
    fn install_files_copies_fixed_names_and_scaffolds_content_dirs() {
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let cms = vendor_dir.join("altis/cms");
        fs_err::create_dir_all(&cms).unwrap();
        fs_err::write(cms.join("index.php"), "<?php // index\n").unwrap();
        fs_err::write(cms.join("wp-config.php"), "<?php // wp-config\n").unwrap();
        fs_err::write(cms.join(".build-script"), "#!/bin/sh\n").unwrap();

        install_files(project_dir.path(), &vendor_dir).unwrap();

        assert_eq!(
            fs_err::read_to_string(project_dir.path().join("index.php")).unwrap(),
            "<?php // index\n"
        );
        assert_eq!(
            fs_err::read_to_string(project_dir.path().join("wp-config.php")).unwrap(),
            "<?php // wp-config\n"
        );
        assert_eq!(
            fs_err::read_to_string(project_dir.path().join(".gitignore")).unwrap(),
            GITIGNORE
        );
        assert!(project_dir.path().join("content/plugins").is_dir());
        assert!(project_dir.path().join("content/themes").is_dir());
    }

    #[test]
    fn install_files_leaves_an_existing_build_script_untouched() {
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let cms = vendor_dir.join("altis/cms");
        fs_err::create_dir_all(&cms).unwrap();
        fs_err::write(cms.join(".build-script"), "new\n").unwrap();
        fs_err::write(project_dir.path().join(".build-script"), "old\n").unwrap();

        install_files(project_dir.path(), &vendor_dir).unwrap();

        assert_eq!(
            fs_err::read_to_string(project_dir.path().join(".build-script")).unwrap(),
            "old\n"
        );
    }

    #[test]
    fn generate_module_manifest_requires_altis_packages_and_root_entrypoints() {
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let module_dir = vendor_dir.join("acme/altis-module");
        fs_err::create_dir_all(&module_dir).unwrap();
        fs_err::write(module_dir.join("load.php"), "<?php\n").unwrap();
        fs_err::write(project_dir.path().join("custom.php"), "<?php\n").unwrap();

        let module = package("acme/altis-module", serde_json::json!({"altis": {}}));
        let other = package("acme/widget", Value::Null);
        let packages: Vec<(&Package, PathBuf)> =
            vec![(&module, PathBuf::new()), (&other, PathBuf::new())];

        let root = root(&serde_json::json!({
            "altis": { "modules": { "custom": { "entrypoint": ["custom.php"] } } }
        }));

        generate_module_manifest(&root, project_dir.path(), &vendor_dir, &packages).unwrap();

        let got = fs_err::read_to_string(vendor_dir.join("modules.php")).unwrap();
        assert!(got.contains("require_once __DIR__ . '/acme/altis-module/load.php';"));
        assert!(!got.contains("acme/widget"));
        assert!(
            got.contains("require_once dirname( __DIR__ ) . DIRECTORY_SEPARATOR . 'custom.php';")
        );
    }

    #[test]
    fn generate_module_manifest_skips_a_missing_root_entrypoint() {
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        fs_err::create_dir_all(&vendor_dir).unwrap();
        let root = root(&serde_json::json!({
            "altis": { "modules": { "custom": { "entrypoint": ["missing.php"] } } }
        }));

        generate_module_manifest(&root, project_dir.path(), &vendor_dir, &[]).unwrap();

        let got = fs_err::read_to_string(vendor_dir.join("modules.php")).unwrap();
        assert!(!got.contains("custom"));
    }
}
