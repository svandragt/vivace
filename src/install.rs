//! `viv install`: wire lock parsing, planning, fetch/store/link and the
//! autoloader together into `viv`'s core command.
//!
//! See `docs/composer-contract.md` for the exact output rules the autoload
//! and installed.* steps defer to; this module only decides *when* to run
//! them and *where* things live on disk.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::{IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
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
use crate::lock::{
    self, Lock, MISSING_REQUIREMENTS_HINT, Package, Root, STALE_LOCK_WARNING, read_lock,
};
use crate::plan::{self, Plan};
use crate::plugins;
use crate::scripts;
use crate::source;
use crate::store::{Store, hex};
use crate::vcs;

/// Fetch requests in flight at once (`fetch::fetch_all`'s concurrency).
/// Downloads are latency-bound (a GitHub zipball round-trip, not vivace's
/// CPU), so raising this shortens a cold install close to linearly until the
/// remote host's own limits take over; 16 measured well short of that knee
/// on a 101-package lock, 64 measured near it with low run-to-run variance.
const CONCURRENCY: usize = 64;

/// `viv install` flags.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's install flags"
)]
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
    /// Reinstall every locked package from the store even if `installed.json`
    /// already matches it, e.g. to relink a `vendor/` Composer (or an older
    /// viv) wrote as plain copies. Prompts for confirmation when stdin is a
    /// terminal (skip with a non-interactive stdin, e.g. `</dev/null`).
    #[arg(long)]
    pub adopt: bool,
    /// Project directory holding `composer.json`/`composer.lock`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Also classmap-scan PSR-0/PSR-4 directories (`config.optimize-autoloader`).
    #[arg(short = 'o', long = "optimize-autoloader")]
    pub optimize_autoloader: bool,
    /// Classmap-only autoloading, no PSR-0/PSR-4 fallback at runtime
    /// (`config.classmap-authoritative`); implies `-o`.
    #[arg(short = 'a', long = "classmap-authoritative")]
    pub classmap_authoritative: bool,
    /// Cache classmap lookups in `APCu` (`config.apcu-autoloader`).
    #[arg(long = "apcu-autoloader")]
    pub apcu_autoloader: bool,
    /// Fixed `APCu` cache-key prefix, instead of one generated per run
    /// (`config.apcu-autoloader-prefix`); implies `--apcu-autoloader`.
    #[arg(long = "apcu-autoloader-prefix", value_name = "PREFIX")]
    pub apcu_autoloader_prefix: Option<String>,
    /// Skip `pre-install-cmd`/`post-install-cmd`/`post-autoload-dump` and
    /// every other root `scripts` listener.
    #[arg(long)]
    pub no_scripts: bool,
    /// No-op since 0.6 (#95): `install` never writes `composer.json`, so
    /// there is nothing left to skip. Kept, hidden, for one release so an
    /// old invocation doesn't fail; prints a deprecation warning instead.
    #[arg(long, hide = true)]
    pub no_normalize: bool,
    /// Install every package under `vendor/`, as Composer does with the same
    /// flag: the native `composer/installers`/`wordpress-core-installer`
    /// adapters are disabled, and any other enabled plugin viv would
    /// otherwise refuse only warns (`docs/plugin-strategy.md`).
    #[arg(long)]
    pub no_plugins: bool,
}

/// `viv dump-autoload` flags: same autoload-shaping knobs as `install`, minus
/// `--dry-run`/`--link-mode`, which only make sense when fetching and
/// linking.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's dump-autoload flags"
)]
#[derive(Args, Debug, Clone)]
pub struct DumpAutoloadArgs {
    /// Skip `require-dev` packages.
    #[arg(long)]
    pub no_dev: bool,
    /// Project directory holding `composer.json`/`composer.lock`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Also classmap-scan PSR-0/PSR-4 directories (`config.optimize-autoloader`).
    #[arg(short = 'o', long = "optimize-autoloader")]
    pub optimize_autoloader: bool,
    /// Classmap-only autoloading, no PSR-0/PSR-4 fallback at runtime
    /// (`config.classmap-authoritative`); implies `-o`.
    #[arg(short = 'a', long = "classmap-authoritative")]
    pub classmap_authoritative: bool,
    /// Cache classmap lookups in `APCu` (`config.apcu-autoloader`).
    #[arg(long = "apcu-autoloader")]
    pub apcu_autoloader: bool,
    /// Fixed `APCu` cache-key prefix, instead of one generated per run
    /// (`config.apcu-autoloader-prefix`); implies `--apcu-autoloader`.
    #[arg(long = "apcu-autoloader-prefix", value_name = "PREFIX")]
    pub apcu_autoloader_prefix: Option<String>,
    /// Skip `pre-autoload-dump`/`post-autoload-dump` and every other root
    /// `scripts` listener.
    #[arg(long)]
    pub no_scripts: bool,
    /// No-op since 0.6 (#95): `dump-autoload` never writes `composer.json`,
    /// so there is nothing left to skip. Kept, hidden, for one release so an
    /// old invocation doesn't fail; prints a deprecation warning instead.
    #[arg(long, hide = true)]
    pub no_normalize: bool,
    /// Regenerate every package's autoload entry at its plain `vendor/`
    /// location: the native installer adapters are disabled, same as
    /// `install --no-plugins` (`docs/plugin-strategy.md`).
    #[arg(long)]
    pub no_plugins: bool,
}

