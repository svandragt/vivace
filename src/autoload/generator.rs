//! Port of Composer's `AutoloadGenerator::dump`: turns the root package and
//! the installed packages into `vendor/autoload.php` and the
//! `vendor/composer/autoload_*.php` files, byte for byte.
//!
//! Paths are handled as `/`-separated strings throughout, as Composer's
//! `Filesystem` does; the tool is Linux-only so Windows prefixes are not
//! ported.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{Map, Value};

use super::classmap::{
    ClassMap, ClassName, ScanKey, Sidecar, SidecarV1, fingerprint_dir, scan_paths,
};
use super::php::{Key, Php, export_bytes, export_static, export_str, loader_properties};
use super::sort::sort_packages;

#[derive(Debug, Clone)]
pub struct RootPackage {
    pub name: String,
    pub autoload: Value,
    pub autoload_dev: Value,
    pub target_dir: Option<String>,
    pub requires: Vec<String>,
    /// `config.include-path`-era per-package include paths (`include_paths.php`).
    pub include_path: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub name: String,
    pub autoload: Value,
    pub requires: Vec<String>,
    pub replaces: Vec<String>,
    pub provides: Vec<String>,
    pub target_dir: Option<String>,
    /// `None` for metapackages: they take part in ordering but autoload nothing.
    pub install_path: Option<PathBuf>,
    pub is_dev: bool,
    pub include_path: Vec<String>,
    /// The store archive dir `install_path` was hardlinked from (its own dir
    /// name is the archive's content hash, the classmap cache key) —
    /// `None` for the root package, a path/git-source/from-source install,
    /// or one the store had no cache hit for. Lets a classmap scan survive
    /// `vendor/` being rebuilt from scratch instead of always rescanning.
    pub archive_dir: Option<PathBuf>,
    /// `type: composer-plugin` or `composer-installer`: `install_order`'s own
    /// `movePluginsToFront` step pulls it (and its non-platform requires)
    /// ahead of every other package.
    pub is_plugin: bool,
    /// A `composer-plugin` with `extra.plugin-modifies-downloads: true`:
    /// `movePluginsToFront`'s separate bucket, moved ahead of every other
    /// plugin.
    pub modifies_downloads: bool,
}

#[derive(Debug, Clone)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the flags of Composer's dump()"
)]
pub struct Input {
    pub root: RootPackage,
    pub packages: Vec<Package>,
    pub dev_mode: bool,
    /// Composer's `--optimize`: also scan PSR-0/PSR-4 directories into the class map.
    pub scan_psr: bool,
    pub suffix: String,
    pub vendor_dir: PathBuf,
    /// The project directory (`composer.json`'s), Composer's `$basePath`.
    pub base_dir: PathBuf,
    /// #311: `base_dir`'s identity as `install::Snapshot::root_fingerprint`
    /// already computed it, reused for the root classmap cache's sidecar
    /// path (#269, `root_classmap_sidecar`) instead of canonicalizing
    /// `base_dir` a second time here. `None` for every caller without a
    /// `Snapshot` (`dump_autoload`, every test in this module) — `generate`
    /// falls back to deriving it from `base_dir` itself, unchanged.
    pub root_fingerprint: Option<String>,
    /// Whether `platform_check.php` is written, so `autoload_real.php` requires it.
    pub platform_check: bool,
    pub prepend_autoloader: bool,
    /// Composer's `--classmap-authoritative`/`-a`: forces `scan_psr`, and
    /// `$loader->setClassMapAuthoritative(true)`.
    pub classmap_authoritative: bool,
    /// Composer's `--apcu-autoloader`: `None` off, `Some(None)` on with a
    /// prefix generated here, `Some(Some(prefix))` on with a fixed prefix.
    pub apcu_prefix: Option<Option<String>>,
    /// `config.use-include-path`: `$loader->setUseIncludePath(true)`.
    pub use_include_path: bool,
}

#[derive(Debug, Default)]
pub struct Generated {
    pub files_written: Vec<PathBuf>,
    pub warnings: Vec<String>,
    pub classmap: BTreeMap<ClassName, PathBuf>,
}

/// How a path is spelled in the generated PHP: relative to `$vendorDir`,
/// to `$baseDir`, or absolute; `rest` is the literal tail (`/src`, `/../src`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct PathCode {
    prefix: Prefix,
    phar: bool,
    rest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prefix {
    Vendor,
    Base,
    Absolute,
}

impl PathCode {
    /// `$vendorDir . '/a/a/src'` as written in the plain map files.
    fn plain(&self) -> String {
        let var = match self.prefix {
            Prefix::Vendor => "$vendorDir . ",
            Prefix::Base => "$baseDir . ",
            Prefix::Absolute => "",
        };
        let phar = if self.phar { "'phar://' . " } else { "" };
        format!("{phar}{var}{}", export_str(&self.rest))
    }

    /// The string PHP would hold after evaluating [`Self::plain`]; Composer
    /// derives the `autoload_static.php` spelling from it.
    fn evaluated(&self, vendor: &str, base: &str) -> String {
        let dir = match self.prefix {
            Prefix::Vendor => vendor,
            Prefix::Base => base,
            Prefix::Absolute => "",
        };
        let phar = if self.phar { "phar://" } else { "" };
        format!("{phar}{dir}{}", self.rest)
    }
}

struct Autoloads {
    psr0: Vec<(String, Vec<String>)>,
    psr4: Vec<(String, Vec<String>)>,
    classmap: Vec<String>,
    files: Vec<(String, String)>,
    exclude: Vec<String>,
}

/// One row of Composer's package map: `(package, install path or null)`.
struct Entry<'a> {
    name: &'a str,
    autoload: Map<String, Value>,
    target_dir: Option<&'a str>,
    /// Empty for the root package.
    install_path: Option<String>,
    is_root: bool,
}

