//! #350: a `WordPress` plugin's bundled dependency tree clashing with the
//! site's own `composer.lock` — candidate 3.2's first build step
//! (`docs/research.md`). Detection only: nothing here is written or
//! rewritten; the fix (`extra.viv.isolate`, prefixing at install time) is
//! #351's job, a human's call until then.
//!
//! Ported from `bench/g3-isolation/inventory.py`/`sites.py` (the research
//! scripts that measured this against WordPress.org and five real sites):
//! same `installed.json` shapes, same directory fallback, same namespace
//! check, same coexist list — rules, not code, since those read a zip or a
//! bare checkout and this reads a directory `viv` just linked.
//!
//! Cost (#300's own pattern): the verdict for one plugin is cached in the
//! store keyed by the plugin's own archive hash plus the site lock's
//! content hash ([`crate::store::isolate_check_sidecar`]), so a warm
//! install with neither changed never opens the plugin's `vendor/` at all
//! — only the small cache file and the store's own dist pointer, neither of
//! them under the project.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::install::package_dir;
use crate::lock::{Lock, Package};
use crate::plugins::data::is_coexist_library;
use crate::store::{self, Store};

/// `inventory.py`'s own cap: up to this many `.php` files are opened
/// looking for the first 3 that declare a `namespace` line at all — a
/// polyfill-style library leads with bootstrap shims that carry a `use`
/// statement and nothing else, so sampling by list position alone would
/// read as a false "no namespace found".
const NAMESPACE_SAMPLE_CANDIDATES: usize = 20;
const NAMESPACE_SAMPLE_SIZE: usize = 3;

/// Strauss/Mozart/php-scoper's own convention of relocating prefixed code
/// out of plain `vendor/` entirely — a library's own directory found under
/// one of these is prefixed by construction, no namespace scan needed.
const PREFIXED_FAMILIES: [&str; 4] = [
    "vendor_prefixed",
    "vendor-prefixed",
    "lib/packages",
    "dependencies",
];

static NAMESPACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*namespace\s+([^\s;{]+)").unwrap());

/// After linking, for every installed package whose install path is a
/// plugin or mu-plugin directory, compare its bundled `vendor/` tree
/// against `lock`'s own package versions and print one stderr message per
/// plugin that bundles an unprefixed library at a different version.
/// `install` and `update` both call this the same way, once per run —
/// plugins viv never installed (not in `lock`) are never looked at.
///
/// `lock_sha256` is a closure, not a plain `&str`: both callers in
/// `install.rs` already have a `Snapshot` (that module's own private
/// memoizing cache) that can compute it, but hashing `composer.lock`'s
/// bytes is a real read this function
/// must not force on the common (no plugins at all) case — called only
/// once the plugins list below is non-empty, so a project with none pays
/// nothing beyond that one in-memory filter, keeping the no-op/warm paths
/// `bench-ab` grades exactly as cheap as before this existed. Likewise
/// `cache_dir`'s `Store` is opened in here, not passed in, so the same
/// holds for it.
pub(crate) fn check(
    lock: &Lock,
    dev: bool,
    vendor_dir: &Path,
    project_dir: &Path,
    cache_dir: Option<&Path>,
    lock_sha256: impl FnOnce() -> Result<String>,
) -> Result<()> {
    let plugins: Vec<&Package> = lock
        .packages(dev)
        .filter(|p| is_plugin_package(p))
        .collect();
    if plugins.is_empty() {
        // The common case for anything that isn't a WordPress site: no
        // disk I/O at all past the lock `install` already parsed.
        return Ok(());
    }
    let lock_sha256 = lock_sha256()?;
    let store = match cache_dir {
        Some(dir) => Some(Store::open(dir)?),
        None => None,
    };
    let site_versions: HashMap<&str, &str> = lock
        .packages(dev)
        .map(|p| (p.name.as_str(), p.version.as_str()))
        .collect();

    for package in plugins {
        let plugin_dir = package_dir(vendor_dir, project_dir, package);
        let clashes = verdict_for(
            package,
            &plugin_dir,
            store.as_ref(),
            cache_dir,
            &lock_sha256,
            &site_versions,
        );
        if let Some(message) = format_message(package.pretty_name(), &clashes) {
            warn_out(&message);
        }
    }
    Ok(())
}

/// A package's resolved install path puts it in a plugin or mu-plugin
/// directory — the native `wordpress-plugin`/`wordpress-muplugin` types, or
/// any type a root's own `extra.installer-paths` override still routes
/// into one (`src/plugins/data.rs`'s `install_path`, already resolved onto
/// `package.install_dir` before planning). Covers a type/path mismatch,
/// too: a `wordpress-plugin` whose mapping adapter is disabled
/// (`--no-plugins`, `allow-plugins` false) still sits in `vendor/<name>` on
/// disk, not literally a `plugins/` directory, but it is still `WordPress`'
/// own bundled-dependency risk either type check alone would miss.
fn is_plugin_package(package: &Package) -> bool {
    matches!(
        package.r#type.as_str(),
        "wordpress-plugin" | "wordpress-muplugin"
    ) || has_plugin_path_component(package.install_dir.as_deref())
}

fn has_plugin_path_component(install_dir: Option<&str>) -> bool {
    let Some(install_dir) = install_dir else {
        return false;
    };
    Path::new(install_dir)
        .components()
        .any(|c| matches!(c.as_os_str().to_str(), Some("plugins" | "mu-plugins")))
}

/// One bundled library's clash against the site's own lock, kept both as
/// [`check`]'s in-memory answer and the cached verdict's own shape
/// (`#[serde]`, round-tripped through [`crate::store::isolate_check_sidecar`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Clash {
    library: String,
    /// `None` is "version unknown" in the printed message (a directory-name
    /// fallback, not an `installed.json` entry) — never counted as a clash
    /// on its own, see [`compute_clashes`].
    bundled_version: Option<String>,
    site_version: String,
}