/// `viv cache` flags: which cache maintenance operation to run.
#[derive(Args, Debug, Clone)]
pub struct CacheArgs {
    #[command(subcommand)]
    pub command: CacheCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum CacheCommand {
    /// Remove stale buckets, orphan temp dirs, orphan `.ok` markers, and any
    /// archive no dist pointer references any more.
    Prune {
        /// Also remove dist pointers not installed from in this many days
        /// (touched on every `viv install` hit), before sweeping archives
        /// that leaves unreferenced.
        #[arg(long, value_name = "DAYS")]
        older_than: Option<u64>,
    },
    /// Remove the whole cache after confirming it looks like a vivace cache
    /// (only our own bucket names, or empty); refuses otherwise.
    Clean,
    /// Print archive and dist-pointer counts and total size.
    Size,
}

/// The autoload-shaping flags `install` and `dump-autoload` both accept,
/// decoupled from either's `clap::Args` so `write_autoload` takes one shared
/// type instead of `InstallArgs` only.
struct AutoloadFlags {
    optimize_autoloader: bool,
    classmap_authoritative: bool,
    apcu_autoloader: bool,
    apcu_autoloader_prefix: Option<String>,
}

impl From<&InstallArgs> for AutoloadFlags {
    fn from(args: &InstallArgs) -> Self {
        AutoloadFlags {
            optimize_autoloader: args.optimize_autoloader,
            classmap_authoritative: args.classmap_authoritative,
            apcu_autoloader: args.apcu_autoloader,
            apcu_autoloader_prefix: args.apcu_autoloader_prefix.clone(),
        }
    }
}

impl From<&DumpAutoloadArgs> for AutoloadFlags {
    fn from(args: &DumpAutoloadArgs) -> Self {
        AutoloadFlags {
            optimize_autoloader: args.optimize_autoloader,
            classmap_authoritative: args.classmap_authoritative,
            apcu_autoloader: args.apcu_autoloader,
            apcu_autoloader_prefix: args.apcu_autoloader_prefix.clone(),
        }
    }
}

/// What a repeat run compares against to recognise a no-op without a
/// network round-trip or a full `installed.json` diff: the lock's
/// `content-hash`, the `--no-dev` flag, and a hash of the root
/// `composer.json` (whose `autoload`/`config` sections feed the generator
/// even when the lock itself has not changed).
///
/// `patches_fingerprint` (#53) covers the one input the other three miss:
/// `cweagans/composer-patches` reads its own patches from `extra.patches`
/// (already inside `composer_json_sha256`, so covered) *and* a separate
/// `patches-file` (`patches.json` by default) that isn't part of
/// `composer.json`, `composer.lock`, or `--no-dev` at all. Without it, editing
/// only that file would hit the fast no-op path below and never reach
/// `Plugins::apply_patches`, leaving a stale patch applied (or a new one
/// un-applied). `None` when the plugin isn't active, so a project without it
/// pays nothing extra.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct State {
    content_hash: Option<String>,
    dev: bool,
    composer_json_sha256: String,
    #[serde(default)]
    patches_fingerprint: Option<String>,
}

pub fn run(args: &InstallArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    run_impl(args, cache_dir, offline, true)
}

/// `update`/`require`/`remove` chaining into `install` once `composer.lock`
/// (and, for `require`/`remove`, `composer.json`) is written, the way
/// `composer update`/`require`/`remove` all chain into the same
/// `Installer::run()` with `update` set: the caller already dispatched
/// `pre-update-cmd` before resolving and dispatches `post-update-cmd` once
/// this returns, in place of `install`'s own `pre-install-cmd`/
/// `post-install-cmd` (`Installer::run`'s own event-name switch on its
/// `update` flag — Composer never fires both pairs for one invocation).
pub fn run_after_update(args: &InstallArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    run_impl(args, cache_dir, offline, false)
}