pub fn generate(input: &Input) -> Result<Generated> {
    let mut out = Generated::default();
    let target_dir = input.vendor_dir.join("composer");
    fs_err::create_dir_all(&target_dir)?;

    let base = normalize_path(&path_str(&fs_err::canonicalize(&input.base_dir)?));
    let vendor = normalize_path(&path_str(&fs_err::canonicalize(&input.vendor_dir)?));
    let target = format!("{vendor}/composer");

    let vendor_code = find_shortest_path_code(&target, &vendor, true, false);
    let vendor_to_target_code = find_shortest_path_code(&vendor, &target, true, false);
    let base_code =
        find_shortest_path_code(&vendor, &base, true, false).replace("__DIR__", "$vendorDir");

    let autoloads = parse_autoloads(input, &base);

    let map_file = |kind: &str, rows: &[(String, Vec<PathCode>)]| {
        let mut file = format!(
            "<?php\n\n// autoload_{kind}.php @generated by Composer\n\n$vendorDir = {vendor_code};\n$baseDir = {base_code};\n\nreturn array(\n"
        );
        for (namespace, paths) in rows {
            let paths: Vec<String> = paths.iter().map(PathCode::plain).collect();
            let _ = writeln!(
                file,
                "    {} => array({}),",
                export_str(namespace),
                paths.join(", ")
            );
        }
        file.push_str(");\n");
        file
    };
    let to_codes = |rows: &[(String, Vec<String>)]| -> Vec<(String, Vec<PathCode>)> {
        rows.iter()
            .map(|(ns, paths)| {
                (
                    ns.clone(),
                    paths.iter().map(|p| path_code(&base, &vendor, p)).collect(),
                )
            })
            .collect()
    };
    let psr0 = to_codes(&autoloads.psr0);
    let psr4 = to_codes(&autoloads.psr4);
    let namespaces_file = map_file("namespaces", &psr0);
    let psr4_file = map_file("psr4", &psr4);

    // Root package with a target-dir: an extra PSR-0 loader that strips the
    // target-dir levels, straight from Composer's heredoc.
    let mut target_dir_loader = None;
    if let Some(root_target) = input.root.target_dir.as_deref().filter(|t| !t.is_empty()) {
        let psr0_keys: Vec<String> = object(&input.root.autoload)
            .get("psr-0")
            .and_then(Value::as_object)
            .map(|m| m.keys().map(|k| export_str(k)).collect())
            .unwrap_or_default();
        if !psr0_keys.is_empty() {
            let levels = normalize_path(root_target).matches('/').count() + 1;
            let prefixes = psr0_keys.join(", ");
            let base_from_target = find_shortest_path_code(&target, &base, true, false);
            target_dir_loader = Some(format!(
                "
    public static function autoload($class)
    {{
        $dir = {base_from_target} . '/';
        $prefixes = array({prefixes});
        foreach ($prefixes as $prefix) {{
            if (0 !== strpos($class, $prefix)) {{
                continue;
            }}
            $path = $dir . implode('/', array_slice(explode('\\\\', $class), {levels})).'.php';
            if (!$path = stream_resolve_include_path($path)) {{
                return false;
            }}
            require $path;

            return true;
        }}
    }}
"
            ));
        }
    }

    // Composer forces the PSR scan when classmap-authoritative is set: a
    // loader with no PSR fallback can only resolve what got classmapped.
    let scan_psr = input.scan_psr || input.classmap_authoritative;

    let mut scanner = Scanner {
        base: &base,
        vendor: &vendor,
        excluded: &autoloads.exclude,
        archives: ArchiveIndex::build(&input.packages),
        root_sidecar: cache_root(&input.packages).map(|root| {
            crate::store::root_classmap_sidecar(
                &root,
                input.root_fingerprint.as_deref().unwrap_or(&base),
            )
        }),
        map: BTreeMap::new(),
        scanned: HashSet::new(),
        warnings: Vec::new(),
        regex_cache: HashMap::new(),
        sidecars: HashMap::new(),
        archive_sidecars: HashMap::new(),
        cache_hits: 0,
        cache_misses: 0,
        scan_paths_elapsed: std::time::Duration::ZERO,
        cache_read_elapsed: std::time::Duration::ZERO,
        merge_elapsed: std::time::Duration::ZERO,
        setup_elapsed: std::time::Duration::ZERO,
        cache_write_elapsed: std::time::Duration::ZERO,
    };
    // Every classmap/PSR directory is gathered into one ordered task list
    // before any of it is scanned (#198): `Scanner::scan_all` fans the
    // filesystem walk out across threads, but the list's order is what the
    // fold back into `scanner.map` follows, so building it up front keeps
    // that order identical to this loop nest's, whatever order the threads
    // themselves finish in.
    let mut tasks: Vec<ScanTask> = autoloads
        .classmap
        .iter()
        .map(|dir| ScanTask {
            dir: dir.clone(),
            psr: None,
        })
        .collect();
    if scan_psr {
        // Grouped by namespace, PSR-4 rules before PSR-0 within a namespace,
        // namespaces in reverse order (longest prefixes first).
        let mut to_scan: BTreeMap<&str, Vec<(&[String], &str)>> = BTreeMap::new();
        for (kind, rows) in [("psr-4", &autoloads.psr4), ("psr-0", &autoloads.psr0)] {
            for (namespace, paths) in rows {
                to_scan.entry(namespace).or_default().push((paths, kind));
            }
        }
        for (namespace, groups) in to_scan.iter().rev() {
            for (paths, kind) in groups {
                for dir in *paths {
                    let dir = normalize_path(&if is_absolute(dir) {
                        dir.clone()
                    } else {
                        format!("{base}/{dir}")
                    });
                    let is_dir_started = std::time::Instant::now();
                    let is_dir = Path::new(&dir).is_dir();
                    scanner.setup_elapsed += is_dir_started.elapsed();
                    if !is_dir {
                        continue;
                    }
                    tasks.push(ScanTask {
                        dir,
                        psr: Some(((*namespace).to_string(), (*kind).to_string())),
                    });
                }
            }
        }
    }
    let scan_started = std::time::Instant::now();
    scanner.scan_all(&tasks)?;
    tracing::debug!(
        cache_hits = scanner.cache_hits,
        cache_misses = scanner.cache_misses,
        scan_paths_ms = scanner.scan_paths_elapsed.as_millis(),
        cache_read_ms = scanner.cache_read_elapsed.as_millis(),
        merge_ms = scanner.merge_elapsed.as_millis(),
        setup_ms = scanner.setup_elapsed.as_millis(),
        cache_write_ms = scanner.cache_write_elapsed.as_millis(),
        elapsed_ms = scan_started.elapsed().as_millis(),
        "scanned classmap/PSR directories"
    );
    out.warnings.append(&mut scanner.warnings);
    // #302: the two large PHP files (`autoload_classmap.php`,
    // `autoload_static.php`) are rendered from `classmap` below with no
    // instrumentation of their own before this — timed separately from
    // `scan_all` above so a slow render on a classmap-heavy `-o` install
    // doesn't hide inside "scanned classmap/PSR directories"'s own number.
    let render_started = std::time::Instant::now();
    let mut classmap = scanner.map;
    classmap.insert(
        b"Composer\\InstalledVersions".to_vec(),
        format!("{vendor}/composer/InstalledVersions.php"),
    );

    // A class name is a raw byte string (Composer's classmap keeps whatever
    // bytes the source declared), so this file is built as bytes rather than
    // a `String`: it may not be valid UTF-8.
    let mut classmap_file: Vec<u8> = format!(
        "<?php\n\n// autoload_classmap.php @generated by Composer\n\n$vendorDir = {vendor_code};\n$baseDir = {base_code};\n\nreturn array(\n"
    )
    .into_bytes();
    let classmap_codes: Vec<(ClassName, PathCode)> = classmap
        .iter()
        .map(|(class, path)| (class.clone(), path_code(&base, &vendor, path)))
        .collect();
    for (class, code) in &classmap_codes {
        classmap_file.extend_from_slice(b"    ");
        classmap_file.extend_from_slice(&export_bytes(class));
        classmap_file.extend_from_slice(b" => ");
        // Straight into the buffer rather than through a second `format!`
        // just to wrap `code.plain()`'s own String: one allocation instead
        // of two for every classmap entry, of which a `-o` install on a
        // large project renders thousands (#302).
        classmap_file.extend_from_slice(code.plain().as_bytes());
        classmap_file.extend_from_slice(b",\n");
    }
    classmap_file.extend_from_slice(b");\n");

    // Files: same path spelled twice is a warning, keyed on the rendered code
    // as upstream does so `./foo.php` and `foo.php` count as duplicates.
    let files_codes: Vec<(String, PathCode)> = autoloads
        .files
        .iter()
        .map(|(id, path)| (id.clone(), path_code(&base, &vendor, path)))
        .collect();
    let mut seen = HashSet::new();
    let mut duplicates = Vec::new();
    for (_, code) in &files_codes {
        let rendered = code.plain();
        if !seen.insert(rendered.clone()) && !duplicates.contains(&rendered) {
            duplicates.push(rendered);
        }
    }
    if !duplicates.is_empty() {
        let mut warning = "The following \"files\" autoload rules are included multiple times, this may cause issues and should be resolved:".to_string();
        for duplicate in duplicates {
            let _ = write!(warning, "\n - {duplicate}");
        }
        out.warnings.push(warning);
    }
    let files_file = (!files_codes.is_empty()).then(|| {
        let mut file = format!(
            "<?php\n\n// autoload_files.php @generated by Composer\n\n$vendorDir = {vendor_code};\n$baseDir = {base_code};\n\nreturn array(\n"
        );
        for (id, code) in &files_codes {
            let _ = writeln!(file, "    {} => {},", export_str(id), code.plain());
        }
        file.push_str(");\n");
        file
    });

    // `AutoloadGenerator::getIncludePathsFile`: root then every package, in
    // install order (never dev-filtered) — see `include_path_entries`.
    let include_path_codes = include_path_entries(input, &base, &vendor);
    let include_paths_file = (!include_path_codes.is_empty()).then(|| {
        let mut file = format!(
            "<?php\n\n// include_paths.php @generated by Composer\n\n$vendorDir = {vendor_code};\n$baseDir = {base_code};\n\nreturn array(\n"
        );
        for code in &include_path_codes {
            let _ = writeln!(file, "    {},", code.plain());
        }
        file.push_str(");\n");
        file
    });

    let apcu_prefix = input.apcu_prefix.as_ref().map(|custom| {
        custom
            .clone()
            .or_else(|| existing_apcu_prefix(&input.vendor_dir))
            .unwrap_or_else(random_apcu_prefix)
    });

    let suffix = &input.suffix;
    let static_file = static_file(
        suffix,
        &target,
        &vendor,
        &base,
        &Maps {
            psr0: &psr0,
            psr4: &psr4,
            classmap: &classmap_codes,
            files: files_file.is_some().then_some(&files_codes),
        },
    );
    let real_file = real_file(
        suffix,
        target_dir_loader.as_deref(),
        include_paths_file.is_some(),
        files_file.is_some(),
        input.prepend_autoloader,
        input.platform_check,
        input.classmap_authoritative,
        apcu_prefix.as_deref(),
        input.use_include_path,
    );
    let autoload_file = autoload_file(&vendor_to_target_code, suffix);

    let mut write = |path: PathBuf, content: &[u8]| -> Result<()> {
        if std::fs::read(&path).is_ok_and(|existing| existing == content) {
            return Ok(());
        }
        fs_err::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
        out.files_written.push(path);
        Ok(())
    };
    write(
        target_dir.join("autoload_namespaces.php"),
        namespaces_file.as_bytes(),
    )?;
    write(target_dir.join("autoload_psr4.php"), psr4_file.as_bytes())?;
    write(target_dir.join("autoload_classmap.php"), &classmap_file)?;
    let files_path = target_dir.join("autoload_files.php");
    match files_file {
        Some(content) => write(files_path, content.as_bytes())?,
        None if files_path.exists() => fs_err::remove_file(&files_path)?,
        None => {}
    }
    let include_paths_path = target_dir.join("include_paths.php");
    match include_paths_file {
        Some(content) => write(include_paths_path, content.as_bytes())?,
        None if include_paths_path.exists() => fs_err::remove_file(&include_paths_path)?,
        None => {}
    }
    write(target_dir.join("autoload_static.php"), &static_file)?;
    write(
        input.vendor_dir.join("autoload.php"),
        autoload_file.as_bytes(),
    )?;
    write(target_dir.join("autoload_real.php"), real_file.as_bytes())?;
    write(
        target_dir.join("ClassLoader.php"),
        include_bytes!("templates/ClassLoader.php"),
    )?;
    write(
        target_dir.join("InstalledVersions.php"),
        include_bytes!("templates/InstalledVersions.php"),
    )?;
    write(
        target_dir.join("LICENSE"),
        include_bytes!("templates/LICENSE"),
    )?;
    tracing::debug!(
        classmap_entries = classmap_codes.len(),
        elapsed_ms = render_started.elapsed().as_millis(),
        "rendered and wrote autoload files"
    );

    out.classmap = classmap
        .into_iter()
        .map(|(class, path)| (class, PathBuf::from(path)))
        .collect();
    Ok(out)
}

/// `AutoloadGenerator::parseAutoloads`: filter dev packages, sort, then
/// collect each autoload type in the order Composer uses.
fn parse_autoloads(input: &Input, base: &str) -> Autoloads {
    // `AutoloadGenerator::dump` feeds this from `$localRepo->getCanonicalPackages()`,
    // whose order for a real `composer install` is the install transaction's
    // own dependency-first DFS (`Transaction::calculateOperations`), not
    // whatever order install/plan bookkeeping (a partial keep-vs-install
    // split, say) handed packages to the generator in. `PackageSorter`'s
    // cycle-breaking (first-visit-wins on a `requires` cycle, see
    // `sort_packages`) depends on this input order, not only on the
    // computed weights, so a dependency cycle sorts differently — a
    // different member becomes the first-visited, cycle-cancelled one —
    // unless this is re-derived here rather than trusted from the caller
    // (#259).
    let mut packages: Vec<&Package> = install_order(&input.packages.iter().collect::<Vec<_>>());
    if !input.dev_mode {
        if packages.iter().any(|p| p.is_dev) {
            packages.retain(|p| !p.is_dev);
        } else {
            packages = filter_package_map(&packages, &input.root);
        }
    }

    let mut sorted: Vec<Entry> = sort_by_dependency_weight(&packages)
        .into_iter()
        .map(|package| Entry {
            name: &package.name,
            autoload: object(&package.autoload),
            target_dir: package.target_dir.as_deref(),
            install_path: package.install_path.as_ref().map(|p| path_str(p)),
            is_root: false,
        })
        .collect();

    let mut root_autoload = object(&input.root.autoload);
    if input.dev_mode {
        merge_recursive(&mut root_autoload, &object(&input.root.autoload_dev));
    }
    sorted.push(Entry {
        name: &input.root.name,
        autoload: root_autoload,
        target_dir: input.root.target_dir.as_deref(),
        install_path: Some(String::new()),
        is_root: true,
    });
    let reversed: Vec<&Entry> = sorted.iter().rev().collect();
    let forward: Vec<&Entry> = sorted.iter().collect();

    let mut psr0 = BTreeMap::new();
    let mut psr4 = BTreeMap::new();
    let mut sink = Sink {
        base,
        classmap: Vec::new(),
        files: Vec::new(),
        exclude: Vec::new(),
    };
    collect_type(&reversed, "psr-0", &mut psr0, &mut sink);
    collect_type(&reversed, "psr-4", &mut psr4, &mut sink);
    collect_type(&reversed, "classmap", &mut BTreeMap::new(), &mut sink);
    collect_type(&forward, "files", &mut BTreeMap::new(), &mut sink);
    collect_type(
        &forward,
        "exclude-from-classmap",
        &mut BTreeMap::new(),
        &mut sink,
    );

    Autoloads {
        psr0: psr0.into_iter().rev().collect(),
        psr4: psr4.into_iter().rev().collect(),
        classmap: sink.classmap,
        files: sink.files,
        exclude: sink.exclude,
    }
}

/// The non-namespaced autoload lists being collected, plus the project dir
/// that stands in for Composer's cwd when resolving root-package rules.
struct Sink<'a> {
    base: &'a str,
    classmap: Vec<String>,
    files: Vec<(String, String)>,
    exclude: Vec<String>,
}

/// `AutoloadGenerator::parseAutoloadsType` for one type; the PSR maps go to
/// `namespaces`, the others to their list.
fn collect_type(
    entries: &[&Entry],
    kind: &str,
    namespaces: &mut BTreeMap<String, Vec<String>>,
    sink: &mut Sink,
) {
    for entry in entries {
        let Some(install_path) = entry.install_path.as_deref() else {
            continue;
        };
        let Some(rules) = entry.autoload.get(kind) else {
            continue;
        };
        let rules: Vec<(String, Vec<String>)> = match rules {
            Value::Object(map) => map
                .iter()
                .map(|(ns, paths)| (ns.clone(), strings(paths)))
                .collect(),
            Value::Array(items) => items
                .iter()
                .map(|paths| (String::new(), strings(paths)))
                .collect(),
            _ => continue,
        };
        // A package's install path includes its target-dir; autoload paths
        // are relative to the package root above it.
        let mut install_path = install_path.to_string();
        if let Some(target) = entry.target_dir.filter(|_| !entry.is_root) {
            let suffix = format!("/{target}");
            if install_path.ends_with(&suffix) {
                install_path.truncate(install_path.len() - suffix.len());
            }
        }

        for (namespace, paths) in rules {
            let namespace = namespace.trim_start_matches('\\').to_string();
            for mut path in paths {
                if matches!(kind, "files" | "classmap" | "exclude-from-classmap")
                    && let Some(target) = entry.target_dir.filter(|t| !t.is_empty())
                    && Path::new(&format!("{install_path}/{path}"))
                        .metadata()
                        .is_err()
                {
                    if entry.is_root {
                        path = strip_target_dir(&path, target);
                    } else {
                        path = format!("{target}/{path}");
                    }
                }

                if kind == "exclude-from-classmap" {
                    let resolve_from = if install_path.is_empty() {
                        sink.base
                    } else {
                        &install_path
                    };
                    if let Some(pattern) = exclusion_pattern(&path, resolve_from) {
                        sink.exclude.push(pattern);
                    }
                    continue;
                }

                let relative = if install_path.is_empty() {
                    if path.is_empty() {
                        ".".to_string()
                    } else {
                        path.clone()
                    }
                } else {
                    format!("{install_path}/{path}")
                };
                match kind {
                    "files" => {
                        let id = format!("{:x}", md5::compute(format!("{}:{path}", entry.name)));
                        match sink.files.iter_mut().find(|(existing, _)| *existing == id) {
                            Some(slot) => slot.1 = relative,
                            None => sink.files.push((id, relative)),
                        }
                    }
                    "classmap" => sink.classmap.push(relative),
                    _ => namespaces
                        .entry(namespace.clone())
                        .or_default()
                        .push(relative),
                }
            }
        }
    }
}