#[derive(Serialize, Deserialize)]
struct Verdict {
    clashes: Vec<Clash>,
}

/// The cached verdict for `package` when its inputs (archive + lock) match
/// a prior run, or a freshly computed one — written back to the cache
/// either way a dist archive is known, so the next run with nothing
/// changed never has to read `plugin_dir` at all.
fn verdict_for(
    package: &Package,
    plugin_dir: &Path,
    store: Option<&Store>,
    cache_dir: Option<&Path>,
    lock_sha256: &str,
    site_versions: &HashMap<&str, &str>,
) -> Vec<Clash> {
    let archive_hash = store
        .and_then(|store| store.lookup(package))
        .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().into_owned()));
    let sidecar = match (&archive_hash, cache_dir) {
        (Some(hash), Some(dir)) => Some(store::isolate_check_sidecar(
            dir,
            &format!("{hash}:{lock_sha256}"),
        )),
        // No dist archive (a path/git-source plugin, rare) or no cache dir
        // yet: recomputed every run rather than cached under a key that
        // couldn't tell one plugin version from another.
        _ => None,
    };
    if let Some(sidecar) = &sidecar
        && let Some(cached) = read_verdict(sidecar)
    {
        return cached.clashes;
    }

    let clashes = compute_clashes(plugin_dir, site_versions);
    if let Some(sidecar) = &sidecar
        && let Ok(bytes) = serde_json::to_vec(&Verdict {
            clashes: clashes.clone(),
        })
    {
        let _ = write_atomic(sidecar, &bytes);
    }
    clashes
}

/// Every bundled library also in `site_versions` at a different version,
/// unprefixed and not on the coexist list — a version this plugin's own
/// tree didn't let us read is appended only once at least one clash with a
/// readable version exists (never reported on its own), so it always
/// trails the readable ones in the returned list.
fn compute_clashes(plugin_dir: &Path, site_versions: &HashMap<&str, &str>) -> Vec<Clash> {
    clashes_from_libraries(bundled_libraries(plugin_dir), site_versions)
}

