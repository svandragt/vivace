//! `automattic/jetpack-autoloader`'s `CustomAutoloaderPlugin::postAutoloadDump`
//! (`POST_AUTOLOAD_DUMP`): copies the plugin's own runtime files into
//! `vendor/jetpack-autoloader/` (plus `vendor/autoload_packages.php`) with a
//! namespace made from the autoloader suffix and the plugin's version, then
//! writes `jetpack_autoload_{classmap,psr4,filemap}.php` into
//! `vendor/composer/`: every autoload rule of the root and the installed
//! packages, each with the version of the package that declared it.
//!
//! Ported from `automattic/jetpack-autoloader` `v6.0.2` (fetched 2026-10-21,
//! issue #389): `CustomAutoloaderPlugin`, `AutoloadGenerator`,
//! `AutoloadProcessor`, `ManifestGenerator` and `AutoloadFileWriter`. The
//! runtime files are read from the installed package's own `src/`, so a
//! newer plugin version needs no change here unless one of those five
//! classes does.
//!
//! The plugin differs from Composer's own generator in ways that show in the
//! output: the root package's `autoload-dev` is always included; a PSR-4
//! directory is only turned into a class map with `-o`, but PSR-0 is always,
//! and only when some package has a `classmap` rule (`processClassmap`
//! returns early otherwise); the scan keeps every class a file declares,
//! whatever its namespace; and a PSR-4 namespace takes the version of the
//! last package that declares it.
//!
//! ponytail: the root's version is `composer.json`'s `version`, else the git
//! guess, else `1.0.0`, the same as `installed.php`; `COMPOSER_ROOT_VERSION`
//! and a root `branch-alias` are not consulted. Packages are visited in
//! `install_order` without its plugin-first step, so a PSR-4 namespace shared
//! by two packages lists their paths in install order, not Composer's actual
//! local-repository order. The ".phar" test is `path_code`'s regex, not the
//! plugin's `strpos`. The progress lines the plugin prints are not printed.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::{Map, Value};

use super::{Adapter, Ctx};
use crate::autoload::classmap::{ClassName, scan_paths};
use crate::autoload::generator::{
    install_order, merge_recursive, normalize_path, object, path_code, path_str,
    sort_by_dependency_weight, strings,
};
use crate::autoload::php::{export_bytes, export_str};
use crate::install::package_dir;
use crate::lock::{Package, Root};
use crate::version;

const NAME: &str = "automattic/jetpack-autoloader";

/// Files `removeLegacyFiles` deletes from `vendor/`.
const LEGACY_FILES: &[&str] = &[
    "autoload_functions.php",
    "class-autoloader-handler.php",
    "class-classes-handler.php",
    "class-files-handler.php",
    "class-plugins-handler.php",
    "class-version-selector.php",
];

/// The plugin's own classes: `AutoloadFileWriter` does not copy these.
const NOT_COPIED: &[&str] = &[
    "AutoloadGenerator.php",
    "AutoloadProcessor.php",
    "CustomAutoloaderPlugin.php",
    "ManifestGenerator.php",
    "AutoloadFileWriter.php",
];

pub(super) struct JetpackAutoloader;

impl Adapter for JetpackAutoloader {
    fn plugin_names(&self) -> &'static [&'static str] {
        &[NAME]
    }

    fn upstream_version(&self) -> &'static str {
        "v6.0.2"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/plugins/jetpack-autoloader"
    }

    fn post_autoload_dump(
        &self,
        ctx: &Ctx<'_>,
        packages: &[&Package],
        scan_psr: bool,
    ) -> Result<()> {
        apply(ctx, packages, scan_psr)
    }
}

/// One autoload rule's path, as `parseAutoloadsType` builds it (the
/// package's install path, then the rule's own path), and the version of the
/// package that declared it.
struct Source {
    path: String,
    version: String,
}

#[derive(Default)]
struct Autoloads {
    psr0: BTreeMap<String, Vec<Source>>,
    psr4: BTreeMap<String, Vec<Source>>,
    classmap: Vec<Source>,
    /// Keyed by `md5(name:path)`; a repeated key replaces the value in place.
    files: Vec<(String, Source)>,
}

/// A package, or the root, as `parseAutoloadsType` reads it.
struct Row<'a> {
    name: &'a str,
    autoload: Map<String, Value>,
    /// `None` for a metapackage, `""` for the root.
    install_path: Option<String>,
    version: String,
}

/// `(key as PHP source, version, path as PHP source)`.
type ManifestRow = (Vec<u8>, String, String);

/// `isRequiredByRoot`: the plugin does nothing for a package that only a
/// dependency requires.
fn required_by_root(root: &Root) -> bool {
    root.require
        .keys()
        .chain(root.require_dev.keys())
        .any(|name| name.eq_ignore_ascii_case(NAME))
}