/// Root package with a target-dir: `Main/Foo/src` becomes `src`.
fn strip_target_dir(path: &str, target: &str) -> String {
    let path = path.trim_start_matches(['\\', '/']);
    let mut remaining = path;
    let mut ok = true;
    for part in target.split(['/', '\\']).filter(|p| !p.is_empty()) {
        if let Some(rest) = remaining
            .strip_prefix(part)
            .and_then(|r| r.strip_prefix(['/', '\\']))
        {
            remaining = rest;
        } else {
            ok = false;
            break;
        }
    }
    if ok { remaining } else { path }
        .trim_start_matches(['\\', '/'])
        .to_string()
}

/// One `exclude-from-classmap` rule as a regex fragment on the realpath of
/// the file. Wildcards: `**` matches across directories, `*` within one.
fn exclusion_pattern(path: &str, install_path: &str) -> Option<String> {
    // `install_path` is never empty here: the caller substitutes the project
    // dir for the root package, where Composer would use the cwd.
    let trimmed = path.replace('\\', "/");
    let trimmed = trimmed.trim_matches('/');
    let mut quoted = collapse_slashes(&regex::escape(trimmed));
    quoted = quoted.replace("\\*\\*", ".+?").replace("\\*", "[^/]+?");

    // Leading `./` and `../` segments are resolved against the install path
    // instead of being matched literally.
    let mut updir = String::new();
    while let Some(consumed) = ["\\./", "\\.\\./"]
        .iter()
        .find(|p| quoted.starts_with(*p))
        .map(|p| p.len())
    {
        updir.push_str(&quoted[..consumed].replace("\\.", "."));
        quoted = quoted[consumed..].to_string();
    }
    let resolved = fs_err::canonicalize(format!("{install_path}/{updir}")).ok()?;
    Some(format!(
        "{}/{quoted}($|/)",
        regex::escape(&path_str(&resolved))
    ))
}

fn collapse_slashes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '/' && out.ends_with('/') {
            continue;
        }
        out.push(c);
    }
    out
}

/// `AutoloadGenerator::filterPackageMap`: keep packages reachable from the
/// root's `require` links, following `replace`s to the replacing package.
fn filter_package_map<'a>(packages: &[&'a Package], root: &RootPackage) -> Vec<&'a Package> {
    let by_name: HashMap<&str, &Package> = packages.iter().map(|p| (p.name.as_str(), *p)).collect();
    let mut replaced_by: HashMap<&str, &str> = HashMap::new();
    for package in packages {
        for replaced in &package.replaces {
            replaced_by.insert(replaced, &package.name);
        }
    }

    let mut include: HashSet<&str> = HashSet::new();
    let mut stack: Vec<&str> = root.requires.iter().map(String::as_str).collect();
    while let Some(target) = stack.pop() {
        let target = replaced_by.get(target).copied().unwrap_or(target);
        if include.insert(target)
            && let Some(package) = by_name.get(target)
        {
            stack.extend(package.requires.iter().map(String::as_str));
        }
    }

    packages
        .iter()
        .copied()
        .filter(|p| {
            std::iter::once(&p.name)
                .chain(&p.replaces)
                .chain(&p.provides)
                .any(|name| include.contains(name.as_str()))
        })
        .collect()
}

/// Composer's `array_merge_recursive` as far as autoload sections need it:
/// nested maps merge, lists append, a scalar meeting another value becomes a
/// list of both (`Main => src/` plus `Main => tests/`).
fn merge_recursive(base: &mut Map<String, Value>, extra: &Map<String, Value>) {
    for (key, value) in extra {
        match base.get_mut(key) {
            None => {
                base.insert(key.clone(), value.clone());
            }
            Some(Value::Object(existing)) if value.is_object() => {
                merge_recursive(existing, value.as_object().expect("checked"));
            }
            Some(existing) => {
                let mut items = match existing.take() {
                    Value::Array(items) => items,
                    other => vec![other],
                };
                match value {
                    Value::Array(more) => items.extend(more.iter().cloned()),
                    other => items.push(other.clone()),
                }
                *existing = Value::Array(items);
            }
        }
    }
}

fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}

/// PHP's `(array) $paths`: a string or a list of strings.
fn strings(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(|v| v.as_str().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}

/// One classmap or PSR directory queued for a scan: `dir` as written for a
/// classmap entry (relative to the project) or, for a PSR rule, already an
/// absolute normalised directory; `psr` is `(namespace, "psr-4"|"psr-0")`.
/// Built up front by [`generate`] so [`Scanner::scan_all`] gets every
/// directory in the exact order the old serial loop would have scanned them
/// in.
struct ScanTask {
    dir: String,
    psr: Option<(String, String)>,
}

/// Which sidecar a [`PreparedScan`] reads/writes through, and the key it
/// scans under — `None` (in [`PreparedScan::cache`]) for a directory with no
/// store archive to key a cache on (root package with nothing in the store
/// yet, or a path/git source). The two variants are the two sidecar shapes
/// [`ScanOutcome`] can come back as: an archive hit is already the merge's
/// final shape (#303); a root hit is the older raw shape, still shaped and
/// PSR-filtered at merge time (see [`super::classmap::Sidecar`]'s doc for
/// why that one wasn't moved).
enum CacheSlot {
    Archive(PathBuf, ScanKey),
    Root(PathBuf, ScanKey),
}

/// A [`ScanTask`] once its cache has been checked: either the sidecar
/// already had the answer, or the directory still needs [`scan_paths`] to
/// walk it — the part `Scanner::scan_all` fans out across threads (#198).
enum ScanOutcome {
    /// An archive sidecar hit (#303): already grouped per file, in final
    /// order, PSR-4 filtered — `Scanner::scan_all`'s fold is a plain ordered
    /// append over this.
    HitShaped(Vec<(PathBuf, Vec<ClassName>)>),
    /// A root-sidecar hit: the raw scan result, shaped by
    /// `Scanner::shape_for_merge` exactly like a miss.
    HitRaw(ClassMap),
    Miss,
}

/// A [`ScanTask`] after `Scanner::scan_all`'s sequential setup pass: the
/// pieces its (possibly parallel) walk and its always-sequential fold each
/// need, computed once so neither has to re-derive them.
struct PreparedScan {
    abs_dir: String,
    psr: Option<(String, String)>,
    cache: Option<CacheSlot>,
    exclusion: Option<Regex>,
    outcome: ScanOutcome,
}

/// Class map scanning across all rules with Composer's "avoid duplicate
/// scans": a file that already yielded classes is not scanned again, so a
/// broader rule cannot report the same file as ambiguous with itself.
struct Scanner<'a> {
    base: &'a str,
    vendor: &'a str,
    excluded: &'a [String],
    archives: ArchiveIndex,
    /// #269: the root package's own classmap/PSR-4 sidecar, one file for the
    /// whole project (unlike `sidecars` below, one per store archive) —
    /// `None` when no package in this install came from the store, so there
    /// is no cache root to put it under (path/git-only project: falls back
    /// to always rescanning the root, same as before this cache existed).
    root_sidecar: Option<PathBuf>,
    map: BTreeMap<ClassName, String>,
    scanned: HashSet<PathBuf>,
    warnings: Vec<String>,
    /// Compiled exclusion regexes keyed by their joined pattern, so two
    /// directories that resolve to the same kept-pattern set (a common
    /// autoload with no vendor-dir overlap trimming) share one `Regex::new`
    /// instead of paying to compile it again per directory.
    regex_cache: HashMap<String, Regex>,
    /// #77/#269: the root package's own sidecar, parsed at most once per
    /// install however many distinct `ScanKey`s it's scanned under — the
    /// pre-#303 raw-scan shape (see [`Sidecar`]'s own doc for why the root
    /// package keeps it).
    sidecars: HashMap<PathBuf, Sidecar>,
    /// #303: a store archive's sidecar, one file per archive, already in the
    /// merge's own final shape — same one-parse-per-install rationale as
    /// `sidecars` above.
    archive_sidecars: HashMap<PathBuf, SidecarV1>,
    /// #54: whether the classmap-scan sidecar cache is actually paying off,
    /// and how much of `scan()`'s time is the filesystem walk/tokenizing
    /// (`scan_paths`) itself versus everything else in `scan()`.
    cache_hits: usize,
    cache_misses: usize,
    scan_paths_elapsed: std::time::Duration,
    /// #77: sidecar open/read/parse time on a cache hit, isolated from the
    /// per-file merge loop below it (both run for a hit; only the merge
    /// loop also runs for a miss).
    cache_read_elapsed: std::time::Duration,
    /// #77: time spent folding a scan's (cached or fresh) result into
    /// `self.map` — path normalizing, PSR filtering, ambiguity bookkeeping.
    merge_elapsed: std::time::Duration,
    /// #77 (measurement only): exclusion-regex build plus `ArchiveIndex`
    /// lookup and `ScanKey` construction, run once per `scan()` call
    /// regardless of hit/miss. #269: for a directory with no store archive,
    /// this also includes the fingerprint walk that stands in for one.
    setup_elapsed: std::time::Duration,
    /// #77 (measurement only): sidecar write time on a miss, isolated from
    /// `scan_paths` (the walk/tokenize) above it.
    cache_write_elapsed: std::time::Duration,
}

/// The store's own root, derived from any one package's `archive_dir`
/// (`<cache_root>/archive-v0/<hash>`) rather than threaded through [`Input`]:
/// every one of the dozens of call sites (mostly tests) that build an
/// [`Input`] literal would otherwise need a new field. `None` when nothing
/// in `packages` came from the store (a path/git-only project) — the root
/// package then simply gets no cache, same as before #269.
fn cache_root(packages: &[Package]) -> Option<PathBuf> {
    packages.iter().find_map(|p| {
        let archive_dir = p.archive_dir.as_ref()?;
        archive_dir.parent()?.parent().map(Path::to_path_buf)
    })
}

/// Maps an absolute, normalised scan directory back to the store archive dir
/// its package's install path was hardlinked from, plus the directory's own
/// offset from that install path — the two pieces [`ScanKey`] and
/// `store::archive_classmap_sidecar` need to cache a scan by content rather
/// than by (rebuildable) vendor path.
struct ArchiveIndex {
    /// `(install path, archive dir)`, longest install path first so a
    /// package nested under another's install path (target-dir) still
    /// resolves to its own entry rather than the outer one.
    entries: Vec<(String, PathBuf)>,
}

impl ArchiveIndex {
    fn build(packages: &[Package]) -> Self {
        let mut entries: Vec<(String, PathBuf)> = packages
            .iter()
            .filter_map(|p| {
                let install_path = p.install_path.as_ref()?;
                let archive_dir = p.archive_dir.as_ref()?;
                Some((normalize_path(&path_str(install_path)), archive_dir.clone()))
            })
            .collect();
        entries.sort_by_key(|(path, _)| std::cmp::Reverse(path.len()));
        ArchiveIndex { entries }
    }

