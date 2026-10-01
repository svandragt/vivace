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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use clap::Args;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::install::{self, DumpAutoloadArgs, package_dir};
use crate::link::{self, LinkMode};
use crate::lock::{self, Lock, Package, Root};
use crate::normalize;
use crate::php;
use crate::plugins::{self, data::is_coexist_library};
use crate::require::write_composer_json;
use crate::source::copy_dir;
use crate::store::{self, Store, hex};
use crate::tool;

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
/// `isolated` (#351's `extra.viv.isolate`, lowercased names) silences a
/// plugin this same run already prefixed: a clash the scoper just resolved
/// is not news.
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
    isolated: &HashSet<String>,
    lock_sha256: impl FnOnce() -> Result<String>,
) -> Result<()> {
    let plugins: Vec<&Package> = lock
        .packages(dev)
        .filter(|p| is_plugin_package(p) && !isolated.contains(&p.name))
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
pub(crate) fn is_plugin_package(package: &Package) -> bool {
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

/// `dir/vendor/composer/installed.json`'s `packages` array, either Composer
/// shape (a bare array, or an object with a `packages` key), as raw
/// [`Value`]s — shared by [`read_installed_json`] (the clash-check's own
/// typed read) and #351's synthetic-lock builder ([`dump_scoped_autoload`]),
/// which needs the untouched entries (`dist`, `source`, the lot) to write
/// back out, not just the fields
/// [`Package`] keeps. `None` for the same three reasons both callers treat
/// as "nothing readable here": missing file, invalid JSON in either shape,
/// or zero entries.
fn installed_json_packages_raw(dir: &Path) -> Option<Vec<Value>> {
    let bytes = fs_err::read(dir.join("vendor/composer/installed.json")).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let entries: Vec<Value> = match value {
        Value::Object(mut obj) => match obj.remove("packages") {
            Some(Value::Array(packages)) => packages,
            _ => return None,
        },
        Value::Array(packages) => packages,
        _ => return None,
    };
    (!entries.is_empty()).then_some(entries)
}

/// `vendor/composer/installed.json`'s packages, reusing [`Package`]'s own
/// `Deserialize` (the same shape a lock entry has — Composer wrote both the
/// same way) rather than a second hand-rolled struct. `None` when the file
/// is missing, isn't valid JSON in either Composer shape, or parses to zero
/// packages — every one of those is "nothing readable here", the condition
/// [`bundled_libraries`] falls back on.
fn read_installed_json(plugin_dir: &Path) -> Option<Vec<Package>> {
    let entries = installed_json_packages_raw(plugin_dir)?;
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

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}

// --- #351: `viv isolate` ---------------------------------------------------
//
// `check`, above, only ever reports a clash; everything from here down is
// the fix: `extra.viv.isolate` names a plugin, and every `install`/`update`
// (plus this module's own CLI) runs php-scoper over its bundled `vendor/`
// with a prefix derived from the slug, caching the scoped tree in the store
// by (archive hash, prefix hash, scoper version) so a repeat install links
// it with no scoper run at all.

/// `viv isolate`'s flags: `<package>` adds it to `extra.viv.isolate` (and
/// isolates it right away); `--rm <package>` removes it and relinks the
/// plain archive; `--list` prints every isolated plugin and its prefix.
/// Mirrors `tool::XArgs`'s own shape (`--uninstall` a package spec,
/// `--list`, a plain positional for the add case) rather than a
/// subcommand enum, since all three share the same "`package` or a flag,
/// never both" structure.
#[derive(Args, Debug, Clone)]
pub struct IsolateArgs {
    /// Print every isolated plugin and its prefix, one per line.
    #[arg(long)]
    pub list: bool,
    /// Remove this package from `extra.viv.isolate` and relink the plain
    /// archive, instead of adding one.
    #[arg(long, value_name = "PACKAGE")]
    pub rm: Option<String>,
    /// `vendor/package` to isolate; omit with `--list`/`--rm`.
    #[arg(required_unless_present_any = ["list", "rm"])]
    pub package: Option<String>,
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
}

pub fn run_isolate(args: &IsolateArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    if args.list {
        return list_isolated(&project_dir);
    }
    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    if let Some(package) = &args.rm {
        return remove_isolate(&project_dir, &package.to_ascii_lowercase(), &cache_dir);
    }
    let package = args
        .package
        .as_deref()
        .expect("clap requires `package` without --list/--rm")
        .to_ascii_lowercase();
    add_isolate(&project_dir, &package, &cache_dir, offline)
}

fn list_isolated(project_dir: &Path) -> Result<()> {
    let bytes = fs_err::read(project_dir.join("composer.json")).context("reading composer.json")?;
    let root = lock::parse_root(&bytes).context("parsing composer.json")?;
    let vendor_dir = project_dir.join(&root.config.vendor_dir);
    for (package, prefix) in install::read_isolate_state(&vendor_dir) {
        out(&format!("{package}  {prefix}"));
    }
    Ok(())
}

/// `project_dir`'s root `composer.json` (both as the raw [`Value`] a
/// caller edits and writes back, and as [`Root`]), `composer.lock`, and
/// each locked package's `install_dir` resolved the same way
/// `install::dump_autoload` resolves it — the shared setup `add_isolate`/
/// `remove_isolate` both need before they can find the named package's
/// on-disk plugin directory.
fn load_project(project_dir: &Path) -> Result<(String, Value, Root, Lock, PathBuf)> {
    let composer_json_path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&composer_json_path).context("reading composer.json")?;
    let root_value: Value = serde_json::from_str(&original).context("parsing composer.json")?;
    let root = lock::root_from_value(&root_value).context("parsing composer.json")?;
    let mut lock =
        lock::read_lock(&project_dir.join("composer.lock")).context("reading composer.lock")?;
    let (resolved, warnings) = plugins::resolve(&lock, &root, false)?;
    for warning in &warnings {
        warn_out(warning);
    }
    for package in &mut lock.packages {
        package.install_dir = resolved.install_dir(&root, package);
    }
    let vendor_dir = project_dir.join(&root.config.vendor_dir);
    Ok((original, root_value, root, lock, vendor_dir))
}

fn find_package<'a>(lock: &'a Lock, name: &str) -> Option<&'a Package> {
    lock.packages(true).find(|p| p.name == name)
}