fn apply(ctx: &Ctx<'_>, packages: &[&Package], scan_psr: bool) -> Result<()> {
    if !required_by_root(ctx.root) {
        return Ok(());
    }
    // Composer never loads the plugin when it is not installed (`require-dev`
    // under `--no-dev`).
    let Some(plugin) = packages.iter().find(|p| p.name == NAME) else {
        return Ok(());
    };
    if ctx.root.config.vendor_dir != "vendor" {
        bail!(
            "{NAME} needs the default vendor directory, but config.vendor-dir is `{}`",
            ctx.root.config.vendor_dir
        );
    }

    let project = fs_err::canonicalize(ctx.project_dir)?;
    let vendor = fs_err::canonicalize(ctx.vendor_dir)?;
    let source = package_dir(&vendor, &project, plugin).join("src");
    let plugin_version = plugin_version(&source)?;
    let suffix = suffix(ctx)?;

    let (base, vendor_str) = (
        normalize_path(&path_str(&project)),
        normalize_path(&path_str(&vendor)),
    );
    let autoloads = parse_autoloads(ctx.root, packages, &plugin_version, &project, &vendor)?;
    let psr4 = (!scan_psr).then(|| psr4_rows(&autoloads, &base, &vendor_str));
    let classmap = classmap_rows(&autoloads, scan_psr, &base, &vendor_str)?;
    let files = autoloads
        .files
        .iter()
        .map(|(id, s)| {
            (
                export_str(id).into_bytes(),
                s.version.clone(),
                path_code(&base, &vendor_str, &s.path).plain(),
            )
        })
        .collect::<Vec<_>>();

    for legacy in LEGACY_FILES {
        remove_if_exists(&vendor.join(legacy))?;
    }
    write_runtime_files(&source, &vendor, &suffix, &plugin_version)?;
    let composer_dir = vendor.join("composer");
    for (file, rows) in [
        ("jetpack_autoload_classmap.php", classmap),
        ("jetpack_autoload_psr4.php", psr4.unwrap_or_default()),
        ("jetpack_autoload_filemap.php", files),
    ] {
        let path = composer_dir.join(file);
        remove_if_exists(&path)?;
        if !rows.is_empty() {
            fs_err::write(&path, manifest(file, &rows))?;
        }
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs_err::remove_file(path)?;
    }
    Ok(())
}

/// `AutoloadGenerator::VERSION`, which `AutoloadFileWriter` puts in the
/// namespace and `parseAutoloadsType` records for the plugin's own classes.
fn plugin_version(source: &Path) -> Result<String> {
    static VERSION: OnceLock<Regex> = OnceLock::new();
    let file = source.join("AutoloadGenerator.php");
    let text = fs_err::read_to_string(&file)?;
    VERSION
        .get_or_init(|| Regex::new(r"const VERSION = '([^']+)';").expect("valid regex"))
        .captures(&text)
        .map(|caps| caps[1].to_string())
        .with_context(|| format!("{}: no `const VERSION`", file.display()))
}

/// `CustomAutoloaderPlugin::determineSuffix`, minus the random fallback:
/// `vendor/autoload.php` already exists when this runs.
fn suffix(ctx: &Ctx<'_>) -> Result<String> {
    static OWN: OnceLock<Regex> = OnceLock::new();
    static COMPOSER: OnceLock<Regex> = OnceLock::new();
    if let Some(suffix) = ctx
        .root
        .config
        .autoloader_suffix
        .as_ref()
        .filter(|s| !s.is_empty())
    {
        return Ok(suffix.clone());
    }
    let own = OWN.get_or_init(|| {
        Regex::new(r"(?m)^namespace Automattic\\Jetpack\\Autoloader\\jp([^;\s]+?)(?:\\al[^;\s]+)?;")
            .expect("valid regex")
    });
    let composer = COMPOSER
        .get_or_init(|| Regex::new(r"ComposerAutoloaderInit([^:\s]+)::").expect("valid regex"));
    for (file, re) in [("autoload_packages.php", own), ("autoload.php", composer)] {
        if let Ok(text) = fs_err::read_to_string(ctx.vendor_dir.join(file))
            && let Some(caps) = re.captures(&text)
        {
            return Ok(caps[1].to_string());
        }
    }
    bail!("{NAME}: no autoloader suffix in vendor/autoload.php")
}

fn root_version(root: &Root, project: &Path) -> Result<String> {
    let pretty = root
        .version
        .clone()
        .or_else(|| crate::vcs::guess_root_version(project).map(|v| v.pretty_version))
        .unwrap_or_else(|| "1.0.0".into());
    version::normalize(&pretty).context("root version")
}