    fn locate(&self, abs_dir: &str) -> Option<(&Path, String)> {
        self.entries.iter().find_map(|(install_path, archive_dir)| {
            if abs_dir == install_path {
                Some((archive_dir.as_path(), String::new()))
            } else {
                abs_dir
                    .strip_prefix(install_path.as_str())
                    .and_then(|rest| rest.strip_prefix('/'))
                    .map(|rest| (archive_dir.as_path(), rest.to_string()))
            }
        })
    }
}

impl Scanner<'_> {
    /// Run every queued [`ScanTask`] and fold the results into `self.map` in
    /// task order (#198). Setup (the exclusion regex, the sidecar cache
    /// lookup) touches `self` and stays a plain sequential pass; only the
    /// cache misses' [`scan_paths`] walk — independent per directory, and
    /// the dominant cost once the sidecar cache doesn't already have the
    /// answer — fans out across threads capped at the core count, the same
    /// `std::thread::scope` shape #193 used for archive extraction. The
    /// fold that follows runs last, over the tasks in their original order:
    /// which directory wins a duplicate class, and which file gets flagged
    /// ambiguous, comes out exactly as the old one-`scan()`-per-directory
    /// loop left it, whatever order the threads finished the walk in.
    fn scan_all(&mut self, tasks: &[ScanTask]) -> Result<()> {
        let mut prepared: Vec<PreparedScan> = Vec::with_capacity(tasks.len());
        for task in tasks {
            let psr = task
                .psr
                .as_ref()
                .map(|(ns, kind)| (ns.as_str(), kind.as_str()));
            let setup_started = std::time::Instant::now();
            let abs_dir = normalize_path(&if is_absolute(&task.dir) {
                task.dir.clone()
            } else {
                format!("{}/{}", self.base, task.dir)
            });

            let mut excluded: Vec<String> = self.excluded.to_vec();
            // A PSR directory containing the vendor dir must not pull vendor
            // classes into the root's namespace scan.
            if psr.is_some() && self.vendor.contains(&format!("{abs_dir}/")) {
                excluded.push(regex::escape(&format!("{}/", self.vendor)));
            }
            let exclusion = build_exclusion_regex(&abs_dir, &excluded, &mut self.regex_cache)?;

            // A directory hardlinked from the store (never the root package,
            // a path/git-source install, or one the store had no pointer
            // for) has an archive to key a cache on directly. Anything else
            // — chiefly the root package's own classmap/PSR-4 trees (#269)
            // — falls back to the project's one root sidecar, keyed on a
            // [`super::classmap::Fingerprint`] instead of an archive's
            // content hash, since it isn't immutable store content.
            let cache = self
                .archives
                .locate(&abs_dir)
                .map(|(archive_dir, subpath)| {
                    let key = ScanKey {
                        subpath,
                        exclude: exclusion.as_ref().map(|r| r.as_str().to_string()),
                        psr: task.psr.clone(),
                        fingerprint: None,
                    };
                    CacheSlot::Archive(crate::store::archive_classmap_sidecar(archive_dir), key)
                })
                .or_else(|| {
                    let sidecar = self.root_sidecar.clone()?;
                    let subpath = abs_dir
                        .strip_prefix(self.base)
                        .unwrap_or(&abs_dir)
                        .trim_start_matches('/')
                        .to_string();
                    let fingerprint = fingerprint_dir(Path::new(&abs_dir)).ok()?;
                    let key = ScanKey {
                        subpath,
                        exclude: exclusion.as_ref().map(|r| r.as_str().to_string()),
                        psr: task.psr.clone(),
                        fingerprint: Some(fingerprint),
                    };
                    Some(CacheSlot::Root(sidecar, key))
                });
            self.setup_elapsed += setup_started.elapsed();

            let read_started = std::time::Instant::now();
            // The sidecar itself is read (and parsed) at most once per
            // archive per install, however many distinct keys that archive
            // is scanned under — a `HashMap` entry, not a file read, on
            // every key after the first (#77).
            let outcome = match &cache {
                Some(CacheSlot::Archive(sidecar, key)) => self
                    .archive_sidecars
                    .entry(sidecar.clone())
                    .or_insert_with(|| SidecarV1::read(sidecar))
                    .get(key, Path::new(&abs_dir))
                    .map_or(ScanOutcome::Miss, ScanOutcome::HitShaped),
                Some(CacheSlot::Root(sidecar, key)) => self
                    .sidecars
                    .entry(sidecar.clone())
                    .or_insert_with(|| Sidecar::read(sidecar))
                    .get(key, Path::new(&abs_dir))
                    .map_or(ScanOutcome::Miss, ScanOutcome::HitRaw),
                None => ScanOutcome::Miss,
            };
            self.cache_read_elapsed += read_started.elapsed();

            if matches!(outcome, ScanOutcome::Miss) {
                self.cache_misses += 1;
            } else {
                self.cache_hits += 1;
            }
            prepared.push(PreparedScan {
                abs_dir,
                psr: task.psr.clone(),
                cache,
                exclusion,
                outcome,
            });
        }

        let miss_indices: Vec<usize> = prepared
            .iter()
            .enumerate()
            .filter(|(_, p)| matches!(p.outcome, ScanOutcome::Miss))
            .map(|(index, _)| index)
            .collect();
        // ponytail: stride scheduling (`worker`, `worker + workers`, ...) is
        // a fixed partition decided before any walk starts, so one worker
        // that draws the single largest directory in the batch sets the
        // floor for the whole parallel phase on its own — measured on
        // bench/laravel, where `laravel/framework` dominates, this is worth
        // roughly 1.4x rather than anywhere near the core count. Upgrade to
        // work-stealing (a shared `AtomicUsize` next-index, as
        // `install::link_archives` already does) or size-ordered scheduling
        // (largest directories dispatched first) if a profile ever shows
        // that tail actually dominating a real workload.
        let mut scanned: HashMap<usize, ClassMap> = if miss_indices.is_empty() {
            HashMap::new()
        } else {
            // `VIV_TEST_SCAN_WORKERS` (test-only, never documented for users)
            // pins the worker count so a test can compare one worker against
            // many without depending on the machine's own core count (#200):
            // production never sets it, same shape as `VIV_TEST_NOW`.
            let workers = std::env::var("VIV_TEST_SCAN_WORKERS")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or_else(|| {
                    std::thread::available_parallelism().map_or(1, std::num::NonZero::get)
                })
                .min(miss_indices.len())
                .max(1);
            // Tripped by the first directory that fails to walk, so the
            // other workers stop picking up new misses instead of grinding
            // through every remaining one.
            let stop = std::sync::atomic::AtomicBool::new(false);
            let scan_started = std::time::Instant::now();
            let mut results: Vec<(usize, Result<ClassMap>)> = std::thread::scope(|scope| {
                (0..workers)
                    .map(|worker| {
                        let miss_indices = &miss_indices;
                        let prepared = &prepared;
                        let stop = &stop;
                        scope.spawn(move || -> Vec<(usize, Result<ClassMap>)> {
                            let mut out = Vec::new();
                            let mut i = worker;
                            while i < miss_indices.len() {
                                if stop.load(std::sync::atomic::Ordering::Relaxed) {
                                    break;
                                }
                                let index = miss_indices[i];
                                let scan = &prepared[index];
                                let found =
                                    scan_paths(Path::new(&scan.abs_dir), scan.exclusion.as_ref());
                                if found.is_err() {
                                    stop.store(true, std::sync::atomic::Ordering::Relaxed);
                                }
                                out.push((index, found));
                                i += workers;
                            }
                            out
                        })
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .flat_map(|handle| handle.join().expect("classmap scan worker thread panicked"))
                    .collect()
            });
            self.scan_paths_elapsed += scan_started.elapsed();

            // `stop` can trip once a high-index worker fails while a
            // lower-index one, not yet scheduled onto a core, is still
            // sitting at its very first position and never gets to fill it
            // in — so a *missing* index is not necessarily the earliest
            // failure, and the fold below cannot tell "skipped because of
            // stop" from "not reached yet" apart just by what's absent.
            // Bail out here instead, picking the lowest-index error
            // directly out of `results`, before the fold ever runs: that
            // matches what the old serial loop's first `?` would have
            // returned, and once this returns, every miss index the fold
            // sees is guaranteed present and `Ok`.
            results.sort_by_key(|(index, _)| *index);
            if let Some(failed) = results.iter().position(|(_, result)| result.is_err()) {
                let (_, result) = results.swap_remove(failed);
                return Err(result.expect_err("position() only matches an Err result"));
            }
            results
                .into_iter()
                .map(|(index, result)| (index, result.expect("errors were returned above")))
                .collect()
        };

        // An upper bound on how many distinct files the fold below will
        // insert into `self.scanned`: every class plus every ambiguous
        // second-occurrence counts one file at most once each, so summing
        // their counts up front and reserving it in one shot avoids the
        // `HashSet` repeatedly doubling and rehashing everything it already
        // holds as a classmap-heavy `-o` install grows it past a few
        // thousand entries (#302).
        let scanned_upper_bound: usize = prepared
            .iter()
            .map(|scan| match &scan.outcome {
                ScanOutcome::HitShaped(shaped) => shaped.len(),
                ScanOutcome::HitRaw(found) => found.map.len() + found.ambiguous.len(),
                ScanOutcome::Miss => 0,
            })
            .sum::<usize>()
            + scanned
                .values()
                .map(|found| found.map.len() + found.ambiguous.len())
                .sum::<usize>();
        self.scanned.reserve(scanned_upper_bound);

        for (index, scan) in prepared.into_iter().enumerate() {
            let merge_started = std::time::Instant::now();
            let psr = scan
                .psr
                .as_ref()
                .map(|(ns, kind)| (ns.as_str(), kind.as_str()));
            let abs_dir = &scan.abs_dir;
            let cache = &scan.cache;

            // #303: an archive hit is already the merge's own final shape
            // (grouped per file, final order, PSR-4 filtered) — nothing left
            // to do but append it. A root hit or a miss is still the raw
            // per-class scan result, shaped here exactly as every scan used
            // to be before the sidecar started storing this shape directly.
            let for_merge: Vec<(PathBuf, PathBuf, Vec<ClassName>)> = match scan.outcome {
                ScanOutcome::HitShaped(shaped) => shaped
                    .into_iter()
                    .map(|(file, classes)| {
                        let real = file.clone();
                        (file, real, classes)
                    })
                    .collect(),
                ScanOutcome::HitRaw(found) => self.shape_for_merge(found, psr, abs_dir),
                ScanOutcome::Miss => {
                    let found = scanned
                        .remove(&index)
                        .expect("every miss index was scanned and any error already returned");
                    // Best-effort below: a failed write (read-only cache,
                    // permissions) must not fail the install that triggered
                    // it, only cost it a cache miss next time. Merges into
                    // whatever this archive's sidecar already held instead
                    // of overwriting it, so a different key already cached
                    // for the same archive doesn't get evicted (#77).
                    if let Some(CacheSlot::Root(sidecar, key)) = cache {
                        let write_started = std::time::Instant::now();
                        let _ = self
                            .sidecars
                            .entry(sidecar.clone())
                            .or_insert_with(|| Sidecar::read(sidecar))
                            .insert_and_write(sidecar, key, Path::new(abs_dir), &found);
                        self.cache_write_elapsed += write_started.elapsed();
                    }
                    let shaped = self.shape_for_merge(found, psr, abs_dir);
                    if let Some(CacheSlot::Archive(sidecar, key)) = cache {
                        let write_started = std::time::Instant::now();
                        // The exact bytes just folded into `self.map` below:
                        // #303's sidecar stores the merge's own final shape,
                        // not a raw scan, so a later hit has nothing left to
                        // recompute.
                        let shaped_for_write: Vec<(PathBuf, Vec<ClassName>)> = shaped
                            .iter()
                            .map(|(file, _, classes)| (file.clone(), classes.clone()))
                            .collect();
                        let _ = self
                            .archive_sidecars
                            .entry(sidecar.clone())
                            .or_insert_with(|| SidecarV1::read(sidecar))
                            .insert_and_write(sidecar, key, Path::new(abs_dir), &shaped_for_write);
                        self.cache_write_elapsed += write_started.elapsed();
                    }
                    shaped
                }
            };

            self.append_into_map(for_merge);
            self.merge_elapsed += merge_started.elapsed();
        }
        Ok(())
    }

    /// The regroup-by-file, PSR-4-filter fold every scan result — cached or
    /// freshly walked — went through before #303's archive sidecar started
    /// storing this shape directly: still needed for the root-package
    /// sidecar (hit or miss, see [`super::classmap::Sidecar`]'s doc for why)
    /// and for any miss (its result becomes the *next* archive sidecar entry
    /// once shaped, at the call site above). Classes a PSR-4 rule rejects
    /// are dropped and, for a file outside the vendor dir only (the root
    /// package's own — `self.vendor`), warned about, exactly as before
    /// #303.
    fn shape_for_merge(
        &mut self,
        found: ClassMap,
        psr: Option<(&str, &str)>,
        abs_dir: &str,
    ) -> Vec<(PathBuf, PathBuf, Vec<ClassName>)> {
        // `found` is owned here, so its map/ambiguous entries can be moved
        // into `per_file` instead of cloned — a class name and a path clone
        // each avoided per entry, which adds up over a package's whole
        // classmap.
        let ClassMap {
            map,
            ambiguous,
            mut canonical,
        } = found;
        let mut per_file: BTreeMap<PathBuf, Vec<ClassName>> = BTreeMap::new();
        for (class, path) in map {
            per_file.entry(path).or_default().push(class);
        }
        for (class, _, other) in ambiguous {
            per_file.entry(other).or_default().push(class);
        }

        let mut out = Vec::with_capacity(per_file.len());
        for (file, classes) in per_file {
            // `scan_paths` already canonicalized this file to dedupe
            // symlinked duplicates; reuse it instead of doing so again.
            // `remove` rather than `get().cloned()`: each file is only ever
            // looked up once per task, so taking ownership skips a
            // `PathBuf` clone for every one of them.
            let real = canonical.remove(&file).unwrap_or_else(|| file.clone());
            let file_path = normalized_path_str(&file);
            let classes = match psr {
                Some((namespace, kind)) => {
                    let (valid, rejected) =
                        filter_by_namespace(&classes, &file_path, namespace, kind, abs_dir);
                    if valid.is_empty() {
                        if !file_path.starts_with(self.vendor) {
                            let short = |p: &str| {
                                p.strip_prefix(self.base)
                                    .map_or_else(|| p.to_string(), |r| format!(".{r}"))
                            };
                            for class in rejected {
                                self.warnings.push(format!(
                                    "Class {} located in {} does not comply with {kind} autoloading standard (rule: {namespace} => {}). Skipping.",
                                    lossy(&class),
                                    short(&file_path),
                                    short(abs_dir)
                                ));
                            }
                        }
                        continue;
                    }
                    valid
                }
                None => classes,
            };
            out.push((file, real, classes));
        }
        out
    }

    /// The tail every scan's `(file, real, classes)` triples fold through,
    /// in order: skip a file some earlier task already claimed (`real`, the
    /// canonicalized dedup key — always `file` itself for an archive hit,
    /// see [`ScanOutcome::HitShaped`]'s own construction above), otherwise
    /// record it and resolve each class against `self.map`, first writer
    /// wins, same ambiguity warning as always. `entries`' own order (the
    /// "final lexicographic order" #198/#259 depend on) decides every tie.
    fn append_into_map(&mut self, entries: Vec<(PathBuf, PathBuf, Vec<ClassName>)>) {
        for (file, real, classes) in entries {
            if self.scanned.contains(&real) {
                continue;
            }
            let file_path = normalized_path_str(&file);
            self.scanned.insert(real);
            for class in classes {
                match self.map.get(&class) {
                    None => {
                        self.map.insert(class, file_path.clone());
                    }
                    Some(existing) if *existing != file_path => self.warnings.push(format!(
                        "Warning: Ambiguous class resolution, \"{}\" was found in both \"{existing}\" and \"{file_path}\", the first will be used.",
                        lossy(&class)
                    )),
                    Some(_) => {}
                }
            }
        }
    }
}

/// `ClassMapGenerator::filterByNamespace`: keep the classes whose PSR path
/// matches the file that declares them.
fn filter_by_namespace(
    classes: &[ClassName],
    file_path: &str,
    namespace: &str,
    kind: &str,
    base_path: &str,
) -> (Vec<ClassName>, Vec<ClassName>) {
    let sub_path = file_path.get(base_path.len() + 1..).unwrap_or("");
    let real_sub_path = sub_path
        .rfind('.')
        .map_or(sub_path, |dot| &sub_path[..dot])
        .as_bytes();

    let mut valid = Vec::new();
    let mut rejected = Vec::new();
    for class in classes {
        let expected: Vec<u8> = if kind == "psr-0" {
            if !namespace.is_empty() && !class.starts_with(namespace.as_bytes()) {
                rejected.push(class.clone());
                continue;
            }
            match class.iter().rposition(|&b| b == b'\\') {
                Some(pos) => {
                    let mut out = replace_byte(&class[..=pos], b'\\', b'/');
                    out.extend(replace_byte(&class[pos + 1..], b'_', b'/'));
                    out
                }
                None => replace_byte(class, b'_', b'/'),
            }
        } else {
            let rest = class.get(namespace.len()..).unwrap_or(&[]);
            replace_byte(rest, b'\\', b'/')
        };
        if expected == real_sub_path {
            valid.push(class.clone());
        } else {
            rejected.push(class.clone());
        }
    }
    (valid, rejected)
}

fn replace_byte(bytes: &[u8], from: u8, to: u8) -> Vec<u8> {
    bytes
        .iter()
        .map(|&b| if b == from { to } else { b })
        .collect()
}

/// Human-readable class name for warnings: bytes outside UTF-8 (only
/// possible for a classmap entry) are decoded lossily since warnings are
/// plain `String`s, not the bytes Composer writes into the classmap files.
fn lossy(class: &[u8]) -> std::borrow::Cow<'_, str> {
    String::from_utf8_lossy(class)
}

