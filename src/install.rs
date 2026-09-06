//! `viv install`: wire lock parsing, planning, fetch/store/link and the
//! autoloader together into the one command vivace v0.1 ships.
//!
//! See `docs/composer-contract.md` for the exact output rules the autoload
//! and installed.* steps defer to; this module only decides *when* to run
//! them and *where* things live on disk.

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Args;
use futures::StreamExt;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::auth::Auth;
use crate::autoload::generator::{self, Input, RootPackage};
use crate::autoload::installed::{installed_json, installed_php};
use crate::autoload::platform::{IgnorePlatform, PlatformInput, platform_check};
use crate::bin;
use crate::fetch;
use crate::link::{LinkMode, link_tree};
use crate::lock::{Lock, Package, Root, read_lock, read_root};
use crate::plan::{self, Plan};
use crate::store::{Store, hex};

/// Fetch requests in flight at once (`fetch::fetch_all`'s concurrency).
const CONCURRENCY: usize = 16;

/// `viv install` flags.
#[derive(Args, Debug, Clone)]
pub struct InstallArgs {
    /// Skip `require-dev` packages.
    #[arg(long)]
    pub no_dev: bool,
    /// Print the plan and stop: no network, no filesystem writes.
    #[arg(long)]
    pub dry_run: bool,
    /// How store files reach `vendor/`.
    #[arg(long, value_enum, default_value = "hardlink")]
    pub link_mode: LinkMode,
    /// Project directory holding `composer.json`/`composer.lock`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
}

/// What a repeat run compares against to recognise a no-op without a
/// network round-trip or a full `installed.json` diff: the lock's
/// `content-hash`, the `--no-dev` flag, and a hash of the root
/// `composer.json` (whose `autoload`/`config` sections feed the generator
/// even when the lock itself has not changed).
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct State {
    content_hash: Option<String>,
    dev: bool,
    composer_json_sha256: String,
}

pub fn run(args: &InstallArgs, cache_dir: Option<&Path>) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;

    let lock_path = project_dir.join("composer.lock");
    if !lock_path.is_file() {
        bail!(
            "composer.lock not found; vivace v0.1 installs from an existing lock, run \
             `composer update` first"
        );
    }
    let root = read_root(&project_dir.join("composer.json")).context("reading composer.json")?;
    let lock = read_lock(&lock_path)?;
    let dev = !args.no_dev;

    let selected: Vec<&Package> = lock.packages(dev).collect();
    for package in &selected {
        package.validate_dist()?;
    }

    let vendor_dir = project_dir.join(&root.config.vendor_dir);
    let plan = plan::plan(&lock, dev, &vendor_dir)?;

    if args.dry_run {
        print_plan(&plan);
        return Ok(());
    }

    let composer_json = fs_err::read(project_dir.join("composer.json"))?;
    let state = State {
        content_hash: lock.content_hash.clone(),
        dev,
        composer_json_sha256: hex(Sha256::digest(&composer_json)),
    };
    let state_path = vendor_dir.join("composer/.vivace-state");
    if plan.is_noop() && read_state(&state_path).as_ref() == Some(&state) {
        out("Nothing to install, update or remove");
        return Ok(());
    }

    let start = Instant::now();
    fs_err::create_dir_all(&vendor_dir)?;

    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => default_cache_dir()?,
    };
    let store = Arc::new(Store::open(&cache_dir)?);

    let mut archive_dirs: HashMap<String, PathBuf> = HashMap::new();
    let mut from_cache = 0usize;
    for package in &plan.install {
        if let Some(dir) = store.lookup(package) {
            archive_dirs.insert(package.name.clone(), dir);
            from_cache += 1;
        }
    }
    let missing: Vec<Package> = plan
        .install
        .iter()
        .filter(|p| !archive_dirs.contains_key(&p.name))
        .cloned()
        .collect();
    if !missing.is_empty() {
        let auth = Auth::load(&project_dir)?;
        let fetcher = fetch::Fetcher::new(auth)?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let downloaded = runtime.block_on(fetch_missing(&fetcher, Arc::clone(&store), &missing))?;
        archive_dirs.extend(downloaded);
    }

    for package in &plan.install {
        let dir = archive_dirs
            .get(&package.name)
            .expect("every install candidate was fetched or found in the store");
        let dest = package_dir(&vendor_dir, package);
        link_tree(dir, &dest, args.link_mode)?;
    }
    for entry in &plan.remove {
        if entry.install_path.exists() {
            fs_err::remove_dir_all(&entry.install_path)?;
        }
    }

    let all: Vec<&Package> = plan.keep.iter().chain(&plan.install).collect();

    let bin_dir = project_dir.join(&root.config.bin_dir);
    let bin_packages: Vec<(&Package, PathBuf)> = all
        .iter()
        .map(|p| (*p, package_dir(&vendor_dir, p)))
        .collect();
    for warning in bin::generate(&vendor_dir, &bin_dir, root.config.bin_compat, &bin_packages)? {
        tracing::warn!("{warning}");
    }

    write_autoload(&root, &lock, &vendor_dir, &project_dir, &all, dev)?;

    write_atomic(
        &vendor_dir.join("composer/installed.json"),
        installed_json(&all, dev)?.as_bytes(),
    )?;
    write_atomic(
        &vendor_dir.join("composer/installed.php"),
        installed_php(&root, &all, dev)?.as_bytes(),
    )?;
    write_atomic(&state_path, &serde_json::to_vec(&state)?)?;

    out(&format!(
        "Installed {} packages ({from_cache} from cache), removed {}, in {:.2}s",
        plan.install.len(),
        plan.remove.len(),
        start.elapsed().as_secs_f64()
    ));
    Ok(())
}

