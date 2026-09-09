//! `viv update`: full and partial updates (`docs/resolver-design.md` stages
//! 4 and 5). Reads the root `composer.json`, builds every repository its
//! `repositories` names (`Repository::from_composer_json`, `#67`) plus the
//! implicit `packagist.org`, solves against them
//! (`solver::solve_update`/`solver::solve_partial_update`, the merged first
//! solve plus the require-only second solve for the dev split), and writes
//! a `composer.lock` (`lock_writer::write`). `update` only ever reads
//! `composer.json`, never rewrites it: normalizing is reserved for the
//! commands that edit it (`add`/`rm`/`init`, `docs/stability.md`), so
//! `--no-normalize` here is a deprecated no-op, same shape as `install`/
//! `dump-autoload`'s own (#95). `--lock` skips solving entirely: it
//! re-derives the lock from itself, matching `composer update --lock`'s
//! own Locker round-trip.
//!
//! Not reachable from `src/install.rs`/`src/main.rs`'s `install` path
//! (`AGENTS.md`'s Performance rule only gates `install`).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::Value;

use crate::auth::Auth;
use crate::fetch::Fetcher;
use crate::install::{self, InstallArgs};
use crate::link::LinkMode;
use crate::normalize;
use crate::repository::{HttpTransport, Repository};
use crate::scripts;
use crate::solver::{
    self,
    pool_builder::{AdvisoryFilter, UpdateAllowMode},
    transaction::ResolvedPackage,
};

/// `viv update` flags: `docs/resolver-design.md` stages 4 (full update) and
/// 5 (partial update, `--minimal-changes`, `--lock`).
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's update flags"
)]
#[derive(Args, Debug, Clone)]
pub struct UpdateArgs {
    /// Only these packages (and, with `-w`/`-W`, their dependencies) may
    /// change version; everything else stays at its locked version
    /// (`Request::UPDATE_*`). Empty means a full update.
    pub packages: Vec<String>,
    /// Also allow each listed package's dependencies to update, except ones
    /// also directly required by the root `composer.json`
    /// (`UPDATE_LISTED_WITH_TRANSITIVE_DEPS_NO_ROOT_REQUIRE`).
    #[arg(short = 'w', long = "with-dependencies")]
    pub with_dependencies: bool,
    /// Like `-w`, but a dependency directly required by the root
    /// `composer.json` may update too
    /// (`UPDATE_LISTED_WITH_TRANSITIVE_DEPS`).
    #[arg(short = 'W', long = "with-all-dependencies")]
    pub with_all_dependencies: bool,
    /// Prefer already-locked versions over the newest one an update could
    /// otherwise pick, for every package not on this update's own literal
    /// package list (`Installer::setMinimalUpdate`, `preferred_versions`
    /// below).
    #[arg(long)]
    pub minimal_changes: bool,
    /// Re-derive `composer.lock` from itself (content-hash, key order,
    /// `fixupJsonDataType`) without solving: `composer update --lock`.
    #[arg(long)]
    pub lock: bool,
    /// Solve without `require-dev`, but still resolve and record dev
    /// packages in the lock (`composer update --no-dev`'s actual behaviour:
    /// only `install`'s package selection skips them, not the lock).
    #[arg(long)]
    pub no_dev: bool,
    /// Prefer the lowest package versions that satisfy every constraint.
    #[arg(long)]
    pub prefer_lowest: bool,
    /// Prefer stable releases, even when a less stable one would otherwise
    /// win the version pick.
    #[arg(long)]
    pub prefer_stable: bool,
    /// Solve and print, but don't write `composer.lock`.
    #[arg(long)]
    pub dry_run: bool,
    /// No-op since 0.8: `update` never writes `composer.json`, so there is
    /// nothing left to skip (only `add`/`rm`/`init` normalize it). Kept,
    /// hidden, for one release so an old invocation doesn't fail; prints a
    /// deprecation warning instead.
    #[arg(long, hide = true)]
    pub no_normalize: bool,
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Skip `pre-update-cmd`/`post-update-cmd` and every other root
    /// `scripts` listener, including the chained install's own
    /// `pre-autoload-dump`/`post-autoload-dump`.
    #[arg(long)]
    pub no_scripts: bool,
    /// Passed straight through to the chained install (`docs/plugin-strategy.md`).
    #[arg(long)]
    pub no_plugins: bool,
    /// Skip the install step after writing `composer.lock`
    /// (`composer update --no-install`): today's `viv update` behaviour.
    #[arg(long)]
    pub no_install: bool,
    /// Allows installing a version a known security advisory covers or a
    /// package Packagist marks abandoned, instead of blocking it by default
    /// (#175, `audit.block-insecure`/`audit.block-abandoned`). Also settable
    /// via `COMPOSER_NO_SECURITY_BLOCKING=1`.
    #[arg(long)]
    pub no_blocking: bool,
    /// Deprecated alias for `--no-blocking`.
    #[arg(long = "no-security-blocking", hide = true)]
    pub no_security_blocking: bool,
}