/// `AutoloadGenerator::buildExclusionRegex`: only the patterns that share a
/// prefix with the scanned directory are compiled in. `cache` keeps one
/// compiled `Regex` per distinct joined pattern across every directory this
/// install scans, rather than recompiling the same pattern set once per
/// directory.
fn build_exclusion_regex(
    dir: &str,
    excluded: &[String],
    cache: &mut HashMap<String, Regex>,
) -> Result<Option<Regex>> {
    if excluded.is_empty() {
        return Ok(None);
    }
    let mut kept: Vec<&str> = excluded.iter().map(String::as_str).collect();
    if let Ok(real) = fs_err::canonicalize(dir) {
        let dir_match = regex::escape(&path_str(&real));
        let dir_normalized = regex::escape(&normalize_path(dir));
        let is_symlink = dir_match != dir_normalized;
        kept.retain(|pattern| {
            let prefix = literal_prefix(pattern);
            let related = |d: &str| prefix.starts_with(d) || d.starts_with(prefix);
            related(&dir_match) || (is_symlink && related(&dir_normalized))
        });
    }
    if kept.is_empty() {
        return Ok(None);
    }
    let pattern = format!("({})", kept.join("|"));
    if let Some(compiled) = cache.get(&pattern) {
        return Ok(Some(compiled.clone()));
    }
    let compiled = Regex::new(&pattern)?;
    cache.insert(pattern, compiled.clone());
    Ok(Some(compiled))
}

/// The leading part of a regex that is plain (possibly escaped) text.
fn literal_prefix(pattern: &str) -> &str {
    let bytes = pattern.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if i + 1 < bytes.len() => i += 2,
            b'.' | b'+' | b'*' | b'?' | b'[' | b'^' | b']' | b'$' | b'(' | b')' | b'{' | b'}'
            | b'=' | b'!' | b'<' | b'>' | b'|' | b':' | b'\\' | b'#' | b'-' => break,
            _ => i += 1,
        }
    }
    &pattern[..i]
}

/// `AutoloadGenerator::getIncludePathsFile`: root then every package, each
/// `include-path` entry resolved against its install path.
///
/// Composer builds this from `$localRepo->getCanonicalPackages()`, never
/// resorted by `sortPackageMap` — but that repo's own order, for a real
/// install, is whatever order the install transaction added packages in,
/// which is `install_order`'s dependency-first DFS
/// (`Transaction::calculateOperations`), not lock/alphabetical order. Unlike
/// `parse_autoloads`, this list is never dev-filtered, matching
/// `getIncludePathsFile`: its packages already exclude dev-only ones (a
/// `--no-dev` install never installs them).
fn include_path_entries(input: &Input, base: &str, vendor: &str) -> Vec<PathCode> {
    let mut codes = Vec::new();
    let mut collect = |install_path: Option<String>, target_dir: Option<&str>, paths: &[String]| {
        let Some(mut install_path) = install_path else {
            return;
        };
        if let Some(target) = target_dir.filter(|t| !t.is_empty()) {
            let suffix = format!("/{target}");
            if install_path.ends_with(&suffix) {
                install_path.truncate(install_path.len() - suffix.len());
            }
        }
        for raw in paths {
            let trimmed = raw.trim_matches('/');
            let relative = if install_path.is_empty() {
                trimmed.to_string()
            } else {
                format!("{install_path}/{trimmed}")
            };
            codes.push(path_code(base, vendor, &relative));
        }
    };
    collect(
        Some(String::new()),
        input.root.target_dir.as_deref(),
        &input.root.include_path,
    );
    let all: Vec<&Package> = input.packages.iter().collect();
    for package in install_order(&all) {
        collect(
            package.install_path.as_ref().map(|p| path_str(p)),
            package.target_dir.as_deref(),
            &package.include_path,
        );
    }
    codes
}

/// The `name`/`require`/`provide`/`replace` edges [`install_order`]'s DFS
/// walks: implemented here for this module's own [`Package`] and in
/// `plugins/mod.rs` for `crate::lock::Package`, so the DFS itself — ported
/// from `Transaction::calculateOperations` — exists exactly once for both
/// shapes (#130).
pub(crate) trait Requires {
    fn install_order_name(&self) -> &str;
    fn install_order_requires(&self) -> impl Iterator<Item = &str>;
    /// Every other name this package satisfies a requirement under
    /// (`provide`/`replace`).
    fn install_order_provides(&self) -> impl Iterator<Item = &str>;
    /// `Package::getType() === 'composer-plugin' || 'composer-installer'`
    /// (`Transaction::movePluginsToFront`). Defaults to `false`: `plugins/mod.rs`'s
    /// `crate::lock::Package` use of the shared DFS has no plugin metadata to
    /// check, so it never reorders for one.
    fn install_order_is_plugin(&self) -> bool {
        false
    }
    /// A `composer-plugin` with `extra.plugin-modifies-downloads: true`:
    /// `movePluginsToFront`'s separate front-of-front bucket. Same default as
    /// [`Self::install_order_is_plugin`], for the same reason.
    fn install_order_modifies_downloads(&self) -> bool {
        false
    }
}

impl Requires for Package {
    fn install_order_name(&self) -> &str {
        &self.name
    }

    fn install_order_requires(&self) -> impl Iterator<Item = &str> {
        self.requires.iter().map(String::as_str)
    }

    fn install_order_provides(&self) -> impl Iterator<Item = &str> {
        self.provides
            .iter()
            .chain(&self.replaces)
            .map(String::as_str)
    }

    fn install_order_is_plugin(&self) -> bool {
        self.is_plugin
    }

    fn install_order_modifies_downloads(&self) -> bool {
        self.modifies_downloads
    }
}

