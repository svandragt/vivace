//! `viv update`: full and partial updates (`docs/resolver-design.md` stages
//! 4 and 5). Reads the root `composer.json`, solves against Packagist
//! (`solver::solve_update`/`solver::solve_partial_update`, the merged first
//! solve plus the require-only second solve for the dev split), and writes
//! a `composer.lock` (`lock_writer::write`). `--lock` skips solving
//! entirely: it re-derives the lock from itself, matching `composer update
//! --lock`'s own Locker round-trip.
//!
//! Not reachable from `src/install.rs`/`src/main.rs`'s `install` path
//! (`AGENTS.md`'s Performance rule only gates `install`).

use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::Value;

use crate::auth::Auth;
use crate::fetch::Fetcher;
use crate::repository::{HttpTransport, Repository};
use crate::solver::{self, pool_builder::UpdateAllowMode, transaction::ResolvedPackage};

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
    /// Prefer already-locked versions over the newest one a partial
    /// update's allow-listed packages could otherwise pick
    /// (`Installer::setMinimalUpdate`). Parsed but not yet wired to a
    /// solve: the pin mechanism itself is ported
    /// (`solver::policy::DefaultPolicy::with_preferred_versions`), but
    /// nothing here constructs a policy with it yet (see that port's own
    /// doc comment for why).
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
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
}

const PACKAGIST_URL: &str = "https://repo.packagist.org";

pub fn run(args: &UpdateArgs, cache_dir: Option<&Path>) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root: Value = serde_json::from_slice(&composer_json).context("parsing composer.json")?;
    let lock_path = project_dir.join("composer.lock");

    let lock = if args.lock {
        lock_only(&lock_path, &composer_json)?
    } else {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let result = runtime.block_on(solve(args, &project_dir, &root, &lock_path, cache_dir))?;
        lock_json(&result, &composer_json)?
    };

    if args.dry_run {
        // `writeln!` to stdout directly, not `println!`, to satisfy the
        // `print_stdout` lint (`install.rs`'s `out` helper does the same).
        write!(std::io::stdout().lock(), "{lock}")?;
        return Ok(());
    }

    fs_err::write(lock_path, lock)?;
    let _ = args.no_dev; // `--no-dev` only changes `install`'s selection, not the lock.
    Ok(())
}

async fn solve(
    args: &UpdateArgs,
    project_dir: &Path,
    root: &Value,
    lock_path: &Path,
    cache_dir: Option<&Path>,
) -> Result<solver::UpdateResult> {
    let secure_http = root
        .pointer("/config/secure-http")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let prefer_stable = args.prefer_stable
        || root
            .get("prefer-stable")
            .and_then(Value::as_bool)
            .unwrap_or(false);

    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => default_cache_dir()?,
    };
    let auth = Auth::load(project_dir)?;
    let fetcher = Fetcher::new(auth)?.secure_http(secure_http);
    let transport = HttpTransport { fetcher: &fetcher };
    let repo = Repository::load(PACKAGIST_URL, &cache_dir, transport).await?;

    if args.packages.is_empty() {
        return solver::solve_update(&repo, root, prefer_stable, args.prefer_lowest).await;
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
    let mode = if args.with_all_dependencies {
        UpdateAllowMode::WithTransitiveDeps
    } else if args.with_dependencies {
        UpdateAllowMode::WithTransitiveDepsNoRootRequire
    } else {
        UpdateAllowMode::OnlyListed
    };
    solver::solve_partial_update(
        &repo,
        root,
        prefer_stable,
        args.prefer_lowest,
        &locked_by_name,
        &args.packages,
        mode,
    )
    .await
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

/// `$XDG_CACHE_HOME/vivace`, falling back to `~/.cache/vivace`. Duplicated
/// from `install.rs` (private there, small, and `install.rs` isn't this
/// lane's file to widen). `pub(crate)`: `require.rs` reuses this one rather
/// than a third copy.
pub(crate) fn default_cache_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("vivace"));
    }
    let home = std::env::var("HOME").context("HOME is not set; pass --cache-dir")?;
    Ok(PathBuf::from(home).join(".cache").join("vivace"))
}