/// `Platform::getBoolEnv('COMPOSER_NO_SECURITY_BLOCKING')`: non-empty and not
/// the literal `"0"` (`main.rs`'s own `network_disabled` reads
/// `COMPOSER_DISABLE_NETWORK` the same way). Shared with `require.rs`'s
/// `--no-blocking`/`--no-security-blocking`.
pub(crate) fn no_security_blocking_env() -> bool {
    std::env::var("COMPOSER_NO_SECURITY_BLOCKING")
        .is_ok_and(|value| !value.is_empty() && value != "0")
}

/// `AdvisoryFilter`'s config/flag half, read the same way `bin_dir` reads
/// `config.bin-dir` off the raw root `Value` rather than the fuller
/// `lock::parse_root` (`update`/`require` each only need this one piece).
/// `no_blocking` is the caller's own CLI flag(s), already OR'd with the
/// deprecated alias; the env var applies on top either way.
pub(crate) fn audit_config_and_no_blocking(
    root: &Value,
    no_blocking: bool,
) -> Result<(crate::lock::AuditConfig, bool)> {
    let audit_config: crate::lock::AuditConfig = root
        .pointer("/config/audit")
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .context("parsing config.audit")?
        .unwrap_or_default();
    Ok((audit_config, no_blocking || no_security_blocking_env()))
}