/// [`compute_clashes`]'s filter/ordering, split out from the disk read so
/// tests can feed it a hand-built library list directly.
fn clashes_from_libraries(
    libs: Vec<BundledLib>,
    site_versions: &HashMap<&str, &str>,
) -> Vec<Clash> {
    let mut readable = Vec::new();
    let mut unknown = Vec::new();
    for lib in libs {
        if lib.prefixed || is_coexist_library(&lib.name) {
            continue;
        }
        let Some(&site_version) = site_versions.get(lib.name.as_str()) else {
            continue;
        };
        match lib.version {
            Some(version) if version == site_version => {}
            Some(version) => readable.push(Clash {
                library: lib.name,
                bundled_version: Some(version),
                site_version: site_version.to_string(),
            }),
            None => unknown.push(Clash {
                library: lib.name,
                bundled_version: None,
                site_version: site_version.to_string(),
            }),
        }
    }
    if readable.is_empty() {
        return Vec::new();
    }
    readable.extend(unknown);
    readable
}

struct BundledLib {
    name: String,
    version: Option<String>,
    prefixed: bool,
}

/// `plugin_dir`'s own bundled dependency tree: `vendor/composer/installed.json`
/// when it parses to at least one package (either Composer shape), else a
/// directory-name fallback under plain `vendor/<vendor>/<name>/` (version
/// unknown) — `bench/g3-isolation/sites.py`'s own two-tier read.
fn bundled_libraries(plugin_dir: &Path) -> Vec<BundledLib> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();

    if let Some(packages) = read_installed_json(plugin_dir) {
        for pkg in packages {
            if pkg.name.is_empty() || !seen.insert(pkg.name.clone()) {
                continue;
            }
            let root_ns = declared_root_namespace(pkg.autoload.as_ref());
            let prefixed = is_prefixed(plugin_dir, &pkg.name, root_ns.as_deref());
            out.push(BundledLib {
                name: pkg.name,
                version: Some(pkg.version),
                prefixed,
            });
        }
        return out;
    }

    for name in vendor_dir_fallback(plugin_dir) {
        if !seen.insert(name.clone()) {
            continue;
        }
        let prefixed = is_prefixed(plugin_dir, &name, None);
        out.push(BundledLib {
            name,
            version: None,
            prefixed,
        });
    }
    out
}

/// `vendor/composer/installed.json`'s packages, reusing [`Package`]'s own
/// `Deserialize` (the same shape a lock entry has — Composer wrote both the
/// same way) rather than a second hand-rolled struct. `None` when the file
/// is missing, isn't valid JSON in either Composer shape, or parses to zero
/// packages — every one of those is "nothing readable here", the condition
/// [`bundled_libraries`] falls back on.
fn read_installed_json(plugin_dir: &Path) -> Option<Vec<Package>> {
    let bytes = fs_err::read(plugin_dir.join("vendor/composer/installed.json")).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let entries: Vec<Value> = match value {
        Value::Object(mut obj) => match obj.remove("packages") {
            Some(Value::Array(packages)) => packages,
            _ => return None,
        },
        Value::Array(packages) => packages,
        _ => return None,
    };
    let packages: Vec<Package> = entries
        .into_iter()
        .filter_map(|entry| serde_json::from_value(entry).ok())
        .collect();
    (!packages.is_empty()).then_some(packages)
}

/// `inventory.py`'s `vendor_dir_fallback`, read straight off disk instead of
/// a zip's member list: every `vendor/<vendor>/<name>/` directory, skipping
/// `vendor/composer/` and `vendor/bin/` (Composer's own scaffolding, never a
/// library). Version is always unknown here — that's the whole reason this
/// is a fallback, not the primary read.
fn vendor_dir_fallback(plugin_dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(vendor_entries) = fs_err::read_dir(plugin_dir.join("vendor")) else {
        return out;
    };
    for vendor_entry in vendor_entries.flatten() {
        if !vendor_entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            continue;
        }
        let vendor_name = vendor_entry.file_name().to_string_lossy().into_owned();
        if vendor_name == "composer" || vendor_name == "bin" {
            continue;
        }
        let Ok(pkg_entries) = fs_err::read_dir(vendor_entry.path()) else {
            continue;
        };
        for pkg_entry in pkg_entries.flatten() {
            if !pkg_entry.file_type().is_ok_and(|ft| ft.is_dir()) {
                continue;
            }
            let pkg_name = pkg_entry.file_name().to_string_lossy().into_owned();
            out.push(format!("{vendor_name}/{pkg_name}").to_ascii_lowercase());
        }
    }
    out
}

