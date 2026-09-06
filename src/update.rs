//! `viv update`: full update only (`docs/resolver-design.md` stage 4). Reads
//! the root `composer.json`, solves against Packagist (`solver::solve_update`,
//! the merged first solve plus the require-only second solve for the dev
//! split), and writes a `composer.lock` (`lock_writer::write`).
//!
//! Not reachable from `src/install.rs`/`src/main.rs`'s `install` path
//! (`AGENTS.md`'s Performance rule only gates `install`), and not a partial
//! update: no package arguments, no update allow-list, no `--lock`-only mode.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde_json::Value;

use crate::auth::Auth;
use crate::fetch::Fetcher;
use crate::repository::{HttpTransport, Repository};
use crate::solver;

/// `viv update` flags: full update only, matching `docs/resolver-design.md`
/// stage 4's scope.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's update flags"
)]
#[derive(Args, Debug, Clone)]
pub struct UpdateArgs {
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
    let auth = Auth::load(&project_dir)?;
    let fetcher = Fetcher::new(auth)?.secure_http(secure_http);
    let transport = HttpTransport { fetcher: &fetcher };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let repo = runtime.block_on(Repository::load(PACKAGIST_URL, &cache_dir, transport))?;

    let result = runtime.block_on(solver::solve_update(
        &repo,
        &root,
        prefer_stable,
        args.prefer_lowest,
    ))?;

    let lock = lock_json(&result, &composer_json)?;

    if args.dry_run {
        // `writeln!` to stdout directly, not `println!`, to satisfy the
        // `print_stdout` lint (`install.rs`'s `out` helper does the same).
        write!(std::io::stdout().lock(), "{lock}")?;
        return Ok(());
    }

    fs_err::write(project_dir.join("composer.lock"), lock)?;
    let _ = args.no_dev; // `--no-dev` only changes `install`'s selection, not the lock.
    Ok(())
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
/// lane's file to widen).
fn default_cache_dir() -> Result<PathBuf> {
    if let Ok(xdg) = std::env::var("XDG_CACHE_HOME")
        && !xdg.is_empty()
    {
        return Ok(PathBuf::from(xdg).join("vivace"));
    }
    let home = std::env::var("HOME").context("HOME is not set; pass --cache-dir")?;
    Ok(PathBuf::from(home).join(".cache").join("vivace"))
}