pub fn run(args: &UpdateArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    if args.no_normalize {
        warn_no_normalize_is_a_noop("update");
    }
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root: Value = serde_json::from_slice(&composer_json).context("parsing composer.json")?;
    let lock_path = project_dir.join("composer.lock");

    // `--lock` re-derives the lock from itself (no solving, so nothing to
    // dispatch `pre-update-cmd`/`post-update-cmd` around, and no install to
    // chain into either).
    if args.lock {
        let lock = lock_only(&lock_path, &composer_json)?;
        if args.dry_run {
            write!(std::io::stdout().lock(), "{lock}")?;
            return Ok(());
        }
        fs_err::write(&lock_path, lock)?;
        return Ok(());
    }

    // `Installer::run`: `pre-update-cmd` dispatches before pool building even
    // starts; skipped entirely under `--dry-run`, which never runs scripts,
    // same as Composer's own `dryRun` -> `runScripts = false`.
    let bin_dir = bin_dir(&root);
    let mut scripts =
        scripts::Runner::new(&root, &project_dir, &bin_dir, !args.no_dev, args.no_scripts);
    if !args.dry_run {
        scripts.dispatch("pre-update-cmd")?;
    }

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let solve_started = Instant::now();
    let result = runtime.block_on(solve(
        args,
        &project_dir,
        &root,
        &lock_path,
        cache_dir,
        offline,
    ))?;
    tracing::debug!(
        elapsed_ms = solve_started.elapsed().as_millis(),
        "resolved metadata and solved (merged solve, plus the dev-split \
         second solve when require-dev is non-empty)"
    );
    let lock_write_started = Instant::now();
    let lock = lock_json(&result, &composer_json)?;
    tracing::debug!(
        elapsed_ms = lock_write_started.elapsed().as_millis(),
        "wrote composer.lock (content-hash + serialisation)"
    );

    // #156: diff against the lock as it still sits on disk, before this
    // overwrites it, and print Composer's `Lock file operations: ...`
    // block. Composer's own dry run prints this same block (still
    // `doUpdate`, before the `writeLock`-gated part) but never "Writing
    // lock file", since nothing is actually persisted.
    print_lock_operations(&lock_path, &result.non_dev, &result.dev, !args.dry_run)?;

    if args.dry_run {
        // `writeln!` to stdout directly, not `println!`, to satisfy the
        // `print_stdout` lint (`install.rs`'s `out` helper does the same).
        write!(std::io::stdout().lock(), "{lock}")?;
        return Ok(());
    }

    fs_err::write(&lock_path, lock)?;

    if !args.no_install {
        let install_args = InstallArgs {
            no_dev: args.no_dev,
            dry_run: false,
            link_mode: LinkMode::default(),
            adopt: false,
            project_dir: project_dir.clone(),
            optimize_autoloader: false,
            classmap_authoritative: false,
            apcu_autoloader: false,
            apcu_autoloader_prefix: None,
            no_scripts: args.no_scripts,
            no_normalize: false,
            no_plugins: args.no_plugins,
            no_progress: false,
        };
        install::run_after_update(&install_args, cache_dir, offline)?;
    }

    scripts.dispatch("post-update-cmd")?;
    Ok(())
}

/// `config.bin-dir`, resolved the same way `lock::Config::bin_dir` does
/// (`{vendor-dir}/bin` when unset), read straight off the raw root `Value`
/// here rather than the fuller `lock::parse_root`: `update`/`require`/
/// `remove` only need this one string each, and `install::run_after_update`
/// re-parses the root properly moments later anyway. `pub(crate)`:
/// `require.rs` reuses this one rather than a third copy (same reasoning as
/// `default_cache_dir` below).
pub(crate) fn bin_dir(root: &Value) -> String {
    let vendor_dir = root
        .pointer("/config/vendor-dir")
        .and_then(Value::as_str)
        .map_or("vendor", |v| v.trim_end_matches('/'));
    root.pointer("/config/bin-dir")
        .and_then(Value::as_str)
        .map_or_else(|| format!("{vendor_dir}/bin"), str::to_string)
}

/// `Fetcher` wired the same way every solve step needs it: `config.secure-http`
/// read off the root `composer.json`, `Auth::load`'d from `project_dir`, and
/// `--offline` passed straight through. Shared with `require::partial_update`
/// (`#158`: that solve used to ignore `--offline` entirely).
pub(crate) fn build_fetcher(project_dir: &Path, root: &Value, offline: bool) -> Result<Fetcher> {
    let secure_http = root
        .pointer("/config/secure-http")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let auth = Auth::load(project_dir)?;
    Ok(Fetcher::new(auth)?
        .secure_http(secure_http)
        .offline(offline))
}

/// `Repository::from_composer_json` against `root`'s own `repositories`
/// (`#67`) plus the implicit `packagist.org`, over `fetcher`. Shared with
/// `require::partial_update`, which used to resolve against a hard-coded
/// `https://repo.packagist.org` instead, ignoring `composer.json`'s own
/// `repositories` (`#158`).
pub(crate) async fn build_repository<'f>(
    root: &Value,
    cache_dir: &Path,
    fetcher: &'f Fetcher,
) -> Result<Repository<HttpTransport<'f>>> {
    Repository::from_composer_json(root, cache_dir, HttpTransport { fetcher }).await
}