/// Whether `lib_name`'s own bundled copy under `plugin_dir` reads as
/// prefixed: its directory sitting under one of [`PREFIXED_FAMILIES`] is
/// enough on its own (a prefixed-dir signal counts as prefixed); otherwise
/// a namespace scan of up to [`NAMESPACE_SAMPLE_SIZE`] of its own `.php`
/// files under plain `vendor/<lib_name>/`, same as `inventory.py`'s
/// `detect_prefixed`. No namespace found at all (old code with no
/// namespaces, or too few `.php` files sampled) reads as not prefixed here
/// — there is nothing to disprove, and the ticket only ever reports a
/// library as unprefixed or skips it, no third state.
fn is_prefixed(plugin_dir: &Path, lib_name: &str, declared_root_ns: Option<&str>) -> bool {
    if PREFIXED_FAMILIES
        .iter()
        .any(|family| plugin_dir.join(family).join(lib_name).is_dir())
    {
        return true;
    }

    let mut candidates = Vec::new();
    collect_php_files(
        &plugin_dir.join("vendor").join(lib_name),
        &mut candidates,
        NAMESPACE_SAMPLE_CANDIDATES,
    );
    let mut sampled = Vec::new();
    for file in &candidates {
        if sampled.len() >= NAMESPACE_SAMPLE_SIZE {
            break;
        }
        if let Ok(bytes) = fs_err::read(file)
            && let Some(ns) = first_namespace(&String::from_utf8_lossy(&bytes))
        {
            sampled.push(ns);
        }
    }
    let Some(first) = sampled.first() else {
        return false;
    };
    let root_ns = declared_root_ns.map_or_else(
        || first.split('\\').next().unwrap_or("").to_string(),
        str::to_string,
    );
    if sampled
        .iter()
        .any(|ns| *ns == root_ns || ns.starts_with(&format!("{root_ns}\\")))
    {
        // Still carries (a sub-namespace of) its own root: unprefixed.
        return false;
    }
    // A namespace present but not rooted at the library's own root: a
    // foreign wrapper if the root's own leaf segment still shows up inside
    // it (`WPForms\Vendor\GuzzleHttp`), same reading `inventory.py` uses.
    let root_leaf = root_ns.rsplit('\\').next().unwrap_or(root_ns.as_str());
    !root_leaf.is_empty() && sampled.iter().any(|ns| ns.contains(root_leaf))
}

fn collect_php_files(dir: &Path, out: &mut Vec<PathBuf>, limit: usize) {
    if out.len() >= limit {
        return;
    }
    let Ok(entries) = fs_err::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if out.len() >= limit {
            return;
        }
        let path = entry.path();
        if entry.file_type().is_ok_and(|ft| ft.is_dir()) {
            collect_php_files(&path, out, limit);
        } else if path.extension().is_some_and(|ext| ext == "php") {
            out.push(path);
        }
    }
}

fn first_namespace(text: &str) -> Option<String> {
    NAMESPACE_RE.captures(text).map(|c| c[1].to_string())
}