fn run_impl(
    args: &InstallArgs,
    cache_dir: Option<&Path>,
    offline: bool,
    dispatch_install_cmd_events: bool,
) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;

    let lock_path = project_dir.join("composer.lock");
    if !lock_path.is_file() {
        bail!(
            "composer.lock not found; viv installs from an existing composer.lock, run \
             `viv update` to create one"
        );
    }

    // `install` never writes `composer.json` (#95): normalizing moved to
    // `require`/`remove`/`update`, the commands that already rewrite it.
    if args.no_normalize {
        warn_no_normalize_is_a_noop("install");
    }
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root = lock::parse_root(&composer_json).context("parsing composer.json")?;
    let read_lock_started = Instant::now();
    let mut lock = read_lock(&lock_path)?;
    tracing::debug!(
        packages = lock.packages.len(),
        elapsed_ms = read_lock_started.elapsed().as_millis(),
        "read and parsed composer.lock"
    );
    let dev = !args.no_dev;

    let plugins_started = Instant::now();
    let (plugins, plugin_warnings) = plugins::resolve(&lock, &root, args.no_plugins)?;
    tracing::debug!(
        elapsed_ms = plugins_started.elapsed().as_millis(),
        "resolved plugins"
    );
    for warning in &plugin_warnings {
        warn_out(warning);
    }
    // `config.preferred-install` (#43): a package with both a dist and a
    // git `source` may still be checked out from source instead of fetched
    // as an archive, per `DownloadManager::resolvePackageInstallPreference`.
    let preferred_install = lock::resolve_preferred_install(&root.config.preferred_install)?;
    for package in &mut lock.packages {
        package.install_dir = plugins.install_dir(&root, package);
        package.install_from_source = package.dist.is_some()
            && package.source.as_ref().is_some_and(|s| s.r#type == "git")
            && preferred_install
                .prefers_source(&package.name, lock::is_dev_version(&package.version));
    }

    // Composer's `Installer::doInstall`: a stale content-hash is only a
    // warning, but a requirement entirely absent from the lock is fatal
    // (`ERROR_LOCK_FILE_INVALID`) — vivace has no solver to tell "missing"
    // from "present but doesn't satisfy the constraint", so it only catches
    // the former, same as `missing_requirements`'s own doc comment.
    if !lock::is_fresh(&lock, &composer_json)? {
        warn_out(STALE_LOCK_WARNING);
    }
    let missing = lock::missing_requirements(&lock, &root, dev);
    if !missing.is_empty() {
        let mut lines = missing;
        lines.extend(MISSING_REQUIREMENTS_HINT.iter().map(|s| (*s).to_string()));
        bail!(lines.join("\n"));
    }

    let selected: Vec<&Package> = lock.packages(dev).collect();
    for package in &selected {
        package.validate_dist()?;
    }

    let vendor_dir = project_dir.join(&root.config.vendor_dir);
    let plan_started = Instant::now();
    let mut plan = plan::plan(&lock, dev, &vendor_dir, &project_dir)?;
    tracing::debug!(
        install = plan.install.len(),
        keep = plan.keep.len(),
        remove = plan.remove.len(),
        elapsed_ms = plan_started.elapsed().as_millis(),
        "diffed lock against installed.json"
    );

    let state_path = vendor_dir.join("composer/.vivace-state");
    // installed.json exists but viv never wrote a state file: vendor/ came
    // from Composer (or a pre-adopt viv) as plain copies, not store links.
    let composer_written =
        vendor_dir.join("composer/installed.json").is_file() && !state_path.is_file();
    // `--adopt` force-relinks a viv-written vendor/ on request; a
    // Composer-written vendor/ is adopted the same way automatically, no
    // flag needed (#123).
    let adopting = args.adopt || composer_written;
    let mut adopted_count = 0usize;
    // Packages adopted automatically (never `--adopt`, which keeps today's
    // hard failure) fetch best-effort: a private dist's archive already sits
    // in `vendor/` as Composer's own copy, so a fetch failure for one of
    // these just keeps that copy in place with a warning instead of failing
    // the whole install (#123 follow-up).
    let mut best_effort_adopt: HashSet<String> = HashSet::new();
    if adopting {
        // `--adopt` always gives a human at a terminal a chance to back out
        // of relinking every kept package in place. The automatic adopt only
        // prompts when the install came through the `composer` shim
        // (`VIV_VIA_SHIM`): a user typing `composer install` didn't opt into
        // viv touching their tree in place, but a plain `viv install` or a
        // script under the shim proceeds unprompted, same as before this
        // existed.
        let should_prompt = std::io::stdin().is_terminal()
            && (args.adopt || std::env::var_os("VIV_VIA_SHIM").is_some());
        if should_prompt && !confirm_adopt()? {
            bail!("Aborted");
        }
        let adopted_names: HashSet<String> = plan
            .keep
            .iter()
            .filter(|p| p.r#type != "metapackage")
            .map(|p| p.name.clone())
            .collect();
        adopted_count = adopted_names.len();
        if !args.adopt {
            best_effort_adopt = adopted_names;
        }
        plan.install.append(&mut plan.keep);
    }

    if args.dry_run {
        print_plan(&plan);
        return Ok(());
    }

    let state = State {
        content_hash: lock.content_hash.clone(),
        dev,
        composer_json_sha256: hex(Sha256::digest(&composer_json)),
        patches_fingerprint: patches_fingerprint(&plugins, &root, &project_dir)?,
    };
    let composer_json_value: Value =
        serde_json::from_slice(&composer_json).context("parsing composer.json")?;
    let mut scripts = scripts::Runner::new(
        &composer_json_value,
        &project_dir,
        &root.config.bin_dir(),
        dev,
        args.no_scripts,
    );
    // Composer still dispatches every event on a no-op install (and its
    // autoloader regeneration is a plain re-dump, cheap because
    // `write_atomic` only rewrites bytes that changed); vivace's no-op fast
    // path skips all of that for speed, which is only safe when there is no
    // `scripts` listener relying on running anyway.
    if plan.is_noop() && read_state(&state_path).as_ref() == Some(&state) && !scripts.enabled() {
        out("Nothing to install, update or remove");
        return Ok(());
    }
    let scripts_started = Instant::now();
    if dispatch_install_cmd_events {
        scripts.dispatch("pre-install-cmd")?;
    }
    tracing::debug!(
        elapsed_ms = scripts_started.elapsed().as_millis(),
        "dispatched pre-install-cmd"
    );

    let start = Instant::now();
    fs_err::create_dir_all(&vendor_dir)?;

    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    let store = Arc::new(Store::open(&cache_dir)?);

    // Path (#13's local-directory case), dist-less git-source (#13's VCS
    // case) and preferred-install-source (#43, `install_from_source`, set
    // above) packages are symlinked/mirrored or cloned straight into
    // `vendor/`, below; only zip/tar dists ever reach the store or fetcher.
    let archive_targets: Vec<&Package> = plan
        .install
        .iter()
        .filter(|p| {
            p.r#type != "metapackage"
                && !p.is_path()
                && !p.is_git_source()
                && !p.install_from_source
        })
        .collect();

    let mut archive_dirs: HashMap<String, PathBuf> = HashMap::new();
    let mut from_cache = 0usize;
    for package in &archive_targets {
        if let Some(dir) = store.lookup(package) {
            archive_dirs.insert(package.name.clone(), dir);
            from_cache += 1;
        }
    }
    let missing: Vec<Package> = archive_targets
        .iter()
        .filter(|p| !archive_dirs.contains_key(&p.name))
        .map(|p| (*p).clone())
        .collect();
    if !missing.is_empty() {
        // `--offline`/`COMPOSER_DISABLE_NETWORK` (#23): report every missing
        // package in one message, rather than let the fetcher fail on
        // whichever one it happens to reach first.
        if offline {
            let mut message = format!(
                "Network disabled, request canceled: not in the store ({} package{}):",
                missing.len(),
                if missing.len() == 1 { "" } else { "s" }
            );
            for package in &missing {
                let _ = write!(message, "\n  {} ({})", package.name, package.version);
            }
            bail!(message);
        }
        let auth = Auth::load(&project_dir)?;
        let mut fetcher = fetch::Fetcher::new(auth)?.secure_http(root.config.secure_http);
        if plugins.has_private_installer() {
            fetcher = fetcher.private_installer(crate::plugins::private_installer::Env::load(
                &root,
                &project_dir,
            ));
        }
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let fetch_started = Instant::now();
        let downloaded = runtime.block_on(fetch_missing(
            &fetcher,
            Arc::clone(&store),
            &missing,
            &best_effort_adopt,
        ))?;
        tracing::debug!(
            packages = missing.len(),
            elapsed_ms = fetch_started.elapsed().as_millis(),
            "fetched and stored missing packages"
        );
        fetcher.log_hop_summary();
        archive_dirs.extend(downloaded);
    }
    // A best-effort adopt that couldn't fetch stays a Composer copy, not a
    // relinked package: `fetch_missing` already warned for it, so it's
    // dropped from the count of packages actually adopted, and (below) from
    // the list `link_archives` touches, leaving `vendor/` untouched for it.
    adopted_count -= best_effort_adopt
        .iter()
        .filter(|name| {
            missing.iter().any(|p| &p.name == *name) && !archive_dirs.contains_key(*name)
        })
        .count();

    sweep_link_litter(&vendor_dir)?;
    let link_started = Instant::now();
    // Path/git-source installs stay sequential: there are typically few of
    // them, and a git checkout shares a per-URL mirror in `cache_dir` that
    // isn't safe to clone into concurrently from two threads. Archive-backed
    // packages (the common case) are the ones extraction already fans out
    // 8-way, so linking them fans out the same way instead of serialising
    // what was already parallel disk I/O up to this point.
    let mut archive_installs: Vec<&Package> = Vec::new();
    for package in &plan.install {
        // Composer's MetapackageInstaller installs nothing (no dir, no dist
        // fetch); mirror that instead of downloading/linking a dist a
        // metapackage happens to declare.
        if package.r#type == "metapackage" {
            continue;
        }
        if package.is_path() || package.is_git_source() || package.install_from_source {
            let dest = package_dir(&vendor_dir, &project_dir, package);
            if package.is_path() {
                source::install_path(&project_dir, package, &dest)?;
            } else {
                source::checkout_git(&cache_dir, package, &dest)?;
            }
        } else if archive_dirs.contains_key(&package.name) {
            archive_installs.push(package);
        }
        // Else: a best-effort adopt whose fetch failed (`archive_dirs` has no
        // entry for it) — leave it out of `archive_installs` entirely so
        // `link_archives` never touches it, keeping Composer's own copy on
        // disk exactly as it was.
    }
    link_archives(
        &archive_installs,
        &vendor_dir,
        &project_dir,
        &archive_dirs,
        args.link_mode,
    )?;
    for entry in &plan.remove {
        if entry.install_path.exists() {
            fs_err::remove_dir_all(&entry.install_path)?;
            // Composer prunes now-empty parents after removing a package
            // (`vendor/<vendor>/` disappears when its last package goes; a
            // target-dir package's scaffold parents go the same way) —
            // bounded at `vendor_dir` for an ordinary package, or
            // `project_dir` for one a native installer mapped elsewhere.
            let boundary = if entry.install_path.starts_with(&vendor_dir) {
                vendor_dir.as_path()
            } else {
                project_dir.as_path()
            };
            prune_empty_ancestors(&entry.install_path, boundary);
        }
    }
    tracing::debug!(
        packages = plan.install.len(),
        elapsed_ms = link_started.elapsed().as_millis(),
        "linked packages into vendor"
    );

    // #53: cweagans/composer-patches. Runs before the autoloader is
    // (re)generated, same as Composer's own POST_PACKAGE_INSTALL/UPDATE —
    // a patch can add or remove classes the classmap needs to see.
    if plugins.has_patches() {
        plugins.apply_patches(
            &root,
            &project_dir,
            &vendor_dir,
            &plan.install,
            &plan.keep,
            &store,
        )?;
    }

    let all: Vec<&Package> = plan.keep.iter().chain(&plan.install).collect();
    // #93: drupal/core-composer-scaffold. Same phase as the patches above —
    // install directories are known, the autoloader isn't regenerated yet.
    plugins.apply_scaffold(&root, &project_dir, &vendor_dir, &all)?;

    let flags = AutoloadFlags::from(args);
    regenerate_vendor_metadata(
        &flags,
        &root,
        &lock,
        &vendor_dir,
        &project_dir,
        &all,
        dev,
        &state,
        &state_path,
        &mut scripts,
        Some(&archive_dirs),
        &plugins,
        true,
    )?;

    if plan.is_noop() {
        // Composer's own wording for this case (only reached here because a
        // stale state or `scripts` listener skipped the earlier fast path):
        // nothing installed or removed, so no package counts to report.
        out("Nothing to install, update or remove");
        let optimized = args.optimize_autoloader
            || args.classmap_authoritative
            || root.config.optimize_autoloader;
        out(if optimized {
            "Generating optimized autoload files"
        } else {
            "Generating autoload files"
        });
    } else {
        let installed_count = plan
            .install
            .iter()
            .filter(|p| p.r#type != "metapackage")
            .count();
        let adopted_suffix = if adopted_count > 0 {
            format!(", adopted {adopted_count} packages from a Composer install")
        } else {
            String::new()
        };
        out(&format!(
            "Installed {installed_count} packages ({from_cache} from cache), removed {}, in \
             {:.2}s{adopted_suffix}",
            plan.remove.len(),
            start.elapsed().as_secs_f64()
        ));
    }
    let scripts_started = Instant::now();
    if dispatch_install_cmd_events {
        scripts.dispatch("post-install-cmd")?;
    }
    tracing::debug!(
        elapsed_ms = scripts_started.elapsed().as_millis(),
        "dispatched post-install-cmd"
    );
    Ok(())
}

/// The tail `install` runs after fetch/link, and all `dump-autoload` runs:
/// `vendor/bin` proxies, the autoload files, `installed.json`/`installed.php`
/// and the `.vivace-state` marker, all derived from `packages` alone (no
/// fetching or linking here).
#[expect(clippy::too_many_arguments, reason = "install's tail, no bundling win")]
fn regenerate_vendor_metadata(
    flags: &AutoloadFlags,
    root: &Root,
    lock: &Lock,
    vendor_dir: &Path,
    project_dir: &Path,
    packages: &[&Package],
    dev: bool,
    state: &State,
    state_path: &Path,
    scripts: &mut scripts::Runner,
    archive_dirs: Option<&HashMap<String, PathBuf>>,
    plugins: &plugins::Plugins,
    is_install: bool,
) -> Result<()> {
    let bin_dir = project_dir.join(root.config.bin_dir());
    let bin_packages: Vec<(&Package, PathBuf)> = packages
        .iter()
        .filter(|p| p.r#type != "metapackage")
        .map(|p| (*p, package_dir(vendor_dir, project_dir, p)))
        .collect();
    let bin_started = Instant::now();
    for warning in bin::generate(vendor_dir, &bin_dir, root.config.bin_compat, &bin_packages)? {
        tracing::warn!("{warning}");
    }
    tracing::debug!(
        elapsed_ms = bin_started.elapsed().as_millis(),
        "generated vendor/bin"
    );

    let autoload_started = Instant::now();
    write_autoload(
        flags,
        root,
        lock,
        vendor_dir,
        project_dir,
        packages,
        dev,
        scripts,
        archive_dirs,
        plugins,
        &bin_packages,
    )?;
    tracing::debug!(
        elapsed_ms = autoload_started.elapsed().as_millis(),
        "generated autoload files"
    );

    // `dealerdirect/phpcodesniffer-composer-installer` and
    // `phpstan/extension-installer` both subscribe to `post-install-cmd`/
    // `post-update-cmd` only, never a bare `dump-autoload`
    // (`plugins::Plugins::apply_post_install`'s doc comment).
    if is_install {
        plugins.apply_post_install(root, &bin_packages)?;
    }

    let installed_started = Instant::now();
    write_atomic(
        &vendor_dir.join("composer/installed.json"),
        installed_json(packages, dev)?.as_bytes(),
    )?;
    // Only shells out to `git` when `composer.json` has no explicit
    // `version` (#125) — an explicit one always wins, same as Composer's
    // own `VersionGuesser::guessVersion`.
    let git_version = root
        .version
        .is_none()
        .then(|| vcs::guess_root_version(project_dir))
        .flatten();
    write_atomic(
        &vendor_dir.join("composer/installed.php"),
        installed_php(root, lock, packages, dev, git_version.as_ref())?.as_bytes(),
    )?;
    write_atomic(state_path, &serde_json::to_vec(state)?)?;
    tracing::debug!(
        elapsed_ms = installed_started.elapsed().as_millis(),
        "wrote installed.json/php and state"
    );
    Ok(())
}

/// `viv dump-autoload`: reread `composer.json`/`composer.lock` and regenerate
/// `vendor/autoload.php`, `vendor/composer/*` and `vendor/bin` from the
/// packages already linked into `vendor/` — no fetch, no link, no remove.
pub fn dump_autoload(args: &DumpAutoloadArgs) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;

    let lock_path = project_dir.join("composer.lock");
    if !lock_path.is_file() {
        bail!(
            "composer.lock not found; viv installs from an existing composer.lock, run \
             `viv update` to create one"
        );
    }
    // Composer's `DumpAutoloadCommand` never checks lock freshness or
    // missing requirements (unlike `InstallCommand`), so neither does this.
    // `dump-autoload` never writes `composer.json` either (#95): see
    // `run`'s own comment.
    if args.no_normalize {
        warn_no_normalize_is_a_noop("dump-autoload");
    }
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root = lock::parse_root(&composer_json).context("parsing composer.json")?;
    let mut lock = read_lock(&lock_path)?;
    let dev = !args.no_dev;

    let (plugins, plugin_warnings) = plugins::resolve(&lock, &root, args.no_plugins)?;
    for warning in &plugin_warnings {
        warn_out(warning);
    }
    for package in &mut lock.packages {
        package.install_dir = plugins.install_dir(&root, package);
    }

    let vendor_dir = project_dir.join(&root.config.vendor_dir);
    if !vendor_dir.is_dir() {
        bail!(
            "{} not found; run `viv install` first",
            vendor_dir.display()
        );
    }

    let selected: Vec<&Package> = lock.packages(dev).collect();
    for package in &selected {
        let dir = package_dir(&vendor_dir, &project_dir, package);
        if package.r#type != "metapackage" && !dir.is_dir() {
            bail!("{} not found; run `viv install` first", dir.display());
        }
    }

    let state = State {
        content_hash: lock.content_hash.clone(),
        dev,
        composer_json_sha256: hex(Sha256::digest(&composer_json)),
        patches_fingerprint: patches_fingerprint(&plugins, &root, &project_dir)?,
    };
    let state_path = vendor_dir.join("composer/.vivace-state");

    let composer_json_value: Value =
        serde_json::from_slice(&composer_json).context("parsing composer.json")?;
    let mut scripts = scripts::Runner::new(
        &composer_json_value,
        &project_dir,
        &root.config.bin_dir(),
        dev,
        args.no_scripts,
    );

    let flags = AutoloadFlags::from(args);
    regenerate_vendor_metadata(
        &flags,
        &root,
        &lock,
        &vendor_dir,
        &project_dir,
        &selected,
        dev,
        &state,
        &state_path,
        &mut scripts,
        // No fetch/link ran here, so no freshly resolved archive dirs to key
        // a classmap cache on; the scan just runs uncached.
        None,
        &plugins,
        false,
    )?;

    out("Generated autoload files");
    Ok(())
}