async fn solve(
    args: &UpdateArgs,
    project_dir: &Path,
    root: &Value,
    lock_path: &Path,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<solver::UpdateResult> {
    // #159: fetcher/repository construction plus the current lock's own
    // read+parse (seeding `--minimal-changes`/#90's prefetch), all of it
    // ahead of `pool_builder`'s own closure walk and its `closure_started`
    // timer, so it would otherwise vanish from the profile as an
    // unaccounted gap between "resolved metadata and solved" and "loaded
    // metadata closure".
    let setup_started = Instant::now();
    let prefer_stable = args.prefer_stable
        || root
            .get("prefer-stable")
            .and_then(Value::as_bool)
            .unwrap_or(false);

    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => default_cache_dir()?,
    };
    let fetcher = build_fetcher(project_dir, root, offline)?;
    let repo = build_repository(root, &cache_dir, &fetcher).await?;
    let (audit_config, no_blocking) =
        audit_config_and_no_blocking(root, args.no_blocking || args.no_security_blocking)?;
    let advisories_transport = crate::audit::HttpTransport { fetcher: &fetcher };
    let advisories = Some(AdvisoryFilter {
        transport: &advisories_transport,
        audit: &audit_config,
        no_blocking,
    });

    if args.packages.is_empty() {
        // #90: a warm update's closure is almost always the previous
        // `composer.lock` again; seeding with its package names starts
        // fetching all of them in the first wave instead of discovering
        // most of them one BFS level's round trip at a time. No lock yet
        // (first-ever update) just means an empty seed, same as before.
        let locked_by_name = read_locked_by_name(lock_path)?;
        let seed: Vec<String> = locked_by_name.keys().cloned().collect();
        let preferred = if args.minimal_changes {
            preferred_versions(&locked_by_name, &[])?
        } else {
            HashMap::new()
        };
        tracing::debug!(
            elapsed_ms = setup_started.elapsed().as_millis(),
            "built fetcher/repository and read the current lock, ahead of the closure walk"
        );
        return solver::solve_update_seeded(
            &repo,
            root,
            prefer_stable,
            args.prefer_lowest,
            &seed,
            preferred,
            advisories,
        )
        .await;
    }

    if !lock_path.exists() {
        bail!(
            "no composer.lock present; a partial update of {} needs one to know which other \
             packages to keep locked",
            args.packages.join(", ")
        );
    }
    let lock_bytes = fs_err::read(lock_path)?;
    let lock: Value = serde_json::from_slice(&lock_bytes).context("parsing composer.lock")?;
    let locked_by_name = locked_packages_by_name(&lock);
    let seed: Vec<String> = locked_by_name.keys().cloned().collect();
    let mode = if args.with_all_dependencies {
        UpdateAllowMode::WithTransitiveDeps
    } else if args.with_dependencies {
        UpdateAllowMode::WithTransitiveDepsNoRootRequire
    } else {
        UpdateAllowMode::OnlyListed
    };
    let preferred = if args.minimal_changes {
        preferred_versions(&locked_by_name, &args.packages)?
    } else {
        HashMap::new()
    };
    solver::solve_partial_update_seeded(
        &repo,
        root,
        prefer_stable,
        args.prefer_lowest,
        &locked_by_name,
        &args.packages,
        mode,
        &seed,
        preferred,
        advisories,
    )
    .await
}

/// `locked_by_name` for the full-update path (#90's seed, keyed the same
/// way `locked_packages_by_name` keys a partial update's): empty when there
/// isn't a lock yet, read straight off the lock file since the full-update
/// path has no other reason to load it.
fn read_locked_by_name(lock_path: &Path) -> Result<HashMap<String, Value>> {
    if !lock_path.exists() {
        return Ok(HashMap::new());
    }
    let lock_bytes = fs_err::read(lock_path)?;
    let lock: Value = serde_json::from_slice(&lock_bytes).context("parsing composer.lock")?;
    Ok(locked_packages_by_name(&lock))
}