/// Build the generator's `Input` from the root and the packages that will
/// end up in `vendor/`, generate the autoload files, then write
/// `platform_check.php` (or delete it) beside them.
fn write_autoload(
    root: &Root,
    lock: &Lock,
    vendor_dir: &Path,
    project_dir: &Path,
    packages: &[&Package],
    dev: bool,
) -> Result<()> {
    let suffix = resolve_suffix(root, lock, vendor_dir)?;
    let root_name = root.name.clone().unwrap_or_else(|| "__root__".into());

    let empty = Map::new();
    let mut platform_inputs = vec![PlatformInput {
        name: &root_name,
        dev: false,
        require: &root.require,
        provide: &empty,
        replace: &empty,
    }];
    platform_inputs.extend(packages.iter().map(|p| PlatformInput {
        name: &p.name,
        dev: p.dev,
        require: &p.require,
        provide: &p.provide,
        replace: &p.replace,
    }));
    let platform_body = platform_check(
        &platform_inputs,
        root.config.platform_check,
        &IgnorePlatform::None,
    )?;

    let keys = |map: &Map<String, Value>| map.keys().cloned().collect::<Vec<String>>();
    let input = Input {
        root: RootPackage {
            name: root_name,
            autoload: root.autoload.clone().unwrap_or(Value::Null),
            autoload_dev: root.autoload_dev.clone().unwrap_or(Value::Null),
            target_dir: None,
            requires: keys(&root.require),
        },
        packages: packages
            .iter()
            .map(|p| generator::Package {
                name: p.name.clone(),
                autoload: p.autoload.clone().unwrap_or(Value::Null),
                requires: keys(&p.require),
                replaces: keys(&p.replace),
                provides: keys(&p.provide),
                target_dir: p.target_dir.clone(),
                install_path: (p.r#type != "metapackage").then(|| package_dir(vendor_dir, p)),
                is_dev: p.dev,
            })
            .collect(),
        dev_mode: dev,
        scan_psr: false,
        suffix,
        vendor_dir: vendor_dir.to_path_buf(),
        base_dir: project_dir.to_path_buf(),
        platform_check: platform_body.is_some(),
        prepend_autoloader: root.config.prepend_autoloader,
    };
    let generated = generator::generate(&input)?;
    for warning in &generated.warnings {
        tracing::warn!("{warning}");
    }

    let platform_check_path = vendor_dir.join("composer/platform_check.php");
    match platform_body {
        Some(body) => write_atomic(&platform_check_path, body.as_bytes())?,
        None if platform_check_path.exists() => fs_err::remove_file(&platform_check_path)?,
        None => {}
    }
    Ok(())
}

/// Download every package not already in the store, extracting each into it
/// as its bytes arrive.
async fn fetch_missing(
    fetcher: &fetch::Fetcher,
    store: Arc<Store>,
    packages: &[Package],
) -> Result<HashMap<String, PathBuf>> {
    let mut stream = fetcher.fetch_all(packages, CONCURRENCY);
    let mut result = HashMap::new();
    while let Some((package, bytes)) = stream.next().await {
        let bytes = bytes.with_context(|| format!("{}: fetching dist", package.name))?;
        let name = package.name.clone();
        let package = package.clone();
        let store = Arc::clone(&store);
        let dir = tokio::task::spawn_blocking(move || store.add_zip(&package, &bytes))
            .await
            .context("store worker panicked")??;
        result.insert(name, dir);
    }
    Ok(result)
}

/// `vendor/<name>`, plus the legacy `target-dir` nesting when the package
/// still declares one.
fn package_dir(vendor_dir: &Path, package: &Package) -> PathBuf {
    let mut dir = vendor_dir.join(&package.name);
    if let Some(target) = package.target_dir.as_deref().filter(|t| !t.is_empty()) {
        dir = dir.join(target);
    }
    dir
}

fn print_plan(plan: &Plan) {
    out(&format!(
        "install {} / keep {} / remove {}",
        plan.install.len(),
        plan.keep.len(),
        plan.remove.len()
    ));
    for package in &plan.install {
        out(&format!("  install {} ({})", package.name, package.version));
    }
    for package in &plan.keep {
        out(&format!("  keep {} ({})", package.name, package.version));
    }
    for entry in &plan.remove {
        out(&format!("  remove {}", entry.name));
    }
}

/// `config.autoloader-suffix`, else the suffix already in
/// `vendor/autoload.php`, else the lock's `content-hash` when it looks like
/// hex, else 32 random hex characters.
fn resolve_suffix(root: &Root, lock: &Lock, vendor_dir: &Path) -> Result<String> {
    if let Some(suffix) = &root.config.autoloader_suffix {
        return Ok(suffix.clone());
    }
    if let Ok(existing) = fs_err::read_to_string(vendor_dir.join("autoload.php")) {
        static SUFFIX_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
        let re = SUFFIX_RE
            .get_or_init(|| Regex::new(r"ComposerAutoloaderInit([^:\s]+)::").expect("valid regex"));
        if let Some(caps) = re.captures(&existing) {
            return Ok(caps[1].to_string());
        }
    }
    if let Some(hash) = &lock.content_hash {
        let looks_lowercase_hex = !hash.is_empty()
            && hash
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase());
        if looks_lowercase_hex {
            return Ok(hash.clone());
        }
    }
    random_hex32()
}

/// 32 random hex characters read from `/dev/urandom` (Linux-only, matches
/// the rest of vivace; no extra dependency for this one call).
fn random_hex32() -> Result<String> {
    let mut bytes = [0u8; 16];
    std::io::Read::read_exact(&mut fs_err::File::open("/dev/urandom")?, &mut bytes)?;
    Ok(hex(bytes))
}

/// `$XDG_CACHE_HOME/vivace`, falling back to `~/.cache/vivace`.
fn default_cache_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("vivace"));
    }
    let home = std::env::var("HOME").context("HOME is not set; pass --cache-dir")?;
    Ok(PathBuf::from(home).join(".cache").join("vivace"))
}

fn read_state(path: &Path) -> Option<State> {
    let content = fs_err::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write `content` to `path` only when it differs, via a temp file in the
/// same directory renamed over the target, so a reader never sees a partial
/// write.
fn write_atomic(path: &Path, content: &[u8]) -> Result<()> {
    if fs_err::read(path).is_ok_and(|existing| existing == content) {
        return Ok(());
    }
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

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}