/// `Transaction::calculateOperations` plus `movePluginsToFront`, ported as a
/// non-recursive stack to match exactly (including a quirk a recursive
/// postorder DFS can't reproduce, see below): every result package is a
/// fresh install here (there's no "present" repository to diff against), so
/// every operation `calculateOperations` would emit is an `InstallOperation`,
/// in the order the stack first pops each package a second time.
///
/// `getRootPackages`, ported almost verbatim: a package already excluded from
/// the root set is skipped *before* it gets to exclude whatever it requires
/// (`if (!isset($roots[$hash])) continue;`) — an order-dependent quirk that
/// is exactly what lets a `requires` cycle still surface one of its members
/// as a root, in descending-name order (`Transaction::setResultPackageMaps`'s
/// `uasort`), which is also why the seed stack below pops in *ascending*
/// name order (`array_pop` off the end of that descending array).
pub(crate) fn install_order<'a, P: Requires>(packages: &[&'a P]) -> Vec<&'a P> {
    let mut by_name: HashMap<&str, &'a P> = HashMap::new();
    for package in packages {
        for name in
            std::iter::once(package.install_order_name()).chain(package.install_order_provides())
        {
            by_name.entry(name).or_insert(package);
        }
    }

    let mut sorted: Vec<&'a P> = packages.to_vec();
    sorted.sort_by(|a, b| b.install_order_name().cmp(a.install_order_name()));

    let mut is_root: HashMap<&str, bool> = sorted
        .iter()
        .map(|p| (p.install_order_name(), true))
        .collect();
    for package in &sorted {
        let name = package.install_order_name();
        if !is_root[name] {
            continue;
        }
        for requirement in package.install_order_requires() {
            if let Some(&dep) = by_name.get(requirement) {
                let dep_name = dep.install_order_name();
                if dep_name != name {
                    is_root.insert(dep_name, false);
                }
            }
        }
    }
    let mut stack: Vec<&'a P> = sorted
        .iter()
        .copied()
        .filter(|p| is_root[p.install_order_name()])
        .collect();

    let mut visited: HashSet<&str> = HashSet::new();
    let mut processed: HashSet<&str> = HashSet::new();
    let mut order: Vec<&'a P> = Vec::with_capacity(packages.len());

    // `calculateOperations`'s own explicit stack: a package popped for the
    // first time is pushed back behind its own (still unvisited-or-not)
    // requires; popped again while unprocessed, it is emitted. Pushing a
    // require regardless of whether it's already visited is what lets a
    // cycle's ancestor come back around and get emitted early.
    while let Some(package) = stack.pop() {
        let name = package.install_order_name();
        if processed.contains(name) {
            continue;
        }
        if visited.insert(name) {
            stack.push(package);
            for requirement in package.install_order_requires() {
                if let Some(&dep) = by_name.get(requirement) {
                    stack.push(dep);
                }
            }
        } else if processed.insert(name) {
            order.push(package);
        }
    }
    // A require cycle with no true root at all (every member unrooted before
    // its own turn) would otherwise drop packages; Composer's own stack
    // never loses one, only reorders it. This fallback has no real Composer
    // behaviour to match — it's a safety net, not a ported quirk.
    for package in packages {
        visit(*package, &by_name, &mut processed, &mut order);
    }

    move_plugins_to_front(order)
}

/// [`install_order`]'s no-true-root fallback: a package's own postorder
/// visit, pushing its still-unvisited `requires` first, last-declared first.
fn visit<'a, P: Requires>(
    package: &'a P,
    by_name: &HashMap<&str, &'a P>,
    visited: &mut HashSet<&'a str>,
    order: &mut Vec<&'a P>,
) {
    if !visited.insert(package.install_order_name()) {
        return;
    }
    let requires: Vec<&str> = package.install_order_requires().collect();
    for requirement in requires.into_iter().rev() {
        if let Some(&dep) = by_name.get(requirement) {
            visit(dep, by_name, visited, order);
        }
    }
    order.push(package);
}

/// `Transaction::movePluginsToFront`: on this module's always-fresh-install
/// modelling, every entry in `operations` is what would be an
/// `InstallOperation`, so this reorders [`install_order`]'s own output
/// in place of a separate operations list. Scans in reverse so a plugin's
/// own (still-to-come, earlier-installed) requires are seen *after* the
/// plugin itself, letting their names already be in `plugin_requires` by the
/// time they're checked.
fn move_plugins_to_front<'a, P: Requires>(operations: Vec<&'a P>) -> Vec<&'a P> {
    let mut dl_modifying_no_deps: Vec<&'a P> = Vec::new();
    let mut dl_modifying_with_deps: Vec<&'a P> = Vec::new();
    let mut dl_modifying_requires: HashSet<&'a str> = HashSet::new();
    let mut plugins_no_deps: Vec<&'a P> = Vec::new();
    let mut plugins_with_deps: Vec<&'a P> = Vec::new();
    let mut plugin_requires: HashSet<&'a str> = HashSet::new();
    let mut rest: Vec<&'a P> = Vec::new();

    for package in operations.into_iter().rev() {
        let names: Vec<&str> = std::iter::once(package.install_order_name())
            .chain(package.install_order_provides())
            .collect();
        let non_platform_requires = || {
            package
                .install_order_requires()
                .filter(|r| !crate::repository::is_platform_package(r))
        };

        let is_downloads_modifying = package.install_order_modifies_downloads();
        if is_downloads_modifying || names.iter().any(|n| dl_modifying_requires.contains(n)) {
            let requires: Vec<&str> = non_platform_requires().collect();
            if is_downloads_modifying && requires.is_empty() {
                dl_modifying_no_deps.push(package);
            } else {
                dl_modifying_requires.extend(requires);
                dl_modifying_with_deps.push(package);
            }
            continue;
        }

        let is_plugin = package.install_order_is_plugin();
        if is_plugin || names.iter().any(|n| plugin_requires.contains(n)) {
            let requires: Vec<&str> = non_platform_requires().collect();
            if is_plugin && requires.is_empty() {
                plugins_no_deps.push(package);
            } else {
                plugin_requires.extend(requires);
                plugins_with_deps.push(package);
            }
            continue;
        }

        rest.push(package);
    }

    for bucket in [
        &mut dl_modifying_no_deps,
        &mut dl_modifying_with_deps,
        &mut plugins_no_deps,
        &mut plugins_with_deps,
        &mut rest,
    ] {
        bucket.reverse();
    }

    dl_modifying_no_deps
        .into_iter()
        .chain(dl_modifying_with_deps)
        .chain(plugins_no_deps)
        .chain(plugins_with_deps)
        .chain(rest)
        .collect()
}

/// `PackageSorter::sortPackages` with no weight overrides, over borrowed
/// packages: dependency weight ascending (leaves first, ties broken
/// alphabetically), the order Composer's own dependency-first sorts use.
fn sort_by_dependency_weight<'a>(packages: &[&'a Package]) -> Vec<&'a Package> {
    // `sort_packages` wants borrowed slices, so the `&str` lists need a home.
    let requires_storage: Vec<Vec<&str>> = packages
        .iter()
        .map(|p| p.requires.iter().map(String::as_str).collect())
        .collect();
    let requires: Vec<(&str, &[&str])> = packages
        .iter()
        .zip(&requires_storage)
        .map(|(package, reqs)| (package.name.as_str(), reqs.as_slice()))
        .collect();
    let by_name: HashMap<&str, &Package> = packages.iter().map(|p| (p.name.as_str(), *p)).collect();
    sort_packages(&requires, &[])
        .into_iter()
        .map(|name| by_name[name])
        .collect()
}

fn apcu_prefix_regex() -> &'static Regex {
    static PREFIX_RE: OnceLock<Regex> = OnceLock::new();
    PREFIX_RE.get_or_init(|| Regex::new(r"setApcuPrefix\('([^']+)'\)").expect("valid regex"))
}

/// The prefix already in `vendor/composer/autoload_real.php`
/// (`setApcuPrefix('...')`), reused so a dump does not orphan the warm
/// `APCu` cache — the prefix keys every entry in it. Mirrors
/// `resolve_suffix`'s read-the-existing-file-first step in `src/install.rs`.
fn existing_apcu_prefix(vendor_dir: &Path) -> Option<String> {
    let existing = fs_err::read_to_string(vendor_dir.join("composer/autoload_real.php")).ok()?;
    apcu_prefix_regex()
        .captures(&existing)
        .map(|caps| caps[1].to_string())
}

/// Composer's `bin2hex(random_bytes(10))`: generated only when
/// `--apcu-autoloader` is set without an explicit prefix and there is no
/// existing autoloader to read one from (`existing_apcu_prefix`); once
/// generated, it is reused on every later dump.
fn random_apcu_prefix() -> String {
    let mut bytes = [0u8; 10];
    std::io::Read::read_exact(
        &mut fs_err::File::open("/dev/urandom").expect("/dev/urandom is available"),
        &mut bytes,
    )
    .expect("read /dev/urandom");
    bytes.iter().fold(String::new(), |mut hex, b| {
        let _ = write!(hex, "{b:02x}");
        hex
    })
}

/// `AutoloadGenerator::getPathCode`.
fn path_code(base: &str, vendor: &str, path: &str) -> PathCode {
    let path = normalize_path(&if is_absolute(path) {
        path.to_string()
    } else {
        format!("{base}/{path}")
    });
    // Same test as `format!("{path}/").starts_with(&format!("{vendor}/"))`
    // (`path` under `vendor`, or equal to it) without allocating two strings
    // just to compare them — this runs once per rendered classmap/PSR entry.
    let under_vendor = path
        .strip_prefix(vendor)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'));
    let (prefix, rest) = if under_vendor {
        (Prefix::Vendor, path[vendor.len()..].to_string())
    } else {
        let relative = normalize_path(&find_shortest_path(base, &path, true));
        if is_absolute(&relative) {
            (Prefix::Absolute, relative)
        } else {
            (Prefix::Base, format!("/{relative}"))
        }
    };
    let phar = phar_regex().is_match(&rest);
    PathCode { prefix, phar, rest }
}

fn phar_regex() -> &'static Regex {
    static PHAR: OnceLock<Regex> = OnceLock::new();
    PHAR.get_or_init(|| Regex::new(r"\.phar([\\/]|$)").expect("valid regex"))
}

fn autoload_file(vendor_to_target_code: &str, suffix: &str) -> String {
    let require = if let Some(open) = vendor_to_target_code.strip_suffix('\'') {
        format!("{open}/autoload_real.php'")
    } else {
        format!("{vendor_to_target_code} . '/autoload_real.php'")
    };
    format!(
        "<?php

// autoload.php @generated by Composer

if (PHP_VERSION_ID < 50600) {{
    if (!headers_sent()) {{
        header('HTTP/1.1 500 Internal Server Error');
    }}
    $err = 'Composer 2.3.0 dropped support for autoloading on PHP <5.6 and you are running '.PHP_VERSION.', please upgrade PHP or use Composer 2.2 LTS via \"composer self-update --2.2\". Aborting.'.PHP_EOL;
    if (!ini_get('display_errors')) {{
        if (PHP_SAPI === 'cli' || PHP_SAPI === 'phpdbg') {{
            fwrite(STDERR, $err);
        }} elseif (!headers_sent()) {{
            echo $err;
        }}
    }}
    throw new RuntimeException($err);
}}

require_once {require};

return ComposerAutoloaderInit{suffix}::getLoader();
"
    )
}