/// `--minimal-changes`'s pin set (`Installer::createPolicy`'s
/// `$preferredVersions[$pkg->getName()] = $pkg->getVersion();` loop): every
/// locked package's own normalized version, except ones on the literal
/// (unexpanded) allow list — `Installer::setUpdateAllowList`'s own
/// lowercased `$packages`, not `pool_builder::expand_allow_list`'s
/// transitive expansion — since those are exactly the packages the user
/// asked to move and must stay free to. `AliasPackage`s are skipped in PHP
/// too, but never show up here: they come from the lock's separate
/// `aliases` array, not its `packages`/`packages-dev` entries that
/// `locked_by_name` is built from.
fn preferred_versions(
    locked_by_name: &HashMap<String, Value>,
    allow_list: &[String],
) -> Result<HashMap<String, crate::semver::NormalizedVersion>> {
    let allow: std::collections::HashSet<String> =
        allow_list.iter().map(|n| n.to_ascii_lowercase()).collect();
    locked_by_name
        .iter()
        .filter(|(name, _)| !allow.contains(*name))
        .map(|(name, entry)| {
            let version = entry
                .get("version")
                .and_then(Value::as_str)
                .with_context(|| format!("locked package {name} has no version"))?;
            Ok((name.clone(), crate::semver::normalize(version)?))
        })
        .collect()
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint
/// (`install.rs`'s own `out` helper does the same).
fn out(message: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr` lint
/// (`install.rs`'s own `warn_out` does the same).
fn warn_out(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

/// #156: diffs `lock_path`'s on-disk packages (read fresh here, so this
/// must run before the caller overwrites the file) against `non_dev`/`dev`
/// and prints Composer's `Lock file operations: ...` block
/// (`Installer::doUpdate`) plus its per-operation lines
/// (`crate::lock::diff_lock_operations`), on stdout like `install.rs`'s own
/// summary line — not stderr, even though Composer's own `writeError` puts
/// it there (`docs/stability.md`'s "Normalized ..." stays on stderr).
/// `will_write` gates the trailing "Writing lock file" line: Composer
/// prints the block either way but that line only when it actually
/// persists (`viv update --dry-run` never does). Shared by
/// `require::partial_update` (`viv add`/`viv rm`), which always writes, so
/// it always passes `true`.
pub(crate) fn print_lock_operations(
    lock_path: &Path,
    non_dev: &[ResolvedPackage],
    dev: &[ResolvedPackage],
    will_write: bool,
) -> Result<()> {
    let previous_by_name = read_locked_by_name(lock_path)?;
    let operations = crate::lock::diff_lock_operations(&previous_by_name, non_dev, dev)?;
    if operations.is_empty() {
        out("Nothing to modify in lock file");
    } else {
        out(&operations.summary_line());
        for line in &operations.lines {
            out(&format!("  - {line}"));
        }
    }
    if will_write {
        out("Writing lock file");
    }
    Ok(())
}

/// `--no-normalize`'s deprecation notice on `update` (0.8): `update` never
/// writes `composer.json`, so the flag has nothing left to disable, same
/// shape as `install.rs`'s own `warn_no_normalize_is_a_noop` for
/// `install`/`dump-autoload` (#95).
fn warn_no_normalize_is_a_noop(command: &str) {
    warn_out(&normalize::no_normalize_is_a_noop_message(
        command,
        "0.8",
        &format!("{command} no longer touches composer.json"),
    ));
}

/// The current lock's `packages`+`packages-dev`, keyed by lowercased name:
/// `solver::solve_partial_update`'s view of "everything currently locked",
/// merging both arrays the way `Request::getLockedPackages` merges a single
/// `LockArrayRepository` built from both (`Locker::getLockedRepository`).
fn locked_packages_by_name(lock: &Value) -> HashMap<String, Value> {
    let mut by_name = HashMap::new();
    for key in ["packages", "packages-dev"] {
        let Some(entries) = lock.get(key).and_then(Value::as_array) else {
            continue;
        };
        for entry in entries {
            if let Some(name) = entry.get("name").and_then(Value::as_str) {
                by_name.insert(name.to_ascii_lowercase(), entry.clone());
            }
        }
    }
    by_name
}

/// `composer update --lock`: re-run `Locker::setLockData`'s own
/// normalisation (content-hash, key order, `fixupJsonDataType`) over the
/// current lock's package lists, without solving anything.
fn lock_only(lock_path: &Path, composer_json: &[u8]) -> Result<String> {
    let lock_bytes = fs_err::read(lock_path).context("reading composer.lock")?;
    let lock: Value = serde_json::from_slice(&lock_bytes).context("parsing composer.lock")?;

    let non_dev = resolved_packages(&lock, "packages");
    let dev = resolved_packages(&lock, "packages-dev");
    let dev = if lock.get("packages-dev").is_some_and(|v| !v.is_null()) {
        Some(dev)
    } else {
        None
    };

    let stability_flags: HashMap<String, u8> = lock
        .get("stability-flags")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, rank)| u8::try_from(rank.as_u64()?).ok().map(|r| (name.clone(), r)))
        .collect();
    let aliases: Vec<crate::solver::transaction::AliasEntry> = lock
        .get("aliases")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(crate::solver::transaction::AliasEntry {
                package: entry.get("package")?.as_str()?.to_string(),
                version: entry.get("version")?.as_str()?.to_string(),
                alias: entry.get("alias")?.as_str()?.to_string(),
                alias_normalized: entry.get("alias_normalized")?.as_str()?.to_string(),
            })
        })
        .collect();
    let minimum_stability = match lock.get("minimum-stability").and_then(Value::as_str) {
        Some("dev") => "dev",
        Some("alpha") => "alpha",
        Some("beta") => "beta",
        Some("RC") => "RC",
        _ => "stable",
    };
    let empty_map = serde_json::Map::new();
    let platform_reqs = lock
        .get("platform")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| empty_map.clone());
    let platform_dev_reqs = lock
        .get("platform-dev")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_else(|| empty_map.clone());
    let platform_overrides = lock
        .get("platform-overrides")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or(empty_map);

    let options = crate::lock_writer::LockOptions {
        minimum_stability,
        stability_flags: &stability_flags,
        prefer_stable: lock
            .get("prefer-stable")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        prefer_lowest: lock
            .get("prefer-lowest")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        platform_reqs: &platform_reqs,
        platform_dev_reqs: &platform_dev_reqs,
        platform_overrides: &platform_overrides,
        aliases: &aliases,
    };
    crate::lock_writer::write(&non_dev, dev.as_deref(), &options, composer_json)
}

fn resolved_packages(lock: &Value, key: &str) -> Vec<ResolvedPackage> {
    lock.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            Some(ResolvedPackage {
                name: entry.get("name")?.as_str()?.to_string(),
                pretty_version: entry.get("version")?.as_str()?.to_string(),
                raw: entry.clone(),
            })
        })
        .collect()
}

fn lock_json(result: &solver::UpdateResult, composer_json: &[u8]) -> Result<String> {
    let options = crate::lock_writer::LockOptions {
        minimum_stability: result.minimum_stability,
        stability_flags: &result.stability_flags,
        prefer_stable: result.prefer_stable,
        prefer_lowest: result.prefer_lowest,
        platform_reqs: &result.platform_reqs,
        platform_dev_reqs: &result.platform_dev_reqs,
        platform_overrides: &result.platform_overrides,
        aliases: &result.aliases,
    };
    crate::lock_writer::write(&result.non_dev, Some(&result.dev), &options, composer_json)
}

/// `$XDG_CACHE_HOME/vivace`, falling back to `~/.cache/vivace`.
pub(crate) fn default_cache_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("vivace"));
    }
    let home = std::env::var("HOME").context("HOME is not set; pass --cache-dir")?;
    Ok(PathBuf::from(home).join(".cache").join("vivace"))
}