fn add_isolate(project_dir: &Path, package: &str, cache_dir: &Path, offline: bool) -> Result<()> {
    let (original, mut root_value, _root, lock, vendor_dir) = load_project(project_dir)?;
    let locked = find_package(&lock, package)
        .with_context(|| format!("{package}: not in composer.lock; run `viv install` first"))?;
    if !is_plugin_package(locked) {
        bail!("{package}: install path is not a plugin directory, nothing to isolate");
    }
    let plugin_dir = package_dir(&vendor_dir, project_dir, locked);
    if !plugin_dir.is_dir() {
        bail!(
            "{}: not installed; run `viv install` first",
            plugin_dir.display()
        );
    }
    let locked = locked.clone();

    edit_isolate_list(&mut root_value, package, true)?;
    let composer_json_path = project_dir.join("composer.json");
    let indent = normalize::detect_indent(&original);
    write_composer_json(&composer_json_path, &root_value)?;
    if normalize::maybe_normalize(&composer_json_path, &indent)? {
        warn_out(&format!("Normalized {}", composer_json_path.display()));
    }
    let root = lock::root_from_value(&root_value)?;

    let store = Store::open(cache_dir)?;
    let php_dir = php::project_php_dir(project_dir, Some(cache_dir), offline)?;
    let scoper_version = apply_for(
        &locked,
        &plugin_dir,
        &store,
        cache_dir,
        php_dir.as_deref(),
        LinkMode::default(),
        offline,
    )?;
    let prefixes = prefix_map(&root);
    let mut checked = install::seed_isolate_checked(&vendor_dir, &prefixes);
    checked.insert(package.to_string(), scoper_version);
    install::write_isolate_state(&vendor_dir, &prefixes, &checked)
}

fn remove_isolate(project_dir: &Path, package: &str, cache_dir: &Path) -> Result<()> {
    let (original, mut root_value, _root, lock, vendor_dir) = load_project(project_dir)?;
    let locked = find_package(&lock, package).cloned();

    edit_isolate_list(&mut root_value, package, false)?;
    let composer_json_path = project_dir.join("composer.json");
    let indent = normalize::detect_indent(&original);
    write_composer_json(&composer_json_path, &root_value)?;
    if normalize::maybe_normalize(&composer_json_path, &indent)? {
        warn_out(&format!("Normalized {}", composer_json_path.display()));
    }
    let root = lock::root_from_value(&root_value)?;

    if let Some(locked) = locked {
        let plugin_dir = package_dir(&vendor_dir, project_dir, &locked);
        if plugin_dir.is_dir() {
            let store = Store::open(cache_dir)?;
            if let Some(plain) = store.lookup(&locked) {
                link::link_tree(&plain, &plugin_dir, LinkMode::default())?;
            }
        }
    }
    let prefixes = prefix_map(&root);
    let checked = install::seed_isolate_checked(&vendor_dir, &prefixes);
    install::write_isolate_state(&vendor_dir, &prefixes, &checked)
}

/// Edits `extra.viv.isolate` in place: adds or removes `package`
/// (case-insensitively), then always rewrites the array sorted and
/// deduplicated, dropping `extra.viv`/`extra` entirely once empty, the same
/// `remove_main_key_if_empty` shape `require.rs` uses for `require`/
/// `require-dev`. Returns whether the named package's own membership
/// changed (not whether the sort/dedup pass touched anything else), so a
/// caller could skip reapplying the isolation step on a true no-op if it
/// mattered — today every caller re-applies regardless, since re-running
/// `apply_for` on an already-isolated plugin is a cache hit, not real work.
fn edit_isolate_list(root_value: &mut Value, package: &str, add: bool) -> Result<bool> {
    let obj = root_value
        .as_object_mut()
        .context("composer.json must be a JSON object")?;
    let extra = obj
        .entry("extra")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .context("\"extra\" must be a JSON object")?;
    let viv = extra
        .entry("viv")
        .or_insert_with(|| Value::Object(serde_json::Map::new()))
        .as_object_mut()
        .context("\"extra.viv\" must be a JSON object")?;
    let list = viv
        .entry("isolate")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .context("\"extra.viv.isolate\" must be an array")?;

    let mut names: Vec<String> = list
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let changed = if add {
        if names.iter().any(|n| n.eq_ignore_ascii_case(package)) {
            false
        } else {
            names.push(package.to_string());
            true
        }
    } else {
        let before = names.len();
        names.retain(|n| !n.eq_ignore_ascii_case(package));
        names.len() != before
    };
    names.sort();
    names.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
    *list = names.into_iter().map(Value::String).collect();

    if list.is_empty() {
        viv.remove("isolate");
    }
    if viv.is_empty() {
        extra.remove("viv");
    }
    if extra.is_empty() {
        obj.remove("extra");
    }
    Ok(changed)
}

/// `extra.viv.isolate`'s own package names, lowercased — the source of
/// truth for "which plugins get prefixed", read fresh from `root` by both
/// `install`'s per-install hook and [`prefix_map`], never cached.
pub(crate) fn isolated_names(root: &Root) -> Vec<String> {
    root.extra
        .pointer("/viv/isolate")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_ascii_lowercase)
        .collect()
}