/// `viv cache prune`/`viv cache clean`.
pub fn cache(args: &CacheArgs, cache_dir: Option<&Path>) -> Result<()> {
    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    match &args.command {
        CacheCommand::Prune { older_than } => {
            let store = Store::open(&cache_dir)?;
            let older_than = older_than.map(|days| Duration::from_secs(days * 86_400));
            let report = store.prune(older_than)?;
            out(&format!(
                "Removed {} entr{} ({}) from {}",
                report.entries,
                if report.entries == 1 { "y" } else { "ies" },
                human_bytes(report.bytes),
                cache_dir.display()
            ));
        }
        CacheCommand::Clean => {
            if !cache_dir.exists() {
                out("Nothing to clean");
                return Ok(());
            }
            let unexpected = Store::unexpected_entries(&cache_dir)?;
            if !unexpected.is_empty() {
                bail!(
                    "{} doesn't look like a vivace cache: unexpected {}. Refusing to remove \
                     it. If that is a bucket from an older viv, `viv cache prune` removes it; \
                     otherwise check --cache-dir/XDG_CACHE_HOME.",
                    cache_dir.display(),
                    unexpected.join(", ")
                );
            }
            Store::open(&cache_dir)?.clean()?;
            out(&format!(
                "Removed the vivace cache at {}",
                cache_dir.display()
            ));
        }
        CacheCommand::Size => {
            let size = Store::open(&cache_dir)?.size()?;
            out(&format!(
                "{} ({} archives, {} pointers) in {}",
                human_bytes(size.archive_bytes),
                size.archives,
                size.pointers,
                cache_dir.display()
            ));
        }
    }
    Ok(())
}