#[expect(
    clippy::fn_params_excessive_bools,
    clippy::too_many_arguments,
    reason = "mirrors getAutoloadRealFile's flags"
)]
fn real_file(
    suffix: &str,
    target_dir_loader: Option<&str>,
    use_include_path_file: bool,
    use_files: bool,
    prepend: bool,
    check_platform: bool,
    classmap_authoritative: bool,
    apcu_prefix: Option<&str>,
    use_global_include_path: bool,
) -> String {
    let prepend = if prepend { "true" } else { "false" };
    let mut file = format!(
        "<?php

// autoload_real.php @generated by Composer

class ComposerAutoloaderInit{suffix}
{{
    private static $loader;

    public static function loadClassLoader($class)
    {{
        if ('Composer\\Autoload\\ClassLoader' === $class) {{
            require __DIR__ . '/ClassLoader.php';
        }}
    }}

    /**
     * @return \\Composer\\Autoload\\ClassLoader
     */
    public static function getLoader()
    {{
        if (null !== self::$loader) {{
            return self::$loader;
        }}

"
    );
    if check_platform {
        file.push_str("        require __DIR__ . '/platform_check.php';\n\n");
    }
    let _ = write!(
        file,
        "        spl_autoload_register(array('ComposerAutoloaderInit{suffix}', 'loadClassLoader'), true, {prepend});
        self::$loader = $loader = new \\Composer\\Autoload\\ClassLoader(\\dirname(__DIR__));
        spl_autoload_unregister(array('ComposerAutoloaderInit{suffix}', 'loadClassLoader'));

"
    );
    if use_include_path_file {
        file.push_str(
            "        $includePaths = require __DIR__ . '/include_paths.php';
        $includePaths[] = get_include_path();
        set_include_path(implode(PATH_SEPARATOR, $includePaths));

",
        );
    }
    let _ = write!(
        file,
        "        require __DIR__ . '/autoload_static.php';
        call_user_func(\\Composer\\Autoload\\ComposerStaticInit{suffix}::getInitializer($loader));

"
    );
    if classmap_authoritative {
        file.push_str("        $loader->setClassMapAuthoritative(true);\n");
    }
    if let Some(prefix) = apcu_prefix {
        let _ = writeln!(
            file,
            "        $loader->setApcuPrefix({});",
            export_str(prefix)
        );
    }
    if use_global_include_path {
        file.push_str("        $loader->setUseIncludePath(true);\n");
    }
    if target_dir_loader.is_some() {
        let _ = write!(
            file,
            "        spl_autoload_register(array('ComposerAutoloaderInit{suffix}', 'autoload'), true, true);\n\n"
        );
    }
    let _ = write!(file, "        $loader->register({prepend});\n\n");
    if use_files {
        let _ = write!(
            file,
            "        $filesToLoad = \\Composer\\Autoload\\ComposerStaticInit{suffix}::$files;
        $requireFile = \\Closure::bind(static function ($fileIdentifier, $file) {{
            if (empty($GLOBALS['__composer_autoload_files'][$fileIdentifier])) {{
                $GLOBALS['__composer_autoload_files'][$fileIdentifier] = true;

                require $file;
            }}
        }}, null, null);
        foreach ($filesToLoad as $fileIdentifier => $file) {{
            $requireFile($fileIdentifier, $file);
        }}

"
        );
    }
    file.push_str("        return $loader;\n    }\n");
    file.push_str(target_dir_loader.unwrap_or(""));
    file.push_str("}\n");
    file
}

/// The maps `autoload_static.php` is built from, as spelled in the plain files.
struct Maps<'a> {
    psr0: &'a [(String, Vec<PathCode>)],
    psr4: &'a [(String, Vec<PathCode>)],
    classmap: &'a [(ClassName, PathCode)],
    /// `None` when there is no `autoload_files.php`.
    files: Option<&'a [(String, PathCode)]>,
}

fn static_file(suffix: &str, target: &str, vendor: &str, base: &str, maps: &Maps) -> Vec<u8> {
    let vendor_code = find_shortest_path_code(target, vendor, true, true);
    let base_code = find_shortest_path_code(target, base, true, true);
    // Composer runs `strtr` over `var_export` output with these four keys;
    // `strtr` takes the longest matching key and a later duplicate key wins.
    let replacements = [
        (
            format!("{}/", vendor.trim_end_matches('/')),
            format!("{vendor_code} . '/"),
        ),
        (
            format!("phar://{}/", vendor.trim_end_matches('/')),
            format!("'phar://' . {vendor_code} . '/"),
        ),
        (
            format!("{}/", base.trim_end_matches('/')),
            format!("{base_code} . '/"),
        ),
        (
            format!("phar://{}/", base.trim_end_matches('/')),
            format!("'phar://' . {base_code} . '/"),
        ),
    ];
    let code = |path: &PathCode| -> Php {
        let value = path.evaluated(vendor, base);
        let best = replacements
            .iter()
            .filter(|(key, _)| value.starts_with(key.as_str()))
            .max_by_key(|(key, _)| key.len());
        Php::Code(match best {
            Some((key, replacement)) => {
                let exported = export_str(&value[key.len()..]);
                format!("{replacement}{}", &exported[1..])
            }
            None => export_str(&value),
        })
    };
    let to_php = |rows: &[(String, Vec<PathCode>)]| -> Vec<(String, Vec<Php>)> {
        rows.iter()
            .map(|(ns, paths)| (ns.clone(), paths.iter().map(code).collect()))
            .collect()
    };

    let mut props: Vec<(&str, Php)> = Vec::new();
    if let Some(files) = maps.files {
        props.push((
            "files",
            Php::Arr(
                files
                    .iter()
                    .map(|(id, path)| (Key::Str(id.clone()), code(path)))
                    .collect(),
            ),
        ));
    }
    props.extend(loader_properties(&to_php(maps.psr0), &to_php(maps.psr4)));
    if !maps.classmap.is_empty() {
        props.push((
            "classMap",
            Php::Arr(
                maps.classmap
                    .iter()
                    .map(|(class, path)| (Key::Bytes(class.clone()), code(path)))
                    .collect(),
            ),
        ));
    }

    // Bytes rather than a `String`: `classMap`'s keys are raw class name
    // bytes and may not be valid UTF-8.
    let mut file: Vec<u8> = format!(
        "<?php\n\n// autoload_static.php @generated by Composer\n\nnamespace Composer\\Autoload;\n\nclass ComposerStaticInit{suffix}\n{{\n"
    )
    .into_bytes();
    let mut initializer = String::new();
    for (name, value) in props {
        file.extend_from_slice(format!("    public static ${name} = ").as_bytes());
        file.extend_from_slice(&export_static(&value));
        file.extend_from_slice(b";\n\n");
        if name != "files" {
            let _ = writeln!(
                initializer,
                "            $loader->{name} = ComposerStaticInit{suffix}::${name};"
            );
        }
    }
    file.extend_from_slice(
        format!(
            "    public static function getInitializer(ClassLoader $loader)
    {{
        return \\Closure::bind(function () use ($loader) {{
{initializer}
        }}, null, ClassLoader::class);
    }}
}}
"
        )
        .as_bytes(),
    );
    file
}

pub(crate) fn path_str(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `normalize_path(&path_str(path))` without the extra allocation
/// `path_str` costs on the way there: every path this tool writes is valid
/// UTF-8, so `Path::to_str` borrows it for free where `path_str`'s
/// `to_string_lossy().into_owned()` always clones, lossy or not. Only the
/// (practically unreachable) non-UTF-8 case pays for both. Called once per
/// scanned classmap file on a `-o` install (#302).
fn normalized_path_str(path: &Path) -> String {
    match path.to_str() {
        Some(s) => normalize_path(s),
        None => normalize_path(&path_str(path)),
    }
}

fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
}

/// `Filesystem::normalizePath` without the Windows drive and UNC handling:
/// collapse `//`, resolve `.` and `..`, drop the trailing slash.
pub(crate) fn normalize_path(path: &str) -> String {
    // Every scanned/cached classmap path already comes out of a prior
    // `normalize_path` (an `abs_dir` joined with a clean relative subpath),
    // so it typically has nothing left to collapse. Detect that case without
    // building the `Vec<&str>` of parts below: this function runs once per
    // classmap file and once per rendered path entry, thousands of times on
    // a `-o` install.
    let already_clean = !path.is_empty()
        && !path.contains('\\')
        && !path.contains("//")
        && !path.split('/').any(|part| part == "." || part == "..")
        && (path == "/" || !path.ends_with('/'));
    if already_clean {
        return path.to_string();
    }
    let path = path.replace('\\', "/");
    let (absolute, rest) = match path.strip_prefix('/') {
        Some(rest) => ("/", rest),
        None => ("", path.as_str()),
    };
    let mut parts: Vec<&str> = Vec::new();
    let mut up = false;
    for chunk in rest.split('/') {
        if chunk == ".." && (!absolute.is_empty() || up) {
            parts.pop();
            up = !(parts.is_empty() || parts.last() == Some(&".."));
        } else if chunk != "." && !chunk.is_empty() {
            parts.push(chunk);
            up = chunk != "..";
        }
    }
    format!("{absolute}{}", parts.join("/"))
}

/// PHP's `dirname` for `/`-separated paths.
fn dirname(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) => "/",
        Some(pos) => &trimmed[..pos],
        None if path.starts_with('/') => "/",
        None => ".",
    }
}

fn basename(path: &str) -> &str {
    path.trim_end_matches('/').rsplit('/').next().unwrap_or("")
}

/// `Filesystem::findShortestPath` (`preferRelative` false).
pub(crate) fn find_shortest_path(from: &str, to: &str, directories: bool) -> String {
    let mut from = normalize_path(from);
    let to = normalize_path(to);
    if directories {
        from = format!("{}/dummy_file", from.trim_end_matches('/'));
    }
    if dirname(&from) == dirname(&to) {
        return format!("./{}", basename(&to));
    }

    let mut common = to.clone();
    while !format!("{from}/").starts_with(&format!("{common}/")) && common != "/" {
        common = dirname(&common).to_string();
    }
    if !from.starts_with(&common) {
        return to;
    }
    let common = format!("{}/", common.trim_end_matches('/'));
    let depth = from.get(common.len()..).unwrap_or("").matches('/').count();
    // Top-level `/foo` and `/bar` are addressed absolutely (Docker layouts).
    if common == "/" && depth > 1 {
        return to;
    }
    let result = format!(
        "{}{}",
        "../".repeat(depth),
        to.get(common.len()..).unwrap_or("")
    );
    if result.is_empty() {
        "./".to_string()
    } else {
        result
    }
}