/// A bundled library's own `autoload.psr-4` root namespace, straight from
/// its `installed.json` entry (the same data a lock entry carries — no need
/// to reopen the library's own nested `composer.json` the way `inventory.py`
/// does, reading a zip with no parsed `installed.json` to hand instead).
fn declared_root_namespace(autoload: Option<&Value>) -> Option<String> {
    let psr4 = autoload?.get("psr-4")?.as_object()?;
    let key = psr4.keys().next()?;
    let trimmed = key.trim_end_matches('\\');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// One stderr message for `clashes` (already ordered readable-first by
/// [`compute_clashes`]), or `None` when there is nothing to report. Every
/// clashing library of the plugin lands in this one message — one line per
/// library after the first, indented — never one message per library.
fn format_message(plugin_name: &str, clashes: &[Clash]) -> Option<String> {
    let (first, rest) = clashes.split_first()?;
    // `compute_clashes` never puts an unknown-version entry first.
    let mut message = format!(
        "{plugin_name} bundles {} {} unprefixed; the site has {}. Run viv isolate {plugin_name} \
         to keep both.",
        first.library,
        first.bundled_version.as_deref().unwrap_or("unknown"),
        first.site_version,
    );
    for clash in rest {
        match &clash.bundled_version {
            Some(version) => {
                let _ = write!(
                    message,
                    "\n    {} {version} unprefixed; the site has {}.",
                    clash.library, clash.site_version
                );
            }
            None => {
                let _ = write!(
                    message,
                    "\n    {} version unknown; the site has {}.",
                    clash.library, clash.site_version
                );
            }
        }
    }
    Some(message)
}

fn read_verdict(sidecar: &Path) -> Option<Verdict> {
    serde_json::from_slice(&fs_err::read(sidecar).ok()?).ok()
}

/// Write `content` to `path` via a temp file in the same directory renamed
/// into place, same crash-safety shape as every other sidecar cache here
/// (`install.rs`'s own `write_atomic`, not shared across modules).
fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs_err::create_dir_all(parent)?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating a temp file next to {}", path.display()))?;
    temp.write_all(content)?;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr`
/// lint — `install.rs`'s own `warn_out`, not reusable from here (`fn`, not
/// `pub(crate)`), so repeated rather than threaded through every caller.
fn warn_out(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_file(path: &Path, content: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    // --- installed.json shapes -----------------------------------------

    #[test]
    fn reads_composer_1_array_shape() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("vendor/composer/installed.json"),
            json!([{"name": "guzzlehttp/guzzle", "version": "7.10.0"}])
                .to_string()
                .as_bytes(),
        );
        let packages = read_installed_json(dir.path()).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "guzzlehttp/guzzle");
        assert_eq!(packages[0].version, "7.10.0");
    }

    #[test]
    fn reads_composer_2_packages_wrapper_shape() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("vendor/composer/installed.json"),
            json!({"packages": [{"name": "guzzlehttp/guzzle", "version": "7.10.0"}]})
                .to_string()
                .as_bytes(),
        );
        let packages = read_installed_json(dir.path()).unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].name, "guzzlehttp/guzzle");
    }

    #[test]
    fn missing_installed_json_is_none() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_installed_json(dir.path()).is_none());
    }

    // --- directory fallback ---------------------------------------------

    #[test]
    fn falls_back_to_vendor_directory_names_with_unknown_version() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("vendor/guzzlehttp/guzzle/src/Client.php"),
            b"<?php\n",
        );
        write_file(
            &dir.path().join("vendor/composer/ClassLoader.php"),
            b"<?php\n",
        );
        write_file(&dir.path().join("vendor/bin/stub"), b"\n");

        let libs = bundled_libraries(dir.path());
        assert_eq!(libs.len(), 1);
        assert_eq!(libs[0].name, "guzzlehttp/guzzle");
        assert_eq!(libs[0].version, None);
    }

    // --- prefix detection --------------------------------------------

    #[test]
    fn namespaced_file_matching_its_own_root_is_not_prefixed() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("vendor/guzzlehttp/guzzle/src/Client.php"),
            b"<?php\nnamespace GuzzleHttp;\nclass Client {}\n",
        );
        assert!(!is_prefixed(
            dir.path(),
            "guzzlehttp/guzzle",
            Some("GuzzleHttp")
        ));
    }

    #[test]
    fn namespace_wrapping_the_root_in_a_foreign_segment_is_prefixed() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("vendor/guzzlehttp/guzzle/src/Client.php"),
            b"<?php\nnamespace WPForms\\Vendor\\GuzzleHttp;\nclass Client {}\n",
        );
        assert!(is_prefixed(
            dir.path(),
            "guzzlehttp/guzzle",
            Some("GuzzleHttp")
        ));
    }

    #[test]
    fn no_namespace_declared_anywhere_is_not_prefixed() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("vendor/acme/legacy/Legacy.php"),
            b"<?php\nclass Acme_Legacy {}\n",
        );
        assert!(!is_prefixed(dir.path(), "acme/legacy", None));
    }

    #[test]
    fn a_prefixed_family_directory_counts_as_prefixed_with_no_namespace_scan() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path()
                .join("vendor_prefixed/guzzlehttp/guzzle/src/Client.php"),
            b"<?php\nnamespace GuzzleHttp;\n", // would read unprefixed on its own
        );
        assert!(is_prefixed(
            dir.path(),
            "guzzlehttp/guzzle",
            Some("GuzzleHttp")
        ));
    }

    // --- message formatting ----------------------------------------------

    #[test]
    fn single_clash_matches_the_issue_wording() {
        let clashes = vec![Clash {
            library: "guzzlehttp/guzzle".to_string(),
            bundled_version: Some("7.10.0".to_string()),
            site_version: "7.15.1".to_string(),
        }];
        assert_eq!(
            format_message("multilingualpress", &clashes).unwrap(),
            "multilingualpress bundles guzzlehttp/guzzle 7.10.0 unprefixed; the site has \
             7.15.1. Run viv isolate multilingualpress to keep both."
        );
    }

    #[test]
    fn two_clashes_print_as_one_message_with_an_indented_second_line() {
        let clashes = vec![
            Clash {
                library: "guzzlehttp/guzzle".to_string(),
                bundled_version: Some("7.10.0".to_string()),
                site_version: "7.15.1".to_string(),
            },
            Clash {
                library: "monolog/monolog".to_string(),
                bundled_version: Some("1.0.0".to_string()),
                site_version: "2.0.0".to_string(),
            },
        ];
        assert_eq!(
            format_message("acme/plugin", &clashes).unwrap(),
            "acme/plugin bundles guzzlehttp/guzzle 7.10.0 unprefixed; the site has 7.15.1. Run \
             viv isolate acme/plugin to keep both.\n    monolog/monolog 1.0.0 unprefixed; the \
             site has 2.0.0."
        );
    }

    #[test]
    fn no_clashes_is_no_message() {
        assert!(format_message("acme/plugin", &[]).is_none());
    }

    // --- compute_clashes: unknown version only counts as context ---------

    #[test]
    fn unknown_version_alone_reports_nothing() {
        let mut site_versions = HashMap::new();
        site_versions.insert("acme/legacy", "2.0.0");
        let clashes = clashes_from_libraries(
            vec![BundledLib {
                name: "acme/legacy".to_string(),
                version: None,
                prefixed: false,
            }],
            &site_versions,
        );
        assert!(clashes.is_empty());
    }

    #[test]
    fn unknown_version_trails_a_readable_clash_from_the_same_plugin() {
        let mut site_versions = HashMap::new();
        site_versions.insert("guzzlehttp/guzzle", "7.15.1");
        site_versions.insert("acme/legacy", "2.0.0");
        let clashes = clashes_from_libraries(
            vec![
                BundledLib {
                    name: "acme/legacy".to_string(),
                    version: None,
                    prefixed: false,
                },
                BundledLib {
                    name: "guzzlehttp/guzzle".to_string(),
                    version: Some("7.10.0".to_string()),
                    prefixed: false,
                },
            ],
            &site_versions,
        );
        assert_eq!(clashes.len(), 2);
        assert_eq!(clashes[0].library, "guzzlehttp/guzzle");
        assert_eq!(clashes[1].library, "acme/legacy");
        assert_eq!(clashes[1].bundled_version, None);
    }

    // --- coexist list ------------------------------------------------------

    #[test]
    fn coexist_globs_match_as_the_data_file_declares() {
        assert!(is_coexist_library("woocommerce/action-scheduler"));
        assert!(is_coexist_library("automattic/jetpack-config"));
        assert!(is_coexist_library("psr/log"));
        assert!(is_coexist_library("symfony/polyfill-mbstring"));
        assert!(!is_coexist_library("guzzlehttp/guzzle"));
    }
}