/// `1234` -> `"1234 bytes"`, `1_500_000` -> `"1.43 MiB"`: binary units,
/// Composer/`du`-shaped, only used for `viv cache`'s own report lines.
#[allow(
    clippy::cast_precision_loss,
    reason = "a cache's byte count is nowhere near f64's 52-bit mantissa limit"
)]
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = UNITS[0];
    for candidate in &UNITS[1..] {
        if value < 1024.0 {
            break;
        }
        value /= 1024.0;
        unit = candidate;
    }
    format!("{value:.2} {unit}")
}

/// Build the generator's `Input` from the root and the packages that will
/// end up in `vendor/`, generate the autoload files, then write
/// `platform_check.php` (or delete it) beside them.
#[expect(clippy::too_many_arguments, reason = "install's tail, no bundling win")]
fn write_autoload(
    flags: &AutoloadFlags,
    root: &Root,
    lock: &Lock,
    vendor_dir: &Path,
    project_dir: &Path,
    packages: &[&Package],
    dev: bool,
    scripts: &mut scripts::Runner,
    archive_dirs: Option<&HashMap<String, PathBuf>>,
    plugins: &plugins::Plugins,
    plugin_packages: &[(&Package, PathBuf)],
) -> Result<()> {
    scripts.dispatch("pre-autoload-dump")?;
    // `tbachert/spi` subscribes to `PRE_AUTOLOAD_DUMP`, so it runs from both
    // `install` and `dump-autoload` alike, right where Composer would run it.
    plugins.apply_pre_autoload_dump(root, vendor_dir, plugin_packages)?;
    // #93: `drupal/core-composer-scaffold`'s own `PRE_AUTOLOAD_DUMP` listener
    // points the root package's classmap at `vendor/drupal/DrupalInstalled.php`
    // (and, conditionally, a few framework classes); `packages` here (not
    // `plugin_packages`) since the real plugin's version hash walks
    // Composer's local repository, metapackages included.
    let scaffold_classmap = plugins.scaffold_classmap(root, project_dir, vendor_dir, packages)?;
    let suffix = resolve_suffix(root, lock, vendor_dir)?;
    let classmap_authoritative = flags.classmap_authoritative || root.config.classmap_authoritative;
    let scan_psr =
        flags.optimize_autoloader || classmap_authoritative || root.config.optimize_autoloader;
    let apcu_prefix_override = flags
        .apcu_autoloader_prefix
        .clone()
        .or_else(|| root.config.apcu_autoloader_prefix.clone());
    let apcu_autoloader = flags.apcu_autoloader
        || flags.apcu_autoloader_prefix.is_some()
        || root.config.apcu_autoloader
        || apcu_prefix_override.is_some();
    let apcu_prefix = apcu_autoloader.then_some(apcu_prefix_override);
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
    let root_autoload = merge_classmap(
        root.autoload.clone().unwrap_or(Value::Null),
        scaffold_classmap,
    );
    let input = Input {
        root: RootPackage {
            name: root_name,
            autoload: root_autoload,
            autoload_dev: root.autoload_dev.clone().unwrap_or(Value::Null),
            target_dir: None,
            requires: keys(&root.require),
            include_path: root.include_path.clone(),
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
                install_path: (p.r#type != "metapackage")
                    .then(|| package_dir(vendor_dir, project_dir, p)),
                is_dev: p.dev,
                include_path: p.include_path.clone(),
                archive_dir: archive_dirs.and_then(|dirs| dirs.get(&p.name)).cloned(),
            })
            .collect(),
        dev_mode: dev,
        scan_psr,
        suffix,
        vendor_dir: vendor_dir.to_path_buf(),
        base_dir: project_dir.to_path_buf(),
        platform_check: platform_body.is_some(),
        prepend_autoloader: root.config.prepend_autoloader,
        classmap_authoritative,
        apcu_prefix,
        use_include_path: root.config.use_include_path,
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
    scripts.dispatch("post-autoload-dump")?;
    // #93: `symfony/runtime` subscribes to this same script event to write
    // `vendor/autoload_runtime.php`.
    plugins.apply_post_autoload_dump(root, project_dir, vendor_dir)?;
    Ok(())
}

/// `drupal/core-composer-scaffold`'s `Plugin::preAutoloadDump` (#93): merges
/// [`plugins::Plugins::scaffold_classmap`]'s extra classmap paths into the
/// root package's own `autoload.classmap` before it reaches the generator —
/// the classmap scanner already resolves an absolute file path in that list
/// on its own, so no other generator change is needed.
fn merge_classmap(autoload: Value, extra: Vec<String>) -> Value {
    if extra.is_empty() {
        return autoload;
    }
    let mut map = match autoload {
        Value::Object(map) => map,
        _ => Map::new(),
    };
    match map
        .entry("classmap")
        .or_insert_with(|| Value::Array(Vec::new()))
    {
        Value::Array(items) => items.extend(extra.into_iter().map(Value::String)),
        other => *other = Value::Array(extra.into_iter().map(Value::String).collect()),
    }
    Value::Object(map)
}

/// Concurrent `Store::add_zip` calls, so a slow disk cannot pile up an
/// unbounded number of decompressed zips' worth of downloaded bytes waiting
/// to be written.
const EXTRACT_CONCURRENCY: usize = 8;

/// #37: fan `link_tree` out across a bounded pool instead of linking every
/// archive-backed package one at a time — hardlinking (or copying, under
/// `--link-mode copy`) is disk/syscall-bound the same way extraction is, so
/// it gets the same eight-way ceiling, further capped by the machine's own
/// core count via `std::thread::scope` (no new dependency: `link_tree` is
/// already safe to call concurrently, each call touching a disjoint `dest`).
fn link_archives(
    packages: &[&Package],
    vendor_dir: &Path,
    project_dir: &Path,
    archive_dirs: &HashMap<String, PathBuf>,
    link_mode: LinkMode,
) -> Result<()> {
    if packages.is_empty() {
        return Ok(());
    }
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(EXTRACT_CONCURRENCY)
        .min(packages.len());
    let next = AtomicUsize::new(0);
    let error: Mutex<Option<anyhow::Error>> = Mutex::new(None);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(package) = packages.get(index) else {
                        break;
                    };
                    if error
                        .lock()
                        .expect("error mutex is never poisoned")
                        .is_some()
                    {
                        break;
                    }
                    let dest = package_dir(vendor_dir, project_dir, package);
                    let dir = archive_dirs
                        .get(&package.name)
                        .expect("every install candidate was fetched or found in the store");
                    if let Err(err) = link_tree(dir, &dest, link_mode) {
                        let mut guard = error.lock().expect("error mutex is never poisoned");
                        if guard.is_none() {
                            *guard = Some(err);
                        }
                        break;
                    }
                }
            });
        }
    });
    match error.into_inner().expect("error mutex is never poisoned") {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Download every package not already in the store, extracting each into it
/// as its bytes arrive.
///
/// Extraction is spawned onto an unbounded `JoinSet`, each task waiting on a
/// `Semaphore` for its turn before doing the actual (blocking) work, rather
/// than gating `downloads.next()` on the extraction count: gating the
/// `select!` branch itself stops the *whole* stream being polled once
/// `EXTRACT_CONCURRENCY` extractions are in flight, which stalls every
/// in-flight download too, since `buffer_unordered`'s in-flight downloads
/// only make progress while their stream is polled — the same head-of-line
/// blocking this function's `JoinSet` was meant to avoid, just eight
/// downloads wide instead of one (#1).
async fn fetch_missing(
    fetcher: &fetch::Fetcher,
    store: Arc<Store>,
    packages: &[Package],
    best_effort_adopt: &HashSet<String>,
) -> Result<HashMap<String, PathBuf>> {
    // #21: large downloads spill to a temp file in the store's own temp area
    // rather than growing an ever-larger `Vec<u8>`; small ones (the common
    // case) still travel as bytes.
    let temp_dir = store.temp_dir()?;
    let mut downloads = fetcher.fetch_all(packages, CONCURRENCY, &temp_dir);
    let extract_slots = Arc::new(tokio::sync::Semaphore::new(EXTRACT_CONCURRENCY));
    // The name travels alongside the outcome (not just on success) so a
    // failure for a best-effort adopt can be told apart from one that must
    // abort the install, without losing which package it was.
    let mut extractions: tokio::task::JoinSet<Result<(String, Result<PathBuf>)>> =
        tokio::task::JoinSet::new();
    let mut result = HashMap::new();
    let mut downloads_done = false;

    while !downloads_done || !extractions.is_empty() {
        tokio::select! {
            item = downloads.next(), if !downloads_done => {
                match item {
                    Some((package, downloaded)) => {
                        let name = package.name.clone();
                        let downloaded = match downloaded {
                            Ok(downloaded) => downloaded,
                            // A best-effort adopt's private dist may 404 or need
                            // auth vivace doesn't have; Composer's own copy is
                            // already on disk, so this keeps it instead of
                            // failing the whole install (#123 follow-up).
                            Err(err) if best_effort_adopt.contains(&name) => {
                                warn_out(&format!(
                                    "Kept vendor/{name} as a Composer copy: {err:#}"
                                ));
                                continue;
                            }
                            Err(err) => return Err(err.context(format!("{name}: fetching dist"))),
                        };
                        let package = package.clone();
                        let store = Arc::clone(&store);
                        let extract_slots = Arc::clone(&extract_slots);
                        extractions.spawn(async move {
                            let _permit = extract_slots
                                .acquire_owned()
                                .await
                                .expect("extract_slots semaphore is never closed");
                            let outcome = tokio::task::spawn_blocking(move || match &downloaded {
                                fetch::Downloaded::Bytes(bytes) => store.add_zip(&package, bytes),
                                fetch::Downloaded::File(path) => {
                                    store.add_archive_from_file(&package, path)
                                }
                            })
                            .await
                            .context("store worker panicked")?;
                            Ok((name, outcome))
                        });
                    }
                    None => downloads_done = true,
                }
            }
            Some(joined) = extractions.join_next(), if !extractions.is_empty() => {
                let (name, outcome) = joined.context("store worker panicked")??;
                match outcome {
                    Ok(dir) => {
                        result.insert(name, dir);
                    }
                    Err(err) if best_effort_adopt.contains(&name) => {
                        warn_out(&format!("Kept vendor/{name} as a Composer copy: {err:#}"));
                    }
                    Err(err) => return Err(err.context(format!("{name}: extracting dist"))),
                }
            }
        }
    }
    Ok(result)
}

/// #36: an install interrupted (Ctrl-C) between `link_tree`'s two renames
/// leaves a `.tmp*`/`.old-*` sibling next to a package dir, under
/// `vendor/<vendor>/`; sweep every such sibling once per install, before
/// linking, rather than leaving it there until it happens to collide with a
/// later `link_tree`'s own temp name. Only vivace's own temp-name patterns
/// are removed — a legitimate dotfile a package or Composer left behind is
/// never touched.
fn sweep_link_litter(vendor_dir: &Path) -> Result<()> {
    if !vendor_dir.is_dir() {
        return Ok(());
    }
    for namespace in fs_err::read_dir(vendor_dir)? {
        let namespace = namespace?;
        if !namespace.file_type()?.is_dir() {
            continue;
        }
        for entry in fs_err::read_dir(namespace.path())? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with(".tmp") && !name.starts_with(".old-") {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs_err::remove_dir_all(entry.path())?;
            } else {
                fs_err::remove_file(entry.path())?;
            }
        }
    }
    Ok(())
}