/// `Filesystem::findShortestPathCode` (`preferRelative` false): PHP code
/// that evaluates to `to` when run from `from`.
pub(crate) fn find_shortest_path_code(
    from: &str,
    to: &str,
    directories: bool,
    static_code: bool,
) -> String {
    let from = normalize_path(from);
    let to = normalize_path(to);
    if from == to {
        return if directories { "__DIR__" } else { "__FILE__" }.to_string();
    }

    let mut common = to.clone();
    while !format!("{from}/").starts_with(&format!("{common}/")) && common != "/" && common != "." {
        common = dirname(&common).to_string();
    }
    if !from.starts_with(&common) || common == "." {
        return export_str(&to);
    }
    let common = format!("{}/", common.trim_end_matches('/'));
    if to.starts_with(&format!("{from}/")) {
        return format!("__DIR__ . {}", export_str(&to[from.len()..]));
    }
    let depth =
        from.get(common.len()..).unwrap_or("").matches('/').count() + usize::from(directories);
    if common == "/" && depth > 1 {
        return export_str(&to);
    }
    let code = if static_code {
        format!("__DIR__ . '{}'", "/..".repeat(depth))
    } else {
        format!("{}__DIR__{}", "dirname(".repeat(depth), ")".repeat(depth))
    };
    let relative = to.get(common.len()..).unwrap_or("");
    if relative.is_empty() {
        code
    } else {
        format!("{code}.{}", export_str(&format!("/{relative}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_package(name: &str, requires: &[&str], is_plugin: bool) -> Package {
        Package {
            name: name.to_string(),
            autoload: Value::Null,
            requires: requires.iter().map(|s| (*s).to_string()).collect(),
            replaces: Vec::new(),
            provides: Vec::new(),
            target_dir: None,
            install_path: None,
            is_dev: false,
            include_path: Vec::new(),
            archive_dir: None,
            is_plugin,
            modifies_downloads: false,
        }
    }

    /// #259: `Transaction::getRootPackages` iterates in descending-name
    /// order (d/d, c/c, b/b, a/a) and skips a package's own `requires` the
    /// moment that package has *already* lost its root status — so by the
    /// time a/a's turn comes (unrooted by c/c's own requires-check, one step
    /// earlier), a/a never gets to unroot b/b via its `requires: [b/b]`.
    /// Only a/a and c/c end up unrooted; b/b and d/d seed the stack, still
    /// in descending order: `[d/d, b/b]`.
    ///
    /// Popping that stack (LIFO; a require is pushed regardless of whether
    /// it's already been visited, so a cycle's ancestor comes back around):
    /// pop b/b (unvisited) -> repush b/b, push its require c/c; pop c/c
    /// (unvisited) -> repush c/c, push its require a/a; pop a/a (unvisited)
    /// -> repush a/a, push its require b/b; pop b/b (visited, unprocessed)
    /// -> emit b/b; pop a/a (visited, unprocessed) -> emit a/a; pop c/c
    /// (visited, unprocessed) -> emit c/c; pop b/b (processed) -> skip; pop
    /// d/d (unvisited) -> repush d/d (no requires); pop d/d (visited,
    /// unprocessed) -> emit d/d.
    #[test]
    fn install_order_cycle_root_quirk_matches_composers_stack() {
        let a = test_package("a/a", &["b/b"], false);
        let b = test_package("b/b", &["c/c"], false);
        let c = test_package("c/c", &["a/a"], false);
        let d = test_package("d/d", &[], false);
        let packages: Vec<&Package> = vec![&a, &b, &c, &d];

        let order: Vec<&str> = install_order(&packages)
            .into_iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(order, ["b/b", "a/a", "c/c", "d/d"]);
    }

    /// `zzz/app` requires `zzz/plugin`, so the plain stack DFS (before
    /// `movePluginsToFront`) emits `[aaa/leaf, zzz/plugin, zzz/app]`: leaf is
    /// the only other root, and the plugin is app's dependency so it comes
    /// out first between the two. `movePluginsToFront` then pulls the
    /// dependency-free plugin to the very front, ahead of even the
    /// independent leaf that sorts before it alphabetically.
    #[test]
    fn install_order_moves_a_dependency_free_plugin_to_the_front() {
        let leaf = test_package("aaa/leaf", &[], false);
        let plugin = test_package("zzz/plugin", &[], true);
        let app = test_package("zzz/app", &["zzz/plugin"], false);
        let packages: Vec<&Package> = vec![&leaf, &plugin, &app];

        let order: Vec<&str> = install_order(&packages)
            .into_iter()
            .map(|p| p.name.as_str())
            .collect();
        assert_eq!(order, ["zzz/plugin", "aaa/leaf", "zzz/app"]);
    }

    #[test]
    fn normalises_paths_like_composer() {
        assert_eq!(normalize_path("/a/b/../c/./d/"), "/a/c/d");
        assert_eq!(normalize_path("../path/../src"), "../src");
        assert_eq!(normalize_path("./"), "");
        assert_eq!(normalize_path("a//b"), "a/b");
    }

    #[test]
    fn shortest_path_code_variants() {
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p/vendor", true, false),
            "dirname(__DIR__)"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor", "/p", true, false),
            "dirname(__DIR__)"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor/composer", "/p", true, true),
            "__DIR__ . '/../..'"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor", "/p/working-dir", true, false),
            "dirname(__DIR__).'/working-dir'"
        );
        assert_eq!(
            find_shortest_path_code("/p/vendor", "/p/vendor/composer", true, false),
            "__DIR__ . '/composer'"
        );
        assert_eq!(find_shortest_path_code("/p", "/p", true, false), "__DIR__");
    }

    #[test]
    fn shortest_path_relative_forms() {
        assert_eq!(
            find_shortest_path("/t/working-dir", "/t/src", true),
            "../src"
        );
        assert_eq!(find_shortest_path("/t", "/t", true), "./");
        assert_eq!(find_shortest_path("/t", "/t/src", true), "./src");
    }

    #[test]
    fn strips_root_target_dir_prefix() {
        assert_eq!(strip_target_dir("Main/Foo/src", "Main/Foo/"), "src");
        assert_eq!(strip_target_dir("lib", "Main/Foo/"), "lib");
    }

    fn empty_scanner<'a>(base: &'a str, vendor: &'a str) -> Scanner<'a> {
        Scanner {
            base,
            vendor,
            excluded: &[],
            archives: ArchiveIndex {
                entries: Vec::new(),
            },
            root_sidecar: None,
            map: BTreeMap::new(),
            scanned: HashSet::new(),
            warnings: Vec::new(),
            regex_cache: HashMap::new(),
            sidecars: HashMap::new(),
            archive_sidecars: HashMap::new(),
            cache_hits: 0,
            cache_misses: 0,
            scan_paths_elapsed: std::time::Duration::ZERO,
            cache_read_elapsed: std::time::Duration::ZERO,
            merge_elapsed: std::time::Duration::ZERO,
            setup_elapsed: std::time::Duration::ZERO,
            cache_write_elapsed: std::time::Duration::ZERO,
        }
    }

    fn two_archive_index(dir_a: &Path, dir_b: &Path) -> ArchiveIndex {
        ArchiveIndex {
            entries: vec![
                (normalize_path(&path_str(dir_a)), dir_a.to_path_buf()),
                (normalize_path(&path_str(dir_b)), dir_b.to_path_buf()),
            ],
        }
    }

    /// #303: a class declared in two different *archive*-backed directories
    /// (unlike `classmap_fold_is_invariant_to_scan_worker_count`'s root+
    /// package case, which never touches an archive sidecar — every one of
    /// its fixtures sets `archive_dir: None`) must resolve the same way
    /// whether the winning task's directory is scanned fresh (a miss, both
    /// tasks below on the first `scan_all`) or comes back as a v1 sidecar
    /// hit (both tasks on the second, against a fresh `Scanner` reusing the
    /// same sidecar files the first one wrote): first task in queue order
    /// wins, the second task's file just warns — same map, same warning,
    /// either way.
    #[test]
    fn archive_sidecar_hit_agrees_with_a_fresh_scan_on_a_cross_archive_ambiguous_class() {
        let root = tempfile::tempdir().expect("tempdir");
        let base = root.path().to_string_lossy().into_owned();
        let vendor = format!("{base}/vendor");

        let dir_a = root.path().join("a");
        let dir_b = root.path().join("b");
        std::fs::create_dir_all(&dir_a).unwrap();
        std::fs::create_dir_all(&dir_b).unwrap();
        std::fs::write(dir_a.join("Dup.php"), "<?php\nclass Dup {}\n").unwrap();
        std::fs::write(dir_b.join("Dup.php"), "<?php\nclass Dup {}\n").unwrap();

        let tasks = vec![
            ScanTask {
                dir: dir_a.to_string_lossy().into_owned(),
                psr: None,
            },
            ScanTask {
                dir: dir_b.to_string_lossy().into_owned(),
                psr: None,
            },
        ];

        let mut cold = Scanner {
            archives: two_archive_index(&dir_a, &dir_b),
            ..empty_scanner(&base, &vendor)
        };
        cold.scan_all(&tasks).expect("cold scan should succeed");
        assert_eq!(cold.cache_misses, 2, "neither directory has a sidecar yet");
        assert_eq!(cold.cache_hits, 0);

        let mut warm = Scanner {
            archives: two_archive_index(&dir_a, &dir_b),
            ..empty_scanner(&base, &vendor)
        };
        warm.scan_all(&tasks).expect("warm scan should succeed");
        assert_eq!(
            warm.cache_hits, 2,
            "the miss above wrote a v1 sidecar for both"
        );
        assert_eq!(warm.cache_misses, 0);

        assert_eq!(
            cold.map, warm.map,
            "the same class must resolve to the same file whether scanned fresh or cache-hit"
        );
        assert_eq!(
            cold.warnings, warm.warnings,
            "the same ambiguous-class warning must fire either way"
        );
        assert_eq!(
            cold.map.get(b"Dup".as_slice()),
            Some(&format!("{}/Dup.php", normalize_path(&path_str(&dir_a)))),
            "the first task in queue order wins the tie"
        );
        assert_eq!(cold.warnings.len(), 1, "one ambiguous-class warning");
    }

    /// #198: with no store archive backing any of these directories,
    /// `ArchiveIndex::locate` never matches, so every task below is a cache
    /// miss and `scan_all` fans every one of them out across its thread
    /// pool. One directory does not exist, so its `scan_paths` call errors;
    /// the rest are real, empty directories that scan cleanly. More tasks
    /// than the machine has cores gives the pool's `stop` short-circuit
    /// room to skip a later index on some other worker before that worker
    /// ever reaches it — the exact scheduling `scan_all`'s fold used to
    /// assume away and panic on (`.expect("every miss index was scanned
    /// exactly once")`). Which indices actually get skipped is scheduling-
    /// dependent and not asserted directly; what matters, and is asserted,
    /// is that the call still returns the error instead of panicking.
    #[test]
    fn scan_all_returns_an_error_instead_of_panicking_when_a_directory_is_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let base = dir.path().to_string_lossy().into_owned();
        let vendor = format!("{base}/vendor");

        let mut tasks = Vec::new();
        for i in 0..64 {
            let sub = format!("ok-{i}");
            std::fs::create_dir(dir.path().join(&sub)).expect("create ok dir");
            tasks.push(ScanTask {
                dir: format!("{base}/{sub}"),
                psr: None,
            });
        }
        tasks.push(ScanTask {
            dir: format!("{base}/does-not-exist"),
            psr: None,
        });

        let mut scanner = empty_scanner(&base, &vendor);
        let error = scanner
            .scan_all(&tasks)
            .expect_err("a missing directory should error, not panic");
        assert!(
            error.to_string().contains("Could not scan for classes"),
            "unexpected error: {error}"
        );
    }

    /// #237: `--apcu-autoloader` without an explicit prefix must reuse the
    /// one already in `vendor/composer/autoload_real.php` rather than
    /// mint a fresh one on every dump, an explicit prefix must still win,
    /// and a cleared `vendor/` (no autoloader to read) must fall back to a
    /// new random one.
    #[test]
    fn apcu_prefix_is_reused_across_dumps_and_regenerated_once_vendor_is_cleared() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vendor_dir = dir.path().join("vendor");
        fs_err::create_dir_all(&vendor_dir).expect("create vendor dir");

        let dump = |apcu_prefix: Option<Option<String>>, vendor_dir: &Path| -> Option<String> {
            let input = Input {
                root: RootPackage {
                    name: "root/pkg".into(),
                    autoload: Value::Null,
                    autoload_dev: Value::Null,
                    target_dir: None,
                    requires: vec![],
                    include_path: vec![],
                },
                packages: vec![],
                dev_mode: false,
                scan_psr: false,
                suffix: "abc123".into(),
                vendor_dir: vendor_dir.to_path_buf(),
                base_dir: dir.path().to_path_buf(),
                root_fingerprint: None,
                platform_check: false,
                prepend_autoloader: true,
                classmap_authoritative: false,
                apcu_prefix,
                use_include_path: false,
            };
            generate(&input).expect("generate");
            let real_file =
                fs_err::read_to_string(vendor_dir.join("composer/autoload_real.php")).unwrap();
            Regex::new(r"setApcuPrefix\('([^']+)'\)")
                .unwrap()
                .captures(&real_file)
                .map(|caps| caps[1].to_string())
        };

        let first = dump(Some(None), &vendor_dir).expect("apcu prefix written");
        let second = dump(Some(None), &vendor_dir).expect("apcu prefix written");
        assert_eq!(first, second, "an unchanged vendor/ must reuse the prefix");

        fs_err::remove_dir_all(&vendor_dir).unwrap();
        fs_err::create_dir_all(&vendor_dir).unwrap();
        let after_clean = dump(Some(None), &vendor_dir).expect("apcu prefix written");
        assert_ne!(
            first, after_clean,
            "a cleared vendor/ has nothing to read a prefix from, so it gets a new one"
        );

        let explicit =
            dump(Some(Some("fixed-prefix".into())), &vendor_dir).expect("apcu prefix written");
        assert_eq!(explicit, "fixed-prefix", "an explicit prefix always wins");
    }
}