/// `AutoloadGenerator::parseAutoloads`.
fn parse_autoloads(
    root: &Root,
    packages: &[&Package],
    plugin_version: &str,
    project: &Path,
    vendor: &Path,
) -> Result<Autoloads> {
    let ordered = install_order(packages);
    let rows = ordered
        .iter()
        .map(|p| {
            let version = if p.name == NAME {
                plugin_version.to_string()
            } else {
                version::normalize(&p.version).with_context(|| format!("{}: version", p.name))?
            };
            let mut install_path =
                (p.r#type != "metapackage").then(|| path_str(&package_dir(vendor, project, p)));
            // A package's install path includes its target-dir; autoload
            // paths are relative to the directory above it.
            if let (Some(path), Some(target)) = (
                install_path.as_mut(),
                p.target_dir.as_deref().filter(|t| !t.is_empty()),
            ) && let Some(stripped) = path.strip_suffix(&format!("/{target}"))
            {
                *path = stripped.to_string();
            }
            Ok(Row {
                name: p.name.as_str(),
                autoload: p.autoload.as_ref().map(object).unwrap_or_default(),
                install_path,
                version,
            })
        })
        .collect::<Result<Vec<_>>>()?;

    let mut root_autoload = root.autoload.as_ref().map(object).unwrap_or_default();
    merge_recursive(
        &mut root_autoload,
        &root.autoload_dev.as_ref().map(object).unwrap_or_default(),
    );
    let root_row = Row {
        name: root.name.as_deref().unwrap_or("__root__"),
        autoload: root_autoload,
        install_path: Some(String::new()),
        version: root_version(root, project)?,
    };

    let by_name: HashMap<&str, &Row> = rows.iter().map(|r| (r.name, r)).collect();
    let sorted: Vec<&Row> = sort_by_dependency_weight(&ordered)
        .into_iter()
        .map(|p| by_name[p.name.as_str()])
        .collect();
    // `psr-0`/`psr-4` take the root, then the packages in their original
    // order; `classmap` the root, then the sorted packages reversed; `files`
    // the sorted packages, then the root.
    let psr_order: Vec<&Row> = std::iter::once(&root_row).chain(&rows).collect();
    let classmap_order: Vec<&Row> = std::iter::once(&root_row)
        .chain(sorted.iter().rev().copied())
        .collect();
    let files_order: Vec<&Row> = sorted
        .iter()
        .copied()
        .chain(std::iter::once(&root_row))
        .collect();

    let mut autoloads = Autoloads::default();
    for (namespace, source, _) in collect(&psr_order, "psr-0") {
        autoloads.psr0.entry(namespace).or_default().push(source);
    }
    for (namespace, source, _) in collect(&psr_order, "psr-4") {
        autoloads.psr4.entry(namespace).or_default().push(source);
    }
    for (_, source, _) in collect(&classmap_order, "classmap") {
        autoloads.classmap.push(source);
    }
    for (_, source, id) in collect(&files_order, "files") {
        match autoloads
            .files
            .iter_mut()
            .find(|(existing, _)| *existing == id)
        {
            Some(slot) => slot.1 = source,
            None => autoloads.files.push((id, source)),
        }
    }
    Ok(autoloads)
}

/// `(namespace, source, file identifier)` for every rule of one autoload
/// type, in the order `rows` lists the packages.
fn collect(rows: &[&Row], kind: &str) -> Vec<(String, Source, String)> {
    let mut found = Vec::new();
    for row in rows {
        let Some(install_path) = row.install_path.as_deref() else {
            continue;
        };
        let rules: Vec<(String, Vec<String>)> = match row.autoload.get(kind) {
            Some(Value::Object(map)) => map
                .iter()
                .map(|(namespace, paths)| (namespace.clone(), strings(paths)))
                .collect(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|paths| (String::new(), strings(paths)))
                .collect(),
            _ => continue,
        };
        for (namespace, paths) in rules {
            for path in paths {
                let relative = match (install_path.is_empty(), path.is_empty()) {
                    (true, true) => ".".to_string(),
                    (true, false) => path.clone(),
                    (false, _) => format!("{install_path}/{path}"),
                };
                let id = format!("{:x}", md5::compute(format!("{}:{path}", row.name)));
                let version = row.version.clone();
                found.push((
                    namespace.clone(),
                    Source {
                        path: relative,
                        version,
                    },
                    id,
                ));
            }
        }
    }
    found
}

/// `AutoloadProcessor::processPsr4Packages`, rendered. `krsort`: the longest
/// namespace of a shared prefix first.
fn psr4_rows(autoloads: &Autoloads, base: &str, vendor: &str) -> Vec<ManifestRow> {
    autoloads
        .psr4
        .iter()
        .rev()
        .map(|(namespace, sources)| {
            let paths: Vec<String> = sources
                .iter()
                .map(|s| path_code(base, vendor, &s.path).plain())
                .collect();
            (
                export_str(namespace).into_bytes(),
                // PHP leaks the loop variable: the last source's version.
                sources
                    .last()
                    .map(|s| s.version.clone())
                    .unwrap_or_default(),
                format!("array( {} )", paths.join(", ")),
            )
        })
        .collect()
}

/// `AutoloadProcessor::processClassmap`, rendered: class name to the
/// version and path of the last source that declares it, sorted by name.
fn classmap_rows(
    autoloads: &Autoloads,
    scan_psr: bool,
    base: &str,
    vendor: &str,
) -> Result<Vec<ManifestRow>> {
    let scan_psr = scan_psr && !autoloads.psr4.is_empty();
    if autoloads.classmap.is_empty() && !scan_psr {
        return Ok(Vec::new());
    }
    let mut classes: BTreeMap<ClassName, (String, String)> = BTreeMap::new();
    let sources = autoloads
        .psr4
        .values()
        .rev()
        .flatten()
        .filter(|_| scan_psr)
        .chain(autoloads.psr0.values().rev().flatten())
        .chain(&autoloads.classmap);
    for source in sources {
        let dir = normalize_path(&if source.path.starts_with('/') {
            source.path.clone()
        } else {
            format!("{base}/{}", source.path)
        });
        // Composer's scanner reads a missing path as empty.
        if !Path::new(&dir).exists() {
            continue;
        }
        for (class, file) in scan_paths(Path::new(&dir), None)?.map {
            let code = path_code(base, vendor, &path_str(&file)).plain();
            classes.insert(class, (source.version.clone(), code));
        }
    }
    Ok(classes
        .into_iter()
        .map(|(class, (version, code))| (export_bytes(&class), version, code))
        .collect())
}

/// `ManifestGenerator::buildFile`.
fn manifest(file: &str, rows: &[ManifestRow]) -> Vec<u8> {
    let mut out = format!(
        "<?php\n\n// This file `{file}` was auto generated by {NAME}.\n\n\
         $vendorDir = dirname(__DIR__);\n$baseDir   = dirname($vendorDir);\n\nreturn array(\n"
    )
    .into_bytes();
    for (key, version, path) in rows {
        out.push(b'\t');
        out.extend(key);
        out.extend(
            format!(
                " => array(\n\t\t'version' => {},\n\t\t'path'    => {path}\n\t),\n",
                export_str(version)
            )
            .as_bytes(),
        );
    }
    out.extend(b");\n");
    out
}

/// `AutoloadFileWriter::copyAutoloaderFiles`: every `.php` file of the
/// plugin's `src/` that is not one of its generator classes, with the
/// header (a namespace made from the suffix and version) substituted;
/// `autoload.php` becomes `vendor/autoload_packages.php`.
fn write_runtime_files(
    source: &Path,
    vendor: &Path,
    suffix: &str,
    plugin_version: &str,
) -> Result<()> {
    let out = vendor.join("jetpack-autoloader");
    if out.exists() {
        fs_err::remove_dir_all(&out)?;
    }
    fs_err::create_dir_all(&out)?;

    let namespace = if suffix == "Current" {
        "Automattic\\Jetpack\\Autoloader\\jpCurrent".to_string()
    } else {
        let version: String = plugin_version
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        format!("Automattic\\Jetpack\\Autoloader\\jp{suffix}\\al{version}")
    };
    let header = format!(
        "/**\n * This file was automatically generated by {NAME}.\n *\n * @package {NAME}\n */\n\n\
         namespace {namespace};\n\n"
    );

    let mut files: Vec<PathBuf> = fs_err::read_dir(source)?
        .map(|entry| entry.map(|e| e.path()))
        .collect::<std::io::Result<_>>()?;
    files.sort();
    for file in files {
        let name = file
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if file.extension().is_none_or(|ext| ext != "php")
            || !file.is_file()
            || NOT_COPIED.contains(&name)
        {
            continue;
        }
        let content = fs_err::read_to_string(&file)?.replace("/* HEADER */", &header);
        let dest = if name == "autoload.php" {
            vendor.join("autoload_packages.php")
        } else {
            out.join(name)
        };
        fs_err::write(dest, content)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn root(value: Value) -> Root {
        serde_json::from_value(value).unwrap()
    }

    #[test]
    fn only_a_root_requirement_counts() {
        assert!(required_by_root(&root(
            json!({"require": {"Automattic/Jetpack-Autoloader": "^5"}})
        )));
        assert!(required_by_root(&root(
            json!({"require-dev": {"automattic/jetpack-autoloader": "^5"}})
        )));
        assert!(!required_by_root(&root(
            json!({"require": {"acme/lib": "^1"}})
        )));
    }
}