/// `extra.viv.isolate`'s names, each mapped to its own derived prefix — pure
/// (no disk I/O, no scoper), so it is cheap enough to recompute on every
/// `install`/`dump-autoload`'s own `State` and always agrees with what
/// `viv isolate --list` last recorded, whether or not the scoper has
/// actually run for a given name yet.
pub(crate) fn prefix_map(root: &Root) -> BTreeMap<String, String> {
    isolated_names(root)
        .into_iter()
        .map(|name| {
            let prefix = prefix_for(&name);
            (name, prefix)
        })
        .collect()
}

/// A package's short name (after the last `/`, or the whole name with no
/// `/`), `PascalCase`d: `-`/`_`/`.`-separated words capitalised and joined,
/// digits left as-is. `wpackagist-plugin/multilingualpress` ->
/// `Multilingualpress`, `acme/wp-migrate-db2` -> `WpMigrateDb2`.
pub(crate) fn slug_to_prefix(name: &str) -> String {
    let slug = name.rsplit('/').next().unwrap_or(name);
    let mut out = String::new();
    for word in slug.split(['-', '_', '.']) {
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// The full namespace prefix a package's name derives, stable across
/// machines since it depends on nothing but the name itself (`docs/research.md`
/// candidate 3.2's own requirement — see issue #351).
pub(crate) fn prefix_for(name: &str) -> String {
    format!("Viv\\Isolated\\{}", slug_to_prefix(name))
}

/// The sibling `.ok` marker for a store dir whose own name can contain dots
/// (a scoper version such as `0.18.17`): [`Path::with_extension`] would
/// mistake the version's own last segment for an extension and corrupt the
/// name, so this builds the sibling by appending to the whole file name
/// instead.
fn isolated_marker(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .expect("an isolated store dir has a name")
        .to_os_string();
    name.push(".ok");
    dest.with_file_name(name)
}

/// `--offline`'s own lookup (#351): the exact key needs the resolved scoper
/// version, which would mean resolving/installing `humbug/php-scoper` to
/// find out — exactly the network access `--offline` forbids. Scanning the
/// bucket for any entry keyed on this plugin's (archive hash, prefix hash)
/// regardless of which scoper version produced it is offline-safe and,
/// since a machine only ever uses one scoper version at a time in
/// practice, as good as the exact key.
fn find_cached_isolated(cache_dir: &Path, key_prefix: &str) -> Result<Option<PathBuf>> {
    let bucket = cache_dir.join(store::ISOLATED_BUCKET);
    let Ok(entries) = fs_err::read_dir(&bucket) else {
        return Ok(None);
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with(key_prefix) && isolated_marker(&entry.path()).is_file() {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// `env_dir`'s own `installed.json`, read for one package's locked
/// version — folded into the `isolated-v0` store key so upgrading
/// `humbug/php-scoper` (a fresh `viv x`/isolate run on a machine that
/// already had an older one cached) builds a new scoped tree instead of
/// silently reusing one an older scoper produced.
fn tool_package_version(env_dir: &Path, name: &str) -> String {
    installed_json_packages_raw(env_dir)
        .into_iter()
        .flatten()
        .find(|entry| entry.get("name").and_then(Value::as_str) == Some(name))
        .and_then(|entry| {
            entry
                .get("version")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// The isolation step for one already-linked plugin (#351): links the
/// store's cached scoped tree over `plugin_dir` if (archive, prefix,
/// scoper version) is already built, otherwise scopes it fresh (network
/// allowed), runs #352's own boot check, and caches the result only once
/// that passes. Called once per plugin `extra.viv.isolate` names, both by
/// `viv isolate` itself (the one package it just edited) and by
/// `install`/`update` (every listed plugin, right after linking). Returns
/// the humbug/php-scoper version this build was checked against (recomputed
/// from the cache entry's own name on a hit, not re-verified — the `.ok`
/// marker only ever exists once), so a caller can record it beside the
/// prefix in `.vivace-state`.
pub(crate) fn apply_for(
    package: &Package,
    plugin_dir: &Path,
    store: &Store,
    cache_dir: &Path,
    php_dir: Option<&Path>,
    link_mode: LinkMode,
    offline: bool,
) -> Result<String> {
    let prefix = prefix_for(&package.name);
    let archive_hash = store
        .lookup(package)
        .and_then(|dir| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
        .with_context(|| format!("{}: no cached archive to isolate", package.pretty_name()))?;
    let prefix_hash = hex(Sha256::digest(prefix.as_bytes()));
    let key_prefix = format!("{archive_hash}-{prefix_hash}-");

    if offline {
        let dest = find_cached_isolated(cache_dir, &key_prefix)?.with_context(|| {
            format!(
                "{}: no isolated build cached, and --offline is set",
                package.pretty_name()
            )
        })?;
        let scoper_version = dest
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.strip_prefix(&key_prefix))
            .unwrap_or("unknown")
            .to_string();
        link::link_tree(&dest, plugin_dir, link_mode)?;
        return Ok(scoper_version);
    }

    // #353: the same project pin `php_dir` above already resolved (its own
    // caller's `php::project_php_dir`), so php-scoper's own version is
    // resolved against it too rather than whatever `php` is on `PATH`.
    let scoper_php_override = php_dir.map(|dir| dir.join("php"));
    let (_, _, scoper_env) = tool::ensure_tool_env(
        "humbug/php-scoper",
        Some(cache_dir),
        offline,
        false,
        scoper_php_override.as_deref(),
    )
    .with_context(|| format!("{}: resolving humbug/php-scoper", package.pretty_name()))?;
    let scoper_version = tool_package_version(&scoper_env, "humbug/php-scoper");
    let key = format!("{key_prefix}{scoper_version}");
    let dest = store.isolated_dir(&key)?;
    let marker = isolated_marker(&dest);

    if !(dest.is_dir() && marker.is_file()) {
        build_scoped_tree(
            package,
            plugin_dir,
            &prefix,
            cache_dir,
            php_dir,
            &scoper_env,
            &dest,
        )?;
        // #352: a scoped plugin that doesn't even boot is worse than the
        // clash it was meant to fix — checked right here, before `dest`
        // is marked `.ok` and becomes a cache hit forever after. A failure
        // drops `dest` entirely (nothing cached, nothing linked); the plain
        // archive `link_tree` below never runs for this plugin this call,
        // so the caller's own `plugin_dir` is left exactly as linking put
        // it before this function was ever called.
        if let Err(err) = boot_check(package, &dest, php_dir) {
            let _ = fs_err::remove_dir_all(&dest);
            return Err(err);
        }
        fs_err::write(&marker, b"")?;
        out(&format!(
            "isolated {} under {prefix}",
            package.pretty_name()
        ));
    }
    link::link_tree(&dest, plugin_dir, link_mode)?;
    Ok(scoper_version)
}

/// Builds `dest` (not yet in the store): a scratch copy of `plugin_dir`,
/// scoped by php-scoper under a generated `scoper.inc.php`, its autoloader
/// regenerated classmap-authoritative (needed because scoping changes a
/// file's namespace without moving the file, breaking the PSR-4
/// dir-matches-namespace assumption a plain autoloader relies on), then
/// renamed into place.
fn build_scoped_tree(
    package: &Package,
    plugin_dir: &Path,
    prefix: &str,
    cache_dir: &Path,
    php_dir: Option<&Path>,
    scoper_env: &Path,
    dest: &Path,
) -> Result<()> {
    let bucket = cache_dir.join(store::ISOLATED_BUCKET);
    fs_err::create_dir_all(&bucket)?;
    let scratch = tempfile::tempdir_in(&bucket)
        .with_context(|| format!("creating a scratch dir under {}", bucket.display()))?;
    let input_dir = scratch.path().join("in");
    let output_dir = scratch.path().join("out");
    copy_dir(plugin_dir, &input_dir).with_context(|| {
        format!(
            "{}: copying {} to scope it",
            package.pretty_name(),
            plugin_dir.display()
        )
    })?;
    // `copy_dir`'s `fs_err::copy` preserves the source's permission bits
    // (std's own documented behaviour) — fine for its other caller
    // (mirroring a path repository), but `plugin_dir` here is a hardlinked
    // store tree (`link::LinkMode::Hardlink` makes vendor files read-only
    // by design), so without this the copy inherits read-only files that
    // neither php-scoper nor `dump_scoped_autoload`'s own rewrite of
    // `vendor/composer/autoload_*.php` can overwrite in place (a plain
    // open-to-write, not `write_atomic`'s rename-over-target).
    make_tree_writable(&input_dir)?;
    write_scoper_config(&input_dir, package, prefix, cache_dir)?;

    let scoper_bin = tool::resolve_bin(scoper_env, "humbug/php-scoper", "php-scoper", None)?;
    let status = std::process::Command::new(&scoper_bin)
        .args([
            "add-prefix",
            "--output-dir",
            &output_dir.to_string_lossy(),
            "--force",
            "--no-interaction",
        ])
        .current_dir(&input_dir)
        .env(
            "PATH",
            php::compose_path(php_dir, &scoper_env.join("vendor/bin")),
        )
        .status()
        .with_context(|| format!("running {}", scoper_bin.display()))?;
    if !status.success() {
        bail!("{}: php-scoper exited with {status}", package.pretty_name());
    }

    dump_scoped_autoload(&output_dir).with_context(|| {
        format!(
            "{}: regenerating the scoped autoloader",
            package.pretty_name()
        )
    })?;

    if dest.is_dir() {
        fs_err::remove_dir_all(dest)?;
    }
    fs_err::create_dir_all(
        dest.parent()
            .expect("the isolated bucket dir is dest's parent"),
    )?;
    fs_err::rename(&output_dir, dest)?;
    Ok(())
}

/// Recursively `chmod`s `dir` so every file is owner-writable (`0o644`) and
/// every directory is traversable and writable (`0o755`) — see
/// [`build_scoped_tree`]'s own call site for why a plain copy of a
/// hardlinked store tree needs this.
fn make_tree_writable(dir: &Path) -> Result<()> {
    for entry in fs_err::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            fs_err::set_permissions(&path, PermissionsExt::from_mode(0o755))?;
            make_tree_writable(&path)?;
        } else {
            fs_err::set_permissions(&path, PermissionsExt::from_mode(0o644))?;
        }
    }
    Ok(())
}

/// A synthetic single-purpose project in `output_dir` (`tool::write_synthetic_root`'s
/// own trick, for a different purpose here): `composer.json`/`composer.lock`
/// built straight from the scoped tree's own `vendor/composer/installed.json`
/// (names/versions php-scoper never touches, only file contents), just
/// enough for `install::dump_autoload` to classmap-scan it — the
/// `exclude-namespaces` plugin code itself was never touched by the
/// scoper, so only its bundled `vendor/` needs a fresh classmap.
/// Classmap-authoritative, not PSR-4, because php-scoper rewrites a file's
/// `namespace` declaration without moving the file: the `installed.json`
/// psr-4 mapping it still carries now points a stale namespace at the
/// right directory. That alone isn't enough, though — Composer's own
/// classmap generator still *validates* a PSR-4/PSR-0-scanned class's
/// namespace against the prefix it was scanned under and silently drops a
/// mismatch (proven the hard way: #352's own load check, run against a
/// first version of this function that left `psr-4` in place, failed every
/// scoped plugin's `class_exists`). [`classmap_safe_autoload`] below
/// rewrites every package's own `autoload` to a plain `classmap` scan of
/// its whole directory before the synthetic lock is ever written, so no
/// scanned class' new, scoped namespace is checked against a mapping that
/// predates the scoper run at all.
fn dump_scoped_autoload(output_dir: &Path) -> Result<()> {
    let mut packages = installed_json_packages_raw(output_dir).unwrap_or_default();
    for package in &mut packages {
        if let Some(entry) = package.as_object_mut() {
            let autoload = classmap_safe_autoload(entry.get("autoload"));
            entry.insert("autoload".to_string(), autoload);
        }
    }
    let lock_json = json!({ "packages": packages, "packages-dev": [] });
    fs_err::write(
        output_dir.join("composer.lock"),
        format!("{}\n", serde_json::to_string_pretty(&lock_json)?),
    )?;
    fs_err::write(output_dir.join("composer.json"), b"{\n}\n")?;

    install::dump_autoload(&DumpAutoloadArgs {
        no_dev: false,
        project_dir: output_dir.to_path_buf(),
        optimize_autoloader: true,
        classmap_authoritative: true,
        apcu_autoloader: false,
        apcu_autoloader_prefix: None,
        ignore_platform_reqs: true,
        ignore_platform_req: Vec::new(),
        no_scripts: true,
        no_normalize: false,
        no_plugins: true,
        no_interaction: true,
    })?;

    // Leave the plugin's install path looking like the plain archive did:
    // no stray `composer.json`/`.lock`, no viv's own `.vivace-state` for a
    // "project" that only ever existed to regenerate one classmap.
    let _ = fs_err::remove_file(output_dir.join("composer.json"));
    let _ = fs_err::remove_file(output_dir.join("composer.lock"));
    let _ = fs_err::remove_file(output_dir.join("vendor/composer/.vivace-state"));
    Ok(())
}

/// `autoload`'s own declared directories (`psr-4`, `psr-0`, `classmap`
/// values alike — the keys, not their namespaces, are what point at a
/// directory worth scanning), reduced to a single `classmap` entry per
/// directory and nothing else, so [`dump_scoped_autoload`]'s own classmap
/// scan accepts every class a directory's files now declare, whatever
/// namespace the scoper rewrote it to. `files`/`exclude-from-classmap`
/// carry over untouched (not namespace-sensitive, no validation to trip on)
/// so a bundled polyfill-style library's global functions still load. A
/// package with no declared directory at all (a plain procedural plugin, or
/// `None`) gets a single `classmap: [""]` covering its own root — a
/// superset scan is harmless; `install::dump_autoload`'s own install-path
/// check already bounds it to the package's own directory.
fn classmap_safe_autoload(autoload: Option<&Value>) -> Value {
    let mut dirs: BTreeSet<String> = BTreeSet::new();
    if let Some(Value::Object(orig)) = autoload {
        for key in ["psr-4", "psr-0", "classmap"] {
            match orig.get(key) {
                Some(Value::Object(map)) => {
                    for paths in map.values() {
                        match paths {
                            Value::String(s) => {
                                dirs.insert(s.clone());
                            }
                            Value::Array(items) => {
                                dirs.extend(
                                    items.iter().filter_map(Value::as_str).map(String::from),
                                );
                            }
                            _ => {}
                        }
                    }
                }
                Some(Value::Array(items)) => {
                    dirs.extend(items.iter().filter_map(Value::as_str).map(String::from));
                }
                _ => {}
            }
        }
    }
    if dirs.is_empty() {
        dirs.insert(String::new());
    }

    let mut out = serde_json::Map::new();
    if let Some(Value::Object(orig)) = autoload {
        for key in ["files", "exclude-from-classmap"] {
            if let Some(value) = orig.get(key) {
                out.insert(key.to_string(), value.clone());
            }
        }
    }
    out.insert(
        "classmap".to_string(),
        Value::Array(dirs.into_iter().map(Value::String).collect()),
    );
    Value::Object(out)
}

// --- #352: a scoped plugin still boots -------------------------------------

/// The minimal `WordPress` function stubs php-scoper's own `exclude-*`
/// config (`write_scoper_config`) assumes are already global when a plugin's
/// main file loads: beside the plugin data files
/// (`src/plugins/data/*`), not under that module's own `rules()` loader —
/// those parse a specific `Rule` shape off `.toml`, this is plain PHP
/// `include_str!`'d whole. Extend the file the first time a real plugin's
/// load check needs a stub it doesn't define.
const WORDPRESS_STUBS: &str = include_str!("plugins/data/wordpress-stubs.php");

/// `php -l` every `.php` file under `dest` (the whole scoped tree php-scoper
/// just wrote, not only `vendor/`) on `php_dir`'s own `php` (the project's
/// pin, same binary [`build_scoped_tree`]'s caller resolved; PATH's `php`
/// when there is none), then a load check: a generated bootstrap, in its own
/// temp dir, that requires [`WORDPRESS_STUBS`], the scoped tree's own
/// `vendor/autoload.php`, then the plugin's own main file
/// ([`plugin_main_file`]) inside a `try`/`catch (\Throwable)` — a fatal
/// (PHP 7+ turns most of those into a catchable `Error`) or an uncaught
/// exception fails the check with PHP's own message. `wp plugin activate`
/// (needs a database) is deliberately not attempted here — see
/// `docs/research.md` candidate 3.2's built verdict.
fn boot_check(package: &Package, dest: &Path, php_dir: Option<&Path>) -> Result<()> {
    let php_bin = php_dir.map_or_else(|| PathBuf::from("php"), |dir| dir.join("php"));

    let mut php_files = Vec::new();
    collect_php_files(dest, &mut php_files, usize::MAX);
    let mut failures = Vec::new();
    for file in &php_files {
        let output = std::process::Command::new(&php_bin)
            .args(["-l", &file.to_string_lossy()])
            .output()
            .with_context(|| format!("running {} -l {}", php_bin.display(), file.display()))?;
        if !output.status.success() {
            failures.push(format!(
                "{}: {}",
                file.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    if !failures.is_empty() {
        bail!(
            "{}: php -l failed on {} file{}:\n{}",
            package.pretty_name(),
            failures.len(),
            if failures.len() == 1 { "" } else { "s" },
            failures.join("\n")
        );
    }

    let main_file = plugin_main_file(dest).with_context(|| {
        format!(
            "{}: no file under {} declares a `Plugin Name:` header to load-check",
            package.pretty_name(),
            dest.display()
        )
    })?;
    let check_dir =
        tempfile::tempdir().context("creating a temp dir for the isolated plugin's load check")?;
    let stubs_path = check_dir.path().join("wordpress-stubs.php");
    fs_err::write(&stubs_path, WORDPRESS_STUBS)?;
    let bootstrap_path = check_dir.path().join("bootstrap.php");
    fs_err::write(
        &bootstrap_path,
        boot_check_source(&stubs_path, &dest.join("vendor/autoload.php"), &main_file),
    )?;
    let output = std::process::Command::new(&php_bin)
        .arg(&bootstrap_path)
        .output()
        .with_context(|| format!("running {} on the load check", php_bin.display()))?;
    if !output.status.success() {
        bail!(
            "{}: load check failed loading {}: {}",
            package.pretty_name(),
            main_file.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// The scoped tree's own main plugin file: the first top-level `.php` file,
/// in name order (never `vendor/`, same scope [`plugin_root_namespaces`]
/// reads), whose contents carry a `Plugin Name:` line — `WordPress`' own
/// convention for a plugin's entry point.
fn plugin_main_file(plugin_dir: &Path) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = fs_err::read_dir(plugin_dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "php"))
        .collect();
    candidates.sort();
    candidates.into_iter().find(|path| {
        fs_err::read(path)
            .is_ok_and(|bytes| String::from_utf8_lossy(&bytes).contains("Plugin Name:"))
    })
}

/// [`boot_check`]'s generated bootstrap source: no config to vary beyond the
/// three paths, so this stays a plain format rather than a templating crate.
fn boot_check_source(stubs_path: &Path, autoload_path: &Path, main_file: &Path) -> String {
    format!(
        "<?php\n\nrequire {};\nrequire {};\n\ntry {{\n    require {};\n}} catch (\\Throwable $e) \
         {{\n    fwrite(STDERR, $e->getMessage() . ' in ' . $e->getFile() . ':' . \
         $e->getLine() . \"\\n\");\n    exit(1);\n}}\n\nexit(0);\n",
        php_string(&stubs_path.to_string_lossy()),
        php_string(&autoload_path.to_string_lossy()),
        php_string(&main_file.to_string_lossy()),
    )
}

/// The plugin's own root namespaces: its `composer.json`'s (if it ships
/// one) `autoload`/`autoload-dev` `psr-4` keys, plus whatever a scan of its
/// own top-level `.php` files (never `vendor/`, never a subdirectory —
/// `docs/research.md`'s own "top-level PHP files" wording) turns up —
/// reused as [`write_scoper_config`]'s `exclude-namespaces`, so prefixing
/// the plugin's bundled `vendor/` never touches the plugin's own code.
fn plugin_root_namespaces(plugin_dir: &Path) -> Vec<String> {
    let mut namespaces: BTreeSet<String> = BTreeSet::new();
    if let Ok(bytes) = fs_err::read(plugin_dir.join("composer.json"))
        && let Ok(value) = serde_json::from_slice::<Value>(&bytes)
    {
        for pointer in ["/autoload/psr-4", "/autoload-dev/psr-4"] {
            if let Some(psr4) = value.pointer(pointer).and_then(Value::as_object) {
                for ns in psr4.keys() {
                    let trimmed = ns.trim_end_matches('\\');
                    if !trimmed.is_empty() {
                        namespaces.insert(trimmed.to_string());
                    }
                }
            }
        }
    }
    let Ok(entries) = fs_err::read_dir(plugin_dir) else {
        return namespaces.into_iter().collect();
    };
    let mut top_level_php: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "php"))
        .collect();
    top_level_php.sort();
    for file in top_level_php.iter().take(NAMESPACE_SAMPLE_CANDIDATES) {
        if let Ok(bytes) = fs_err::read(file)
            && let Some(ns) = first_namespace(&String::from_utf8_lossy(&bytes))
        {
            // The full declared namespace, not just its first segment:
            // `exclude-namespaces` already excludes every sub-namespace of
            // whatever is listed here
            // (`docs/configuration.md#excluding-namespaces`), so a
            // first-segment-only entry (`Inpsyde` for
            // `Inpsyde\MultilingualPress`) would over-exclude any unrelated
            // bundled library that happens to share the same first
            // segment.
            namespaces.insert(ns);
        }
    }
    namespaces.into_iter().collect()
}

/// `sniccowp/php-scoper-wordpress-excludes`'s `generated/*.json` dir, in
/// its own `viv x`-style tool env (Packagist still carries it as of this
/// writing; `docs/further-reading.md#wordpress-support`'s own recipe is
/// `file_get_contents`+`json_decode` straight off these files, reused
/// here by absolute path instead of copying them per plugin). Best-effort:
/// `None` (a warning, not a failure) when it can't be resolved, so a
/// network hiccup or the package someday disappearing from Packagist never
/// blocks isolating a plugin — `WordPress` globals just go unexcluded, same
/// as before this package existed.
fn wordpress_excludes_dir(cache_dir: &Path) -> Option<PathBuf> {
    match tool::ensure_tool_env(
        "sniccowp/php-scoper-wordpress-excludes",
        Some(cache_dir),
        false,
        false,
        None,
    ) {
        Ok((_, _, env_dir)) => {
            let dir = env_dir.join("vendor/sniccowp/php-scoper-wordpress-excludes/generated");
            dir.is_dir().then_some(dir)
        }
        Err(err) => {
            warn_out(&format!(
                "WordPress excludes unavailable ({err:#}); isolating without them"
            ));
            None
        }
    }
}

/// A hand-written scoper config fragment for one plugin's own known edge
/// case (a `class_exists('Foo')` string check php-scoper cannot see
/// through, say): `src/plugins/data/isolate/<slug>.php`, `include_str!`'d
/// in and merged over the generated defaults with `array_replace` — the
/// same embedded-data shape `plugins/data.rs`'s own file list uses. None
/// today: add the data file and a match arm here the first time a real
/// plugin needs one.
fn isolate_override(_slug: &str) -> Option<&'static str> {
    None
}

fn php_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// The generated `scoper.inc.php` php-scoper reads by its own default name
/// (no explicit `--config`, run with `input_dir` as the working dir):
/// `prefix`, `exclude-namespaces` for the plugin's own code (or, when it
/// declares none, the `/^$/` regex excluding the global namespace itself —
/// `docs/configuration.md#excluding-namespaces`'s own documented way to
/// name "the global namespace", since an empty string there excludes
/// everything instead), `WordPress`' own globals via
/// [`wordpress_excludes_dir`] when available, and `expose-global-*` so a
/// bundled library's own global-namespace symbols, if it has any, still
/// resolve under their original names.
fn write_scoper_config(
    input_dir: &Path,
    package: &Package,
    prefix: &str,
    cache_dir: &Path,
) -> Result<()> {
    let root_namespaces = plugin_root_namespaces(input_dir);
    let wp_excludes_dir = wordpress_excludes_dir(cache_dir);
    let override_source =
        isolate_override(package.name.rsplit('/').next().unwrap_or(&package.name));
    let src = scoper_config_source(
        prefix,
        &root_namespaces,
        wp_excludes_dir.as_deref(),
        override_source,
    );
    fs_err::write(input_dir.join("scoper.inc.php"), src)?;
    Ok(())
}

/// [`write_scoper_config`]'s pure half, split out so a test can check the
/// generated config without touching disk or network for
/// `wordpress_excludes_dir`'s own tool-env resolve.
fn scoper_config_source(
    prefix: &str,
    root_namespaces: &[String],
    wp_excludes_dir: Option<&Path>,
    override_source: Option<&str>,
) -> String {
    let mut src = String::from("<?php\n\ndeclare(strict_types=1);\n\n");
    src.push_str(
        "// Generated by `viv isolate` (#351) every time this plugin is re-scoped;\n\
         // hand edits here are lost on the next `viv install`. Ship a per-plugin\n\
         // override instead (src/plugins/data/isolate/<slug>.php in vivace's own\n\
         // source, merged in below).\n\n",
    );
    src.push_str(
        "$load = static function (string $path): array {\n    return is_file($path) ? \
         (json_decode((string) file_get_contents($path), true) ?: []) : [];\n};\n\n",
    );
    match wp_excludes_dir {
        Some(dir) => {
            let _ = writeln!(
                src,
                "$wpClasses = array_merge($load({}), $load({}), $load({}));\n$wpFunctions = \
                 $load({});\n$wpConstants = $load({});\n",
                php_string(
                    dir.join("exclude-wordpress-classes.json")
                        .to_string_lossy()
                        .as_ref()
                ),
                php_string(
                    dir.join("exclude-wordpress-interfaces.json")
                        .to_string_lossy()
                        .as_ref()
                ),
                php_string(
                    dir.join("exclude-wordpress-traits.json")
                        .to_string_lossy()
                        .as_ref()
                ),
                php_string(
                    dir.join("exclude-wordpress-functions.json")
                        .to_string_lossy()
                        .as_ref()
                ),
                php_string(
                    dir.join("exclude-wordpress-constants.json")
                        .to_string_lossy()
                        .as_ref()
                ),
            );
        }
        None => src.push_str("$wpClasses = [];\n$wpFunctions = [];\n$wpConstants = [];\n\n"),
    }

    let mut namespaces: Vec<String> = root_namespaces.iter().map(|ns| php_string(ns)).collect();
    if root_namespaces.is_empty() {
        namespaces.push(php_string("/^$/"));
    }
    let _ = writeln!(
        src,
        "$config = [\n    'prefix' => {},\n    'exclude-namespaces' => [{}],\n    \
         'exclude-classes' => $wpClasses,\n    'exclude-functions' => $wpFunctions,\n    \
         'exclude-constants' => $wpConstants,\n    'expose-global-functions' => true,\n    \
         'expose-global-classes' => true,\n];\n",
        php_string(prefix),
        namespaces.join(", "),
    );
    match override_source {
        Some(code) => {
            let _ = writeln!(
                src,
                "\n$override = (static function () {{\n{code}\n}})();\n\nreturn \
                 array_replace($config, (array) $override);"
            );
        }
        None => src.push_str("\nreturn $config;\n"),
    }
    src
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

    // --- #351: slug -> prefix ----------------------------------------------

    #[test]
    fn slug_to_prefix_pascal_cases_hyphens_and_digits() {
        assert_eq!(slug_to_prefix("multilingualpress"), "Multilingualpress");
        assert_eq!(slug_to_prefix("wp-migrate-db"), "WpMigrateDb");
        assert_eq!(slug_to_prefix("wp-migrate-db2"), "WpMigrateDb2");
        assert_eq!(slug_to_prefix("acme.sub_plugin"), "AcmeSubPlugin");
        // Only the part after the last `/` feeds the prefix.
        assert_eq!(
            slug_to_prefix("wpackagist-plugin/multilingualpress"),
            "Multilingualpress"
        );
    }

    #[test]
    fn prefix_for_namespaces_under_viv_isolated() {
        assert_eq!(
            prefix_for("wpackagist-plugin/multilingualpress"),
            "Viv\\Isolated\\Multilingualpress"
        );
    }

    // --- #351: scoper config generation -------------------------------------

    #[test]
    fn plugin_root_namespaces_reads_composer_json_psr4_and_top_level_php() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("composer.json"),
            json!({"autoload": {"psr-4": {"Acme\\Plugin\\": "src/"}}})
                .to_string()
                .as_bytes(),
        );
        write_file(
            &dir.path().join("bootstrap.php"),
            b"<?php\nnamespace Acme\\Plugin;\n",
        );
        write_file(
            &dir.path().join("vendor/guzzlehttp/guzzle/Client.php"),
            b"<?php\n",
        );

        let namespaces = plugin_root_namespaces(dir.path());
        assert_eq!(namespaces, vec!["Acme\\Plugin".to_string()]);
    }

    #[test]
    fn plugin_root_namespaces_is_empty_for_a_plain_procedural_plugin() {
        let dir = tempfile::tempdir().unwrap();
        write_file(
            &dir.path().join("plugin.php"),
            b"<?php\nfunction acme_init() {}\n",
        );
        assert!(plugin_root_namespaces(dir.path()).is_empty());
    }

    #[test]
    fn scoper_config_excludes_the_plugins_own_namespace_not_the_global_one() {
        let source = scoper_config_source(
            "Viv\\Isolated\\Multilingualpress",
            &["Inpsyde\\MultilingualPress".to_string()],
            None,
            None,
        );
        assert!(source.contains("'prefix' => 'Viv\\\\Isolated\\\\Multilingualpress'"));
        assert!(source.contains("'Inpsyde\\\\MultilingualPress'"));
        assert!(!source.contains("/^$/"));
        assert!(source.contains("'expose-global-functions' => true"));
        assert!(source.contains("'expose-global-classes' => true"));
        assert!(source.contains("return $config;"));
    }

    #[test]
    fn scoper_config_excludes_the_global_namespace_when_the_plugin_declares_none() {
        let source = scoper_config_source("Viv\\Isolated\\Acme", &[], None, None);
        assert!(source.contains("'/^$/'"));
    }

    #[test]
    fn scoper_config_lists_wordpress_excludes_by_absolute_path_when_available() {
        let dir = PathBuf::from("/cache/tools-v0/sniccowp/php-scoper-wordpress-excludes/generated");
        let source = scoper_config_source("Viv\\Isolated\\Acme", &[], Some(&dir), None);
        assert!(source.contains("exclude-wordpress-classes.json"));
        assert!(source.contains("exclude-wordpress-functions.json"));
        assert!(source.contains("exclude-wordpress-constants.json"));
    }

    #[test]
    fn scoper_config_merges_a_per_plugin_override_over_the_defaults() {
        let source = scoper_config_source(
            "Viv\\Isolated\\Acme",
            &[],
            None,
            Some("return ['exclude-classes' => ['Acme_Legacy']];"),
        );
        assert!(source.contains("array_replace($config, (array) $override)"));
        assert!(source.contains("Acme_Legacy"));
    }

    // --- #351: `extra.viv.isolate` composer.json edit -----------------------

    #[test]
    fn edit_isolate_list_adds_sorted_and_deduplicated() {
        let mut root = json!({
            "extra": {"viv": {"isolate": ["wpackagist-plugin/zeta"]}}
        });
        assert!(edit_isolate_list(&mut root, "wpackagist-plugin/alpha", true).unwrap());
        // Adding an already-listed package (any case) is a no-op.
        assert!(!edit_isolate_list(&mut root, "Wpackagist-Plugin/Zeta", true).unwrap());
        assert_eq!(
            root.pointer("/extra/viv/isolate").unwrap(),
            &json!(["wpackagist-plugin/alpha", "wpackagist-plugin/zeta"])
        );
    }

    #[test]
    fn edit_isolate_list_removes_and_drops_empty_extra() {
        let mut root = json!({
            "extra": {"viv": {"isolate": ["wpackagist-plugin/alpha"]}}
        });
        assert!(edit_isolate_list(&mut root, "wpackagist-plugin/alpha", false).unwrap());
        assert!(root.get("extra").is_none());
    }

    #[test]
    fn edit_isolate_list_removing_an_absent_package_is_a_no_op() {
        let mut root = json!({});
        assert!(!edit_isolate_list(&mut root, "wpackagist-plugin/alpha", false).unwrap());
        assert!(root.get("extra").is_none());
    }

    #[test]
    fn isolated_names_and_prefix_map_read_extra_viv_isolate() {
        let root: Root = serde_json::from_value(json!({
            "extra": {"viv": {"isolate": ["Wpackagist-Plugin/Multilingualpress"]}}
        }))
        .unwrap();
        assert_eq!(
            isolated_names(&root),
            vec!["wpackagist-plugin/multilingualpress".to_string()]
        );
        let map = prefix_map(&root);
        assert_eq!(
            map.get("wpackagist-plugin/multilingualpress").unwrap(),
            "Viv\\Isolated\\Multilingualpress"
        );
    }

    #[test]
    fn isolated_marker_is_dot_safe_against_a_dotted_scoper_version() {
        let dest = PathBuf::from("/cache/isolated-v0/abc123-def456-0.18.17");
        assert_eq!(
            isolated_marker(&dest),
            PathBuf::from("/cache/isolated-v0/abc123-def456-0.18.17.ok")
        );
    }
}