/// #56/#37: after removing a package's install directory, remove now-empty
/// parent directories one at a time, stopping at (never removing) `boundary`
/// itself, a parent that still has something in it, or one this process
/// cannot remove. Covers both a bare `vendor/<vendor>/` losing its last
/// package and a `target-dir` package's scaffold parents (`vendor/<name>/`
/// itself, one level up from the target-dir leaf `install_path` points at).
fn prune_empty_ancestors(removed: &Path, boundary: &Path) {
    let mut dir = removed.parent();
    while let Some(current) = dir {
        if current == boundary || !current.starts_with(boundary) {
            break;
        }
        let is_empty = fs_err::read_dir(current).is_ok_and(|mut entries| entries.next().is_none());
        if !is_empty || fs_err::remove_dir(current).is_err() {
            break;
        }
        dir = current.parent();
    }
}

/// `vendor/<name>`, plus the legacy `target-dir` nesting when the package
/// still declares one — or, when `src/plugins.rs` mapped this package
/// outside `vendor/` (a native `composer/installers`/`wordpress-core`
/// adapter), that mapped directory under `project_dir` instead.
pub(crate) fn package_dir(vendor_dir: &Path, project_dir: &Path, package: &Package) -> PathBuf {
    if let Some(install_dir) = &package.install_dir {
        return project_dir.join(install_dir);
    }
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

fn read_state(path: &Path) -> Option<State> {
    let content = fs_err::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// `State::patches_fingerprint`: `None` when `cweagans/composer-patches`
/// isn't active, so a project without it never pays for this.
fn patches_fingerprint(
    plugins: &plugins::Plugins,
    root: &Root,
    project_dir: &Path,
) -> Result<Option<String>> {
    plugins
        .has_patches()
        .then(|| plugins::patches::fingerprint(root, project_dir))
        .transpose()
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

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr` lint.
fn warn_out(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

/// `--no-normalize`'s deprecation notice on `install`/`dump-autoload` (#95):
/// the flag is hidden and does nothing now that neither command touches
/// `composer.json`, but a script that still passes it shouldn't fail.
fn warn_no_normalize_is_a_noop(command: &str) {
    warn_out(&format!(
        "--no-normalize is a no-op on {command} since 0.6; {command} no longer touches \
         composer.json"
    ));
}

/// `--adopt`'s TTY confirmation: `y`/`yes` (any case) continues, anything
/// else (including a bare Enter) aborts.
fn confirm_adopt() -> Result<bool> {
    let mut stderr = std::io::stderr().lock();
    write!(
        stderr,
        "This will relink every installed package from the store, overwriting vendor/ in \
         place. Continue? [y/N] "
    )?;
    stderr.flush()?;
    drop(stderr);
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    Ok(matches!(answer.trim().to_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::{prune_empty_ancestors, sweep_link_litter};

    /// #56/#37a: `vendor/<vendor>/` is pruned once its last package's dir is
    /// removed, whether that dir is a plain `vendor/<vendor>/<name>` or a
    /// target-dir package's nested scaffold (`vendor/<vendor>/<name>/Deep/Leaf`).
    #[test]
    fn prune_empty_ancestors_removes_the_vendor_namespace_dir_once_last_package_goes() {
        let vendor_dir = tempfile::tempdir().unwrap();
        let acme = vendor_dir.path().join("acme");
        fs_err::create_dir_all(acme.join("a")).unwrap();
        fs_err::create_dir_all(acme.join("b")).unwrap();

        // One sibling left: vendor/acme must stay.
        fs_err::remove_dir_all(acme.join("a")).unwrap();
        prune_empty_ancestors(&acme.join("a"), vendor_dir.path());
        assert!(acme.is_dir(), "vendor/acme should stay: b/ is still there");

        // Last sibling goes: vendor/acme is pruned too, but not vendor/ itself.
        fs_err::remove_dir_all(acme.join("b")).unwrap();
        prune_empty_ancestors(&acme.join("b"), vendor_dir.path());
        assert!(!acme.exists(), "vendor/acme should be pruned");
        assert!(vendor_dir.path().is_dir(), "vendor/ itself must survive");
    }

    /// A target-dir package's `install_path` points at the nested leaf
    /// (`vendor/<vendor>/<name>/Symfony/Component/Yaml`); removing it must
    /// also clear the now-empty scaffold above it, up to and including
    /// `vendor/<vendor>/<name>` itself, not just the leaf.
    #[test]
    fn prune_empty_ancestors_clears_a_target_dir_scaffold() {
        let vendor_dir = tempfile::tempdir().unwrap();
        let leaf = vendor_dir
            .path()
            .join("symfony/yaml/Symfony/Component/Yaml");
        fs_err::create_dir_all(&leaf).unwrap();
        fs_err::remove_dir_all(&leaf).unwrap();

        prune_empty_ancestors(&leaf, vendor_dir.path());

        assert!(
            !vendor_dir.path().join("symfony").exists(),
            "the whole scaffold above the leaf should be gone"
        );
        assert!(vendor_dir.path().is_dir(), "vendor/ itself must survive");
    }

    /// A native-installer path (`wp-content/mu-plugins/<name>`) prunes up to
    /// `project_dir`, not `vendor_dir` — the package never lived under vendor/.
    #[test]
    fn prune_empty_ancestors_stops_at_a_non_vendor_boundary() {
        let project_dir = tempfile::tempdir().unwrap();
        let plugin = project_dir.path().join("wp-content/mu-plugins/only-one");
        fs_err::create_dir_all(&plugin).unwrap();
        fs_err::remove_dir_all(&plugin).unwrap();

        prune_empty_ancestors(&plugin, project_dir.path());

        assert!(
            !project_dir.path().join("wp-content").exists(),
            "the empty wp-content scaffold should be pruned"
        );
        assert!(project_dir.path().is_dir(), "project_dir must survive");
    }

    #[test]
    fn sweep_link_litter_removes_only_its_own_temp_patterns() {
        let vendor_dir = tempfile::tempdir().unwrap();
        let acme = vendor_dir.path().join("acme");
        fs_err::create_dir_all(acme.join("pkg")).unwrap();
        fs_err::write(acme.join("pkg/A.php"), "<?php").unwrap();
        fs_err::create_dir_all(acme.join(".tmpABCDEF")).unwrap();
        fs_err::create_dir_all(acme.join(".old-123456")).unwrap();
        fs_err::write(acme.join(".tmpfile"), "x").unwrap();
        // Not ours: a legitimate dotfile a package left behind must survive.
        fs_err::write(acme.join(".gitkeep"), "").unwrap();

        sweep_link_litter(vendor_dir.path()).unwrap();

        let remaining: Vec<String> = fs_err::read_dir(&acme)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            {
                let mut sorted = remaining;
                sorted.sort();
                sorted
            },
            [".gitkeep", "pkg"]
        );
    }

    #[test]
    fn sweep_link_litter_is_a_no_op_on_a_missing_vendor_dir() {
        let vendor_dir = tempfile::tempdir().unwrap();
        let missing = vendor_dir.path().join("does-not-exist");
        sweep_link_litter(&missing).unwrap();
    }
}
