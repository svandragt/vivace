//! `viv add`/`viv rm`: constraint synthesis (a `VersionSelector`
//! port), an in-place `composer.json` edit (add/remove a `require`/
//! `require-dev` entry, #288: no normalize, so every other byte stays and
//! `--no-normalize` is a deprecated no-op, same shape as `install`/
//! `dump-autoload`'s own, `docs/stability.md`), a partial update of the
//! touched package(s) (`docs/resolver-design.md` stage 5,
//! composer/composer#42), and a chained `install` (`--no-install` opts
//! out), matching `composer require`/`composer remove`'s own chain into
//! `Installer::run()` with `update` set.
//!
//! #145 replaced the format-preserving `JsonManipulator` port with a parse,
//! edit, serialize and normalize pass; #288 brought back a smaller
//! `manipulator` that finds each value's byte range with `RawValue` instead
//! of a hand-written scanner.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::{Map, Value, json};

use crate::install::{self, InstallArgs};
use crate::link::LinkMode;
use crate::normalize;
use crate::scripts;
use crate::solver::{
    self,
    pool_builder::{AdvisoryFilter, UpdateAllowMode},
};
use crate::update::audit_config_and_no_blocking;
use manipulator::Manipulator;

/// `viv add` flags.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's require flags"
)]
#[derive(Args, Debug, Clone)]
pub struct RequireArgs {
    /// `vendor/package` or `vendor/package:constraint`; a bare name gets a
    /// synthesised constraint (`VersionSelector::findRecommendedRequireVersion`).
    #[arg(required = true)]
    pub packages: Vec<String>,
    /// Add to `require-dev` instead of `require`.
    #[arg(long)]
    pub dev: bool,
    /// Edit `composer.json` only; don't resolve, touch `composer.lock`, or
    /// install (implies `--no-install`, matching `composer require`).
    #[arg(long = "no-update")]
    pub no_update: bool,
    /// Sort the touched require section alphabetically (platform packages
    /// first), even if `config.sort-packages` is not set.
    #[arg(long = "sort-packages")]
    pub sort_packages: bool,
    /// Prefer the lowest package versions that satisfy every constraint.
    #[arg(long)]
    pub prefer_lowest: bool,
    /// Prefer stable releases for the synthesised constraint and the
    /// partial update alike.
    #[arg(long)]
    pub prefer_stable: bool,
    /// Deprecated, no-op (#145, #288): `add` edits `composer.json` in place
    /// without normalizing it, so there is nothing to skip.
    #[arg(long)]
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
    /// (`composer require --no-install`): today's `viv add` behaviour.
    #[arg(long)]
    pub no_install: bool,
    /// Ignore every platform (`php`/`ext-*`/`lib-*`) requirement in the
    /// partial update's solve and in the chained install's platform
    /// check, as Composer's flag does (#242).
    #[arg(long)]
    pub ignore_platform_reqs: bool,
    /// Ignore one named platform requirement (`*` glob, repeatable) in the
    /// solve and in the chained install's platform check (#242).
    #[arg(long, value_name = "REQ")]
    pub ignore_platform_req: Vec<String>,
    /// Allows installing a version a known security advisory covers or a
    /// package Packagist marks abandoned, instead of blocking it by default
    /// (#175, `audit.block-insecure`/`audit.block-abandoned`). Also settable
    /// via `COMPOSER_NO_SECURITY_BLOCKING=1`.
    #[arg(long)]
    pub no_blocking: bool,
    /// Deprecated alias for `--no-blocking`.
    #[arg(long = "no-security-blocking", hide = true)]
    pub no_security_blocking: bool,
    /// Seconds a cached `/p2/` provider file may be served without
    /// revalidating it (#191). `0` (the default) always revalidates,
    /// matching today's behaviour. Also settable via `VIV_METADATA_TTL`
    /// (this flag wins); `--offline` always wins over either.
    #[arg(long)]
    pub metadata_ttl: Option<u64>,
    /// `type:url`, repeatable; appended to `repositories` (#315), e.g.
    /// `--repository path:packages/*`.
    #[arg(long = "repository", value_name = "TYPE:URL")]
    pub repository: Vec<String>,
}

/// `viv rm` flags.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's remove flags"
)]
#[derive(Args, Debug, Clone)]
pub struct RemoveArgs {
    /// `vendor/package`, one or more.
    #[arg(required = true)]
    pub packages: Vec<String>,
    /// Remove from `require-dev` instead of `require`.
    #[arg(long)]
    pub dev: bool,
    /// Edit `composer.json` only; don't resolve, touch `composer.lock`, or
    /// install (implies `--no-install`, matching `composer remove`).
    #[arg(long = "no-update")]
    pub no_update: bool,
    /// Deprecated, no-op (#145, #288): `rm` edits `composer.json` in place
    /// without normalizing it, so there is nothing to skip.
    #[arg(long)]
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
    /// (`composer remove --no-install`): today's `viv rm` behaviour.
    #[arg(long)]
    pub no_install: bool,
    /// Ignore every platform (`php`/`ext-*`/`lib-*`) requirement in the
    /// partial update's solve and in the chained install's platform
    /// check, as Composer's flag does (#242).
    #[arg(long)]
    pub ignore_platform_reqs: bool,
    /// Ignore one named platform requirement (`*` glob, repeatable) in the
    /// solve and in the chained install's platform check (#242).
    #[arg(long, value_name = "REQ")]
    pub ignore_platform_req: Vec<String>,
    /// Allows installing a version a known security advisory covers or a
    /// package Packagist marks abandoned, instead of blocking it by default
    /// (#175, `audit.block-insecure`/`audit.block-abandoned`). Also settable
    /// via `COMPOSER_NO_SECURITY_BLOCKING=1`.
    #[arg(long)]
    pub no_blocking: bool,
    /// Deprecated alias for `--no-blocking`.
    #[arg(long = "no-security-blocking", hide = true)]
    pub no_security_blocking: bool,
    /// Seconds a cached `/p2/` provider file may be served without
    /// revalidating it (#191). `0` (the default) always revalidates,
    /// matching today's behaviour. Also settable via `VIV_METADATA_TTL`
    /// (this flag wins); `--offline` always wins over either.
    #[arg(long)]
    pub metadata_ttl: Option<u64>,
}

pub fn run_require(args: &RequireArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&composer_json_path).context("reading composer.json")?;
    let mut root: Value = serde_json::from_str(&original).context("parsing composer.json")?;
    if !root.is_object() {
        bail!("composer.json must be a JSON object");
    }
    let mut manifest = Manipulator::new(&original)?;
    append_repositories(
        root.as_object_mut()
            .context("composer.json must be a JSON object")?,
        &args.repository,
    )?;
    if !args.repository.is_empty() {
        manifest.set_main_key("repositories", &root["repositories"])?;
    }

    let link_type = if args.dev { "require-dev" } else { "require" };
    let remove_key = if args.dev { "require" } else { "require-dev" };
    let sort_packages = args.sort_packages
        || root["config"]["sort-packages"]
            .as_bool()
            .unwrap_or_default();

    let mut requested = Vec::with_capacity(args.packages.len());
    for spec in &args.packages {
        let (name, constraint) = split_spec(spec);
        requested.push((name.to_ascii_lowercase(), constraint.map(str::to_string)));
    }

    let metadata_ttl = crate::update::metadata_ttl(args.metadata_ttl, offline);
    let mut allow_list = Vec::with_capacity(requested.len());
    for (name, constraint) in &requested {
        let constraint = if let Some(c) = constraint {
            c.clone()
        } else {
            synthesize_constraint(name, &root, &project_dir, cache_dir, offline, metadata_ttl)?
        };
        manifest.add_link(link_type, name, &constraint, sort_packages)?;
        // `RequireCommand::updateFileCleanly` always removes the same
        // package from the *other* require section too, moving it rather
        // than leaving a stale duplicate (no interactive confirmation
        // gate: that only decides which section a *warning* suggests,
        // never whether the move itself happens).
        manifest.remove_sub_node(remove_key, name)?;
        allow_list.push(name.clone());
    }
    manifest.remove_main_key_if_empty(remove_key)?;

    fs_err::write(&composer_json_path, manifest.contents())?;
    if args.no_normalize {
        warn_no_normalize_is_a_noop("add");
    }

    if args.no_update {
        return Ok(());
    }

    let lock_path = project_dir.join("composer.lock");
    let lock_backup = lock_path
        .exists()
        .then(|| fs_err::read(&lock_path))
        .transpose()?;
    let resolution_completed = Cell::new(false);
    let result = partial_update(
        &project_dir,
        cache_dir,
        offline,
        args.prefer_stable,
        args.prefer_lowest,
        &allow_list,
        UpdateAllowMode::OnlyListed,
        args.no_scripts,
        args.no_plugins,
        args.no_install,
        args.ignore_platform_reqs,
        &args.ignore_platform_req,
        args.no_blocking || args.no_security_blocking,
        metadata_ttl,
        &resolution_completed,
    );
    // `RequireCommand::execute` reverts only while the resolution is still
    // pending: once the lock is written, an install failure keeps both files,
    // which agree with each other.
    if result.is_err()
        && !resolution_completed.get()
        && let Err(err) = revert_composer_files(
            "Installation failed",
            &composer_json_path,
            &original,
            &lock_path,
            lock_backup.as_deref(),
        )
    {
        warn_out(&format!("Reverting failed: {err:#}"));
    }
    result
}

/// `RequireCommand::revertComposerFile`/`RemoveCommand`'s own revert: puts
/// back the bytes `composer.json` (and `composer.lock`, when the caller kept
/// one) had before the command ran.
fn revert_composer_files(
    failure: &str,
    composer_json_path: &Path,
    composer_json: &str,
    lock_path: &Path,
    lock: Option<&[u8]>,
) -> Result<()> {
    let files = if lock.is_some() {
        "./composer.json and ./composer.lock to their"
    } else {
        "./composer.json to its"
    };
    warn_out(&format!("\n{failure}, reverting {files} original content."));
    fs_err::write(composer_json_path, composer_json)?;
    if let Some(lock) = lock {
        fs_err::write(lock_path, lock)?;
    }
    Ok(())
}

pub fn run_remove(args: &RemoveArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&composer_json_path).context("reading composer.json")?;
    let mut manifest = Manipulator::new(&original)?;

    let link_type = if args.dev { "require-dev" } else { "require" };
    let mut allow_list = Vec::with_capacity(args.packages.len());
    for name in &args.packages {
        let name = name.to_ascii_lowercase();
        if manifest.remove_sub_node(link_type, &name)? {
            allow_list.push(name);
        } else {
            // Composer's own wording (`RemoveCommand::execute`), naming the
            // fact viv already knows so a typo doesn't look like a
            // successful removal (#241).
            warn_out(&format!(
                "{name} is not required in your composer.json and has not been removed"
            ));
        }
    }
    if allow_list.is_empty() {
        // Nothing was actually removed: leave composer.json untouched and
        // skip the update, so a no-op stays a no-op in the working tree
        // too (#241).
        return Ok(());
    }
    // `JsonConfigSource::removeLink` always follows `removeSubNode` with
    // this, dropping `require`/`require-dev` entirely once its last
    // package is gone.
    manifest.remove_main_key_if_empty(link_type)?;

    fs_err::write(&composer_json_path, manifest.contents())?;
    if args.no_normalize {
        warn_no_normalize_is_a_noop("rm");
    }

    if args.no_update {
        return Ok(());
    }

    // `RemoveCommand`'s own default: the removed package's dependents may
    // update too, but a dependency also directly required by root stays put
    // (`Request::UPDATE_LISTED_WITH_TRANSITIVE_DEPS_NO_ROOT_REQUIRE`).
    let resolution_completed = Cell::new(false);
    let result = partial_update(
        &project_dir,
        cache_dir,
        offline,
        false,
        false,
        &allow_list,
        UpdateAllowMode::WithTransitiveDepsNoRootRequire,
        args.no_scripts,
        args.no_plugins,
        args.no_install,
        args.ignore_platform_reqs,
        &args.ignore_platform_req,
        args.no_blocking || args.no_security_blocking,
        crate::update::metadata_ttl(args.metadata_ttl, offline),
        &resolution_completed,
    );
    // Same boundary as `run_require`; `RemoveCommand` never restores the lock.
    if result.is_err()
        && !resolution_completed.get()
        && let Err(err) = revert_composer_files(
            "Removal failed",
            &composer_json_path,
            &original,
            &project_dir.join("composer.lock"),
            None,
        )
    {
        warn_out(&format!("Reverting failed: {err:#}"));
    }
    result
}

/// Shared tail of both commands: reload the (just-edited) `composer.json`,
/// dispatch `pre-update-cmd`, solve a partial update allow-listing `names`,
/// write the lock, chain into `install` (`--no-install` opts out), and
/// dispatch `post-update-cmd` — matching `composer require`/`composer
/// remove`'s own chain into `Installer::run()` with `update` set.
/// `resolution_completed` is set once the lock is written, so a caller that
/// reverts its edit on failure knows when to stop (`PRE_OPERATIONS_EXEC`).
#[expect(
    clippy::too_many_arguments,
    clippy::fn_params_excessive_bools,
    reason = "require/remove's shared tail, no bundling win"
)]
pub(crate) fn partial_update(
    project_dir: &Path,
    cache_dir: Option<&Path>,
    offline: bool,
    prefer_stable: bool,
    prefer_lowest: bool,
    names: &[String],
    mode: UpdateAllowMode,
    no_scripts: bool,
    no_plugins: bool,
    no_install: bool,
    ignore_platform_reqs: bool,
    ignore_platform_req: &[String],
    no_blocking: bool,
    metadata_ttl: std::time::Duration,
    resolution_completed: &Cell<bool>,
) -> Result<()> {
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root: Value = serde_json::from_slice(&composer_json).context("parsing composer.json")?;
    let lock_path = project_dir.join("composer.lock");
    // #242: reaches this partial update's own solve, not just the follow-up
    // `install` chained after it.
    let ignore = install::ignore_platform(ignore_platform_reqs, ignore_platform_req);

    // `Installer::run`: `pre-update-cmd` dispatches before pool
    // building/solving even starts.
    let bin_dir = crate::update::bin_dir(&root);
    let mut scripts = scripts::Runner::new(&root, project_dir, &bin_dir, true, no_scripts);
    scripts.dispatch("pre-update-cmd")?;

    let prefer_stable = prefer_stable
        || root
            .get("prefer-stable")
            .and_then(Value::as_bool)
            .unwrap_or(false);

    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // #158: build the repository set the same way `update::solve` does
    // (`composer.json`'s own `repositories`, `#67`, plus the implicit
    // `packagist.org`) instead of always resolving against a hard-coded
    // `https://repo.packagist.org`, and pass `offline` through so a cache
    // miss errors cleanly instead of silently reaching the network.
    let fetcher = crate::update::build_fetcher(project_dir, &root, offline)?;
    let repo = runtime.block_on(crate::update::build_repository(
        project_dir,
        &root,
        &cache_dir,
        &fetcher,
        metadata_ttl,
    ))?;
    let (audit_config, no_blocking) = audit_config_and_no_blocking(&root, no_blocking)?;
    // #182: same per-repository advertised endpoints `update::solve` uses.
    let advisory_endpoints = repo.security_advisory_urls();
    let advisories_transport = crate::audit::HttpTransport { fetcher: &fetcher };
    let advisories = Some(AdvisoryFilter {
        transport: &advisories_transport,
        endpoints: &advisory_endpoints,
        audit: &audit_config,
        no_blocking,
        prefetched: None,
        // #197: same knob and cache dir `update::solve` already resolves.
        cache_dir: Some(&cache_dir),
        metadata_ttl,
    });

    // A brand new `composer.json` (no lock yet) cannot do a partial update
    // (`PoolBuilder::buildPool` requires a locked repository); fall back to
    // a full update, matching `RequireCommand`'s own "no lock present" skip
    // of `setUpdateAllowList`.
    let result = if lock_path.exists() {
        let lock_bytes = fs_err::read(&lock_path)?;
        let lock: Value = serde_json::from_slice(&lock_bytes).context("parsing composer.lock")?;
        let locked_by_name = locked_packages_by_name(&lock);
        runtime.block_on(solver::solve_partial_update_seeded(
            &repo,
            &root,
            project_dir,
            prefer_stable,
            prefer_lowest,
            &locked_by_name,
            names,
            mode,
            &[],
            HashMap::new(),
            advisories,
            Some(&cache_dir),
            None,
            &ignore,
        ))?
    } else {
        runtime.block_on(solver::solve_update_seeded(
            &repo,
            &root,
            project_dir,
            prefer_stable,
            prefer_lowest,
            &[],
            HashMap::new(),
            &HashMap::new(),
            advisories,
            Some(&cache_dir),
            None,
            &ignore,
        ))?
    };
    // #177: `repo` is never read again below (see `update::forget_repo`'s
    // own doc for why forgetting it here is safe and worth it).
    crate::update::forget_repo(repo);

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
    let lock =
        crate::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)?;
    // #156: same block `viv update` prints, since `add`/`rm` go through
    // this same lock-writing path; always followed by "Writing lock file",
    // since this function (unlike `update::run`) has no `--dry-run`.
    crate::update::print_lock_operations(&lock_path, &result.non_dev, &result.dev, true)?;
    fs_err::write(&lock_path, lock)?;
    resolution_completed.set(true);

    if !no_install {
        let install_args = InstallArgs {
            no_dev: false,
            dry_run: false,
            link_mode: LinkMode::default(),
            adopt: false,
            project_dir: project_dir.to_path_buf(),
            optimize_autoloader: false,
            classmap_authoritative: false,
            apcu_autoloader: false,
            apcu_autoloader_prefix: None,
            ignore_platform_reqs,
            ignore_platform_req: ignore_platform_req.to_vec(),
            no_scripts,
            no_normalize: false,
            no_plugins,
            no_progress: false,
            no_interaction: false,
            prefer_dist: false,
            no_suggest: false,
        };
        install::run_after_update(&install_args, Some(&cache_dir), offline)?;
    }

    scripts.dispatch("post-update-cmd")?;
    Ok(())
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr` lint
/// (`install.rs`'s own `warn_out` does the same).
fn warn_out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

/// `--no-normalize`'s deprecation notice on `add`/`rm` (#145, #288): neither
/// normalizes `composer.json` any more, same shape as `install.rs`'s own
/// `warn_no_normalize_is_a_noop` for `install`/`dump-autoload` (#95).
fn warn_no_normalize_is_a_noop(command: &str) {
    warn_out(&normalize::no_normalize_is_a_noop_message(
        command,
        "0.8",
        &format!("{command} edits composer.json in place and does not normalize it"),
    ));
}

/// Same merge as `update::locked_packages_by_name` (private there); kept as
/// its own small copy rather than widened, since `update.rs`'s function
/// isn't this lane's call to make `pub(crate)` on top of.
fn locked_packages_by_name(lock: &Value) -> std::collections::HashMap<String, Value> {
    let mut by_name = std::collections::HashMap::new();
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

/// `RequireCommand::execute`'s `$preferredStability` (`getPreferStable() ?
/// 'stable' : getMinimumStability()`).
/// The `None`-constraint branch of `RequireCommand::determineRequirements`:
/// fetch `name`'s versions and turn the best candidate into a `^`-style
/// constraint (`version_selector::find_recommended_constraint`).
pub(crate) fn synthesize_constraint(
    name: &str,
    root: &Value,
    project_dir: &Path,
    cache_dir: Option<&Path>,
    offline: bool,
    metadata_ttl: std::time::Duration,
) -> Result<String> {
    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // #160: resolve through composer.json's own `repositories` and honour
    // `--offline`, the same fetcher/repository construction `partial_update`
    // uses since #158, instead of always hitting `https://repo.packagist.org`.
    let fetcher = crate::update::build_fetcher(project_dir, root, offline)?;
    let repo = runtime.block_on(crate::update::build_repository(
        project_dir,
        root,
        &cache_dir,
        &fetcher,
        metadata_ttl,
    ))?;
    let preferred_stability = preferred_stability(root);
    runtime
        .block_on(version_selector::find_recommended_constraint(
            &repo,
            name,
            preferred_stability,
        ))?
        .with_context(|| format!("could not find a version of {name}"))
}

fn preferred_stability(root: &Value) -> &'static str {
    if root
        .get("prefer-stable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return "stable";
    }
    match root.get("minimum-stability").and_then(Value::as_str) {
        Some("dev") => "dev",
        Some(s) if s.eq_ignore_ascii_case("rc") => "RC",
        Some("beta") => "beta",
        Some("alpha") => "alpha",
        _ => "stable",
    }
}

/// `vendor/package` or `vendor/package:constraint` (`composer require`'s
/// own CLI argument shape; the `=` separator Composer also accepts is not
/// supported here, only the more common `:`).
pub(crate) fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((name, constraint)) => (name, Some(constraint)),
        None => (spec, None),
    }
}

/// `Package\Version\VersionSelector`, cut down to what constraint synthesis
/// needs: no platform-requirement filtering (`--ignore-platform-req(s)`
/// isn't wired for `require` at this stage) and no branch-alias dev-version
/// handling (`findRecommendedRequireVersion`'s `isDev()` branch) — a bare
/// `viv add vendor/pkg` on a stable release is the common case this
/// stage's fixtures exercise.
mod version_selector {
    use anyhow::Result;

    use crate::repository::{DevAcceptance, PackageVersion, Repository, Transport};
    use crate::semver;

    /// `VersionSelector::findBestCandidate` then
    /// `findRecommendedRequireVersion`/`transformVersion`, combined: fetch
    /// every version, pick the best one at or above `preferred_stability`,
    /// and turn it into a `^`-constraint (or return the bare pretty version
    /// for a `dev-*` branch, which `transformVersion` never touches).
    pub(super) async fn find_recommended_constraint<T: Transport>(
        repo: &Repository<T>,
        name: &str,
        preferred_stability: &str,
    ) -> Result<Option<String>> {
        let dev = if preferred_stability == "dev" {
            DevAcceptance::Both
        } else {
            DevAcceptance::NonDevOnly
        };
        let versions = repo.load_package(name, dev).await?;
        let Some(best) = pick_best(&versions, preferred_stability)? else {
            return Ok(None);
        };
        Ok(Some(recommended_constraint(&best)?))
    }

    fn pick_best(
        versions: &[PackageVersion],
        preferred_stability: &str,
    ) -> Result<Option<PackageVersion>> {
        let preferred_rank = stability_rank(preferred_stability);
        let mut best: Option<(&PackageVersion, semver::NormalizedVersion, &'static str)> = None;
        for pv in versions {
            let normalized = semver::normalize(&pv.version_normalized)?;
            let stability = semver::stability(normalized.as_str());
            let rank = stability_rank(stability);
            let candidate_is_worse_than_preferred = preferred_rank < rank;
            let better = match &best {
                None => true,
                Some((_, best_version, best_stability)) => {
                    let best_rank = stability_rank(best_stability);
                    let best_is_worse_than_preferred = preferred_rank < best_rank;
                    if candidate_is_worse_than_preferred && !best_is_worse_than_preferred {
                        false
                    } else if !candidate_is_worse_than_preferred && best_is_worse_than_preferred {
                        true
                    } else {
                        semver::compare(&normalized, best_version) == std::cmp::Ordering::Greater
                    }
                }
            };
            if better {
                best = Some((pv, normalized, stability));
            }
        }
        Ok(best.map(|(pv, ..)| pv.clone()))
    }

    fn stability_rank(stability: &str) -> u8 {
        match stability {
            "stable" => 0,
            "RC" => 5,
            "beta" => 10,
            "alpha" => 15,
            // "dev" and anything unrecognised both rank last.
            _ => 20,
        }
    }

    /// `VersionSelector::transformVersion`: `1.2.1` -> `^1.2`, `0.1.2` ->
    /// `^0.1.2` (a `0.x` major keeps the patch component, matching
    /// `unset($semanticVersionParts[3])` only), `2.0-beta.1` -> `^2.0@beta`.
    /// A `dev-*` pretty version (no numeric shape to transform) is
    /// returned untouched, matching `findRecommendedRequireVersion`'s own
    /// `isDev()` early exit (branch-alias resolution is not ported, see the
    /// module doc).
    pub(super) fn recommended_constraint(pv: &PackageVersion) -> Result<String> {
        if pv.version.starts_with("dev-") || pv.version.ends_with("-dev") {
            return Ok(pv.version.clone());
        }
        let normalized = semver::normalize(&pv.version_normalized)?;
        let stability = semver::stability(normalized.as_str());
        let parts: Vec<&str> = normalized.as_str().split('.').collect();
        if parts.len() != 4 || !parts[3].chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return Ok(pv.version.clone());
        }
        let mut kept = if parts[0] == "0" {
            vec![parts[0], parts[1], parts[2]]
        } else {
            vec![parts[0], parts[1]]
        };
        let mut version = kept.join(".");
        if stability != "stable" {
            version.push('@');
            version.push_str(stability);
        }
        let _ = &mut kept;
        Ok(format!("^{version}"))
    }
}

/// `JsonConfigSource::addLink` for the callers that run `maybe_normalize`
/// afterwards (`workspace add`): no format-preserving edit, just parse into
/// a [`Value`], edit the map, and let `normalize` reindent and resort
/// (including the require/require-dev section itself, so there is no
/// `sort-packages` handling to port either). `add`/`rm` edit the text in
/// place instead ([`manipulator`], #288).
pub(crate) fn add_link(
    root: &mut Value,
    link_type: &str,
    package: &str,
    constraint: &str,
) -> Result<()> {
    let obj = root
        .as_object_mut()
        .context("composer.json must be a JSON object")?;
    let links = obj
        .entry(link_type)
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .with_context(|| format!("{link_type} must be a JSON object"))?;
    if let Some(existing_key) = links
        .keys()
        .find(|k| k.eq_ignore_ascii_case(package))
        .cloned()
    {
        links.remove(&existing_key);
    }
    links.insert(package.to_string(), Value::String(constraint.to_string()));
    Ok(())
}

/// `--repository type:url` (#315, repeatable on `viv init`/`viv add`) and
/// `workspace init`'s own per-pattern `path` entries: append one entry to
/// `repositories`, creating the array if it isn't there yet. `spec` splits
/// on the first `:` only, so a URL that has one of its own (`https://...`)
/// still parses as `("composer", "https://...")`.
pub(crate) fn append_repositories(root: &mut Map<String, Value>, specs: &[String]) -> Result<()> {
    if specs.is_empty() {
        return Ok(());
    }
    let repositories = root
        .entry("repositories")
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .context("\"repositories\" must be an array")?;
    for spec in specs {
        let (repo_type, url) = spec
            .split_once(':')
            .with_context(|| format!("--repository {spec:?} must be \"type:url\""))?;
        repositories.push(json!({"type": repo_type, "url": url}));
    }
    Ok(())
}

/// `JsonManipulator::removeSubNode`, `main_node` always `require`/
/// `require-dev` here. A no-op if `main_node` is absent or doesn't hold
/// `package` (case-insensitively). Returns whether anything was actually
/// removed, so `run_remove` can tell a real removal from a typo (#241).
/// `pub(crate)`: `validate.rs`'s own `--fix` (#262) reuses this same
/// removal, `main_node` there ranging over link and scripts sections too.
pub(crate) fn remove_sub_node(root: &mut Value, main_node: &str, package: &str) -> bool {
    let Some(Value::Object(links)) = root.get_mut(main_node) else {
        return false;
    };
    let Some(existing_key) = links
        .keys()
        .find(|k| k.eq_ignore_ascii_case(package))
        .cloned()
    else {
        return false;
    };
    links.remove(&existing_key);
    true
}

/// `JsonManipulator::removeMainKeyIfEmpty`: drop `key` from the root object
/// if its value parsed to an empty object or array. `pub(crate)`: shared with
/// `remove_sub_node` above.
pub(crate) fn remove_main_key_if_empty(root: &mut Value, key: &str) {
    let is_empty = match root.get(key) {
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        _ => false,
    };
    if is_empty && let Some(obj) = root.as_object_mut() {
        obj.remove(key);
    }
}

/// Serialize the edited root and write it to `path`: any formatting here is
/// throwaway, `maybe_normalize` immediately reindents/resorts it, so a
/// plain pretty-printer (not a format-preserving one) is enough.
/// `pub(crate)`: `update.rs`'s own `bump-after-update` rewrite (#205) reuses
/// this same write-then-normalize path rather than a second manifest
/// writer.
pub(crate) fn write_composer_json(path: &Path, root: &Value) -> Result<()> {
    let mut contents = serde_json::to_string_pretty(root)?;
    contents.push('\n');
    fs_err::write(path, contents)?;
    Ok(())
}

/// `Composer\Json\JsonManipulator`, cut down to what `add`/`rm` edit (#288):
/// the entries of a `require`/`require-dev` section and one top-level key.
/// It edits the file's own text, so every byte it does not touch stays. Each
/// value's byte range comes from a `RawValue`, which borrows its slice from
/// the text, in place of Composer's recursive regex.
mod manipulator {
    use std::collections::HashMap;
    use std::ops::Range;

    use anyhow::{Context, Result};
    use serde_json::value::RawValue;
    use serde_json::{Map, Value, json};

    pub(super) struct Manipulator {
        text: String,
        newline: &'static str,
        indent: String,
    }

    impl Manipulator {
        pub(super) fn new(text: &str) -> Result<Manipulator> {
            let manipulator = Manipulator {
                text: text.to_string(),
                newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
                indent: crate::normalize::detect_indent(text),
            };
            manipulator
                .pairs(&manipulator.top())
                .context("parsing composer.json")?;
            Ok(manipulator)
        }

        pub(super) fn contents(&self) -> &str {
            &self.text
        }

        /// `JsonManipulator::addLink`: set `package`'s constraint in
        /// `link_type`, keeping its place if it is there and appending it
        /// otherwise; `sort_packages` then sorts the whole section.
        pub(super) fn add_link(
            &mut self,
            link_type: &str,
            package: &str,
            constraint: &str,
            sort_packages: bool,
        ) -> Result<()> {
            let new_section = self.render(&json!({ package: constraint }), 1)?;
            let section = match self.value_span(link_type)? {
                None => {
                    let top = self.top();
                    return self.append_pair(&top, 0, link_type, &new_section);
                }
                // PHP writes an empty section as `[]`.
                Some(span) if self.text[span.clone()].starts_with('[') => {
                    self.text.replace_range(span, &new_section);
                    return Ok(());
                }
                Some(span) => span,
            };
            let existing = self
                .pairs(&section)?
                .into_iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(package));
            if let Some((_, value)) = existing {
                self.text.replace_range(value, &quote(constraint));
            } else {
                self.append_pair(&section, 1, package, &quote(constraint))?;
            }
            if sort_packages && let Some(section) = self.value_span(link_type)? {
                let mut links: Map<String, Value> =
                    serde_json::from_str(&self.text[section.clone()])?;
                crate::normalize::sort_package_links(&mut links);
                let sorted = self.render(&Value::Object(links), 1)?;
                self.text.replace_range(section, &sorted);
            }
            Ok(())
        }

        /// `JsonManipulator::removeSubNode`: drop `package` from
        /// `main_node`, whether it was there or not.
        pub(super) fn remove_sub_node(&mut self, main_node: &str, package: &str) -> Result<bool> {
            let Some(section) = self.value_span(main_node)? else {
                return Ok(false);
            };
            if self.text[section.clone()].starts_with('[') {
                return Ok(false);
            }
            let Some(index) = self
                .pairs(&section)?
                .iter()
                .position(|(key, _)| key.eq_ignore_ascii_case(package))
            else {
                return Ok(false);
            };
            self.remove_pair(&section, 1, index)?;
            Ok(true)
        }

        /// `JsonManipulator::removeMainKeyIfEmpty`.
        pub(super) fn remove_main_key_if_empty(&mut self, key: &str) -> Result<()> {
            let top = self.top();
            let pairs = self.pairs(&top)?;
            let Some(index) = pairs.iter().position(|(k, _)| k == key) else {
                return Ok(());
            };
            let is_empty = match serde_json::from_str(&self.text[pairs[index].1.clone()])? {
                Value::Object(object) => object.is_empty(),
                Value::Array(array) => array.is_empty(),
                _ => false,
            };
            if is_empty {
                self.remove_pair(&top, 0, index)?;
            }
            Ok(())
        }

        /// `JsonManipulator::addMainKey`: replace the value of a top-level
        /// key, or append the key if it is absent.
        pub(super) fn set_main_key(&mut self, key: &str, value: &Value) -> Result<()> {
            let rendered = self.render(value, 1)?;
            if let Some(span) = self.value_span(key)? {
                self.text.replace_range(span, &rendered);
            } else {
                let top = self.top();
                self.append_pair(&top, 0, key, &rendered)?;
            }
            Ok(())
        }

        fn top(&self) -> Range<usize> {
            0..self.text.len()
        }

        /// The byte range of a top-level key's value. `HashMap` keeps the
        /// last of two equal keys, as `json_decode` does.
        fn value_span(&self, key: &str) -> Result<Option<Range<usize>>> {
            Ok(self
                .pairs(&self.top())?
                .into_iter()
                .find(|(k, _)| k == key)
                .map(|(_, span)| span))
        }

        /// Every pair of the object at `object`, as its key and the byte
        /// range of its value, in source order.
        fn pairs(&self, object: &Range<usize>) -> Result<Vec<(String, Range<usize>)>> {
            let slice = &self.text[object.clone()];
            let map: HashMap<String, &RawValue> = serde_json::from_str(slice)?;
            let mut pairs: Vec<_> = map
                .into_iter()
                .map(|(key, raw)| {
                    let start = object.start + raw.get().as_ptr().addr() - slice.as_ptr().addr();
                    (key, start..start + raw.get().len())
                })
                .collect();
            pairs.sort_by_key(|(_, span)| span.start);
            Ok(pairs)
        }

        /// The byte offsets of the `{` and `}` around the object at `object`.
        fn braces(&self, object: &Range<usize>) -> (usize, usize) {
            let slice = &self.text[object.clone()];
            (
                object.start + slice.len() - slice.trim_start().len(),
                object.start + slice.trim_end().len() - 1,
            )
        }

        /// `JsonManipulator::format`: `value` pretty-printed with the file's
        /// indent and line ending, for a key `depth` levels down.
        fn render(&self, value: &Value, depth: usize) -> Result<String> {
            let mut buf = Vec::new();
            let formatter = serde_json::ser::PrettyFormatter::with_indent(self.indent.as_bytes());
            let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
            serde::Serialize::serialize(value, &mut serializer)?;
            let line_break = format!("{}{}", self.newline, self.indent.repeat(depth));
            Ok(String::from_utf8(buf)?.replace('\n', &line_break))
        }

        /// Add a last pair to the object at `object`, whose own key sits
        /// `depth` levels down: after the last value, or inside an empty
        /// object.
        fn append_pair(
            &mut self,
            object: &Range<usize>,
            depth: usize,
            key: &str,
            value: &str,
        ) -> Result<()> {
            let pair = format!("{}{}: {value}", self.indent.repeat(depth + 1), quote(key));
            if let Some((_, last)) = self.pairs(object)?.pop() {
                self.text
                    .insert_str(last.end, &format!(",{}{pair}", self.newline));
            } else {
                let (open, close) = self.braces(object);
                let object = format!(
                    "{{{}{pair}{}{}}}",
                    self.newline,
                    self.newline,
                    self.indent.repeat(depth)
                );
                self.text.replace_range(open..=close, &object);
            }
            Ok(())
        }

        /// Remove pair `index` of the object at `object` along with the
        /// comma that joined it to its neighbour. The only pair leaves a
        /// line break and the closing brace's indent.
        fn remove_pair(&mut self, object: &Range<usize>, depth: usize, index: usize) -> Result<()> {
            let pairs = self.pairs(object)?;
            let (open, close) = self.braces(object);
            if pairs.len() == 1 {
                let empty = format!("{}{}", self.newline, self.indent.repeat(depth));
                self.text.replace_range(open + 1..close, &empty);
                return Ok(());
            }
            let before = if index == 0 {
                open + 1
            } else {
                pairs[index - 1].1.end
            };
            let span = if index + 1 < pairs.len() {
                self.next_key(before)..self.next_key(pairs[index].1.end)
            } else {
                before..pairs[index].1.end
            };
            self.text.replace_range(span, "");
            Ok(())
        }

        /// Where the next key starts after byte `from`: past the whitespace
        /// and the comma, if there is one, and the whitespace after that.
        fn next_key(&self, from: usize) -> usize {
            let skip_whitespace =
                |at: usize| at + self.text[at..].len() - self.text[at..].trim_start().len();
            let at = skip_whitespace(from);
            if self.text[at..].starts_with(',') {
                skip_whitespace(at + 1)
            } else {
                at
            }
        }
    }

    fn quote(string: &str) -> String {
        Value::from(string).to_string()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::manipulator::Manipulator;
    use super::version_selector;
    use super::{add_link, remove_main_key_if_empty, remove_sub_node, run_require};

    #[test]
    fn add_link_creates_a_brand_new_require_section() {
        let mut root = json!({"name": "acme/pkg"});
        add_link(&mut root, "require", "psr/log", "^3.0").unwrap();
        assert_eq!(
            root,
            json!({"name": "acme/pkg", "require": {"psr/log": "^3.0"}})
        );
    }

    #[test]
    fn add_link_replaces_an_existing_constraint() {
        let mut root = json!({"require": {"psr/log": "^2.0"}});
        add_link(&mut root, "require", "psr/log", "^3.0").unwrap();
        assert_eq!(root, json!({"require": {"psr/log": "^3.0"}}));
    }

    #[test]
    fn remove_sub_node_leaves_other_entries_untouched() {
        let mut root = json!({"require": {"monolog/monolog": "^3.0", "psr/log": "^3.0"}});
        remove_sub_node(&mut root, "require", "psr/log");
        assert_eq!(root, json!({"require": {"monolog/monolog": "^3.0"}}));
    }

    #[test]
    fn remove_main_key_if_empty_drops_the_now_empty_section() {
        let mut root = json!({"require-dev": {}});
        remove_main_key_if_empty(&mut root, "require-dev");
        assert_eq!(root, json!({}));
    }

    /// A `composer.json` with a duplicate top-level `require` key isn't
    /// valid JSON per any spec, but `serde_json` (like Composer's own
    /// `json_decode`) accepts it, last occurrence winning. #145: parsing
    /// straight into a [`super::Value`] rather than splicing text means the
    /// edit always lands on that surviving section, unlike the old
    /// text-splice manipulator, which edited whichever occurrence its
    /// scanner found first — silently discarded once `normalize`'s own
    /// `serde_json` parse then kept the *other* one instead.
    #[test]
    fn add_link_edits_the_surviving_section_of_a_duplicate_key() {
        let mut root: super::Value = serde_json::from_str(
            r#"{"require": {"old/pkg": "^1.0"}, "require": {"psr/log": "^2.0"}}"#,
        )
        .unwrap();
        add_link(&mut root, "require", "monolog/monolog", "^3.0").unwrap();
        assert_eq!(
            root,
            json!({"require": {"psr/log": "^2.0", "monolog/monolog": "^3.0"}})
        );
    }

    const MANIFEST: &str = r#"{
    "type": "project",
    "require": {
        "psr/log": "^3.0",
        "ext-json": "*"
    },
    "config": {"a": 1}
}
"#;

    fn edited(json: &str, edit: impl FnOnce(&mut Manipulator)) -> String {
        let mut manifest = Manipulator::new(json).unwrap();
        edit(&mut manifest);
        manifest.contents().to_string()
    }

    #[test]
    fn manipulator_appends_a_link_and_changes_nothing_else() {
        let got = edited(MANIFEST, |m| {
            m.add_link("require", "monolog/monolog", "^3.0", false)
                .unwrap();
        });
        assert_eq!(
            got,
            MANIFEST.replace(
                "\"ext-json\": \"*\"",
                "\"ext-json\": \"*\",\n        \"monolog/monolog\": \"^3.0\""
            )
        );
    }

    #[test]
    fn manipulator_replaces_a_constraint_in_place_keeping_the_key_as_written() {
        let got = edited(MANIFEST, |m| {
            m.add_link("require", "PSR/Log", "^2.0", false).unwrap();
        });
        assert_eq!(got, MANIFEST.replace("^3.0", "^2.0"));
    }

    /// `config.sort-packages`/`--sort-packages`: Composer re-sorts the whole
    /// section, platform packages first.
    #[test]
    fn manipulator_sorts_the_section_when_asked() {
        let got = edited(MANIFEST, |m| {
            m.add_link("require", "monolog/monolog", "^3.0", true)
                .unwrap();
        });
        assert_eq!(
            got,
            MANIFEST.replace(
                "\"psr/log\": \"^3.0\",\n        \"ext-json\": \"*\"",
                "\"ext-json\": \"*\",\n        \"monolog/monolog\": \"^3.0\",\n        \"psr/log\": \"^3.0\""
            )
        );
    }

    #[test]
    fn manipulator_adds_a_missing_section_at_the_end() {
        let got = edited(MANIFEST, |m| {
            m.add_link("require-dev", "phpunit/phpunit", "^11.0", false)
                .unwrap();
        });
        assert_eq!(
            got,
            MANIFEST.replace(
                "\"config\": {\"a\": 1}\n",
                "\"config\": {\"a\": 1},\n    \"require-dev\": {\n        \"phpunit/phpunit\": \"^11.0\"\n    }\n"
            )
        );
        let got = edited("{}", |m| {
            m.add_link("require", "psr/log", "^3.0", false).unwrap();
        });
        assert_eq!(
            got,
            "{\n    \"require\": {\n        \"psr/log\": \"^3.0\"\n    }\n}"
        );
    }

    /// PHP writes an empty `require` as `[]`; Composer turns it into an object.
    #[test]
    fn manipulator_fills_an_empty_array_section() {
        let got = edited("{\n    \"require\": []\n}\n", |m| {
            m.add_link("require", "psr/log", "^3.0", false).unwrap();
        });
        assert_eq!(
            got,
            "{\n    \"require\": {\n        \"psr/log\": \"^3.0\"\n    }\n}\n"
        );
    }

    #[test]
    fn manipulator_removes_a_first_middle_last_and_only_entry() {
        let three = "{\n    \"require\": {\n        \"a/a\": \"1\",\n        \"b/b\": \"2\",\n        \"c/c\": \"3\"\n    },\n    \"name\": \"x/y\"\n}\n";
        for (name, kept) in [
            ("a/a", "\"b/b\": \"2\",\n        \"c/c\": \"3\""),
            ("b/b", "\"a/a\": \"1\",\n        \"c/c\": \"3\""),
            ("c/c", "\"a/a\": \"1\",\n        \"b/b\": \"2\""),
        ] {
            let got = edited(three, |m| {
                assert!(m.remove_sub_node("require", name).unwrap());
            });
            assert_eq!(
                got,
                format!(
                    "{{\n    \"require\": {{\n        {kept}\n    }},\n    \"name\": \"x/y\"\n}}\n"
                )
            );
        }

        let only =
            "{\n    \"name\": \"x/y\",\n    \"require-dev\": {\n        \"a/a\": \"1\"\n    }\n}\n";
        let got = edited(only, |m| {
            assert!(m.remove_sub_node("require-dev", "a/a").unwrap());
            assert!(!m.remove_sub_node("require-dev", "b/b").unwrap());
            m.remove_main_key_if_empty("require-dev").unwrap();
        });
        assert_eq!(got, "{\n    \"name\": \"x/y\"\n}\n");
    }

    #[test]
    fn manipulator_keeps_the_files_line_endings() {
        let crlf = MANIFEST.replace('\n', "\r\n");
        let got = edited(&crlf, |m| {
            m.add_link("require", "monolog/monolog", "^3.0", false)
                .unwrap();
            m.add_link("require-dev", "phpunit/phpunit", "^11.0", false)
                .unwrap();
        });
        assert!(!got.replace("\r\n", "").contains('\n'), "{got:?}");
    }

    #[test]
    fn manipulator_sets_a_main_key_in_place_or_at_the_end() {
        let repositories = json!([{"type": "path", "url": "pkg"}]);
        let got = edited(MANIFEST, |m| {
            m.set_main_key("repositories", &repositories).unwrap();
        });
        assert_eq!(
            got,
            MANIFEST.replace(
                "\"config\": {\"a\": 1}\n",
                "\"config\": {\"a\": 1},\n    \"repositories\": [\n        {\n            \"type\": \"path\",\n            \"url\": \"pkg\"\n        }\n    ]\n"
            )
        );
        let again = edited(&got, |m| {
            m.set_main_key("repositories", &json!([])).unwrap();
        });
        assert_eq!(again, got.replace(
            "[\n        {\n            \"type\": \"path\",\n            \"url\": \"pkg\"\n        }\n    ]",
            "[]"
        ));
    }

    /// Non-UTF-8 bytes in `composer.json` fail at the same
    /// `fs_err::read_to_string` call the old manipulator's input also had
    /// to pass through first: no behaviour change (#145).
    #[test]
    fn run_require_errors_cleanly_on_non_utf8_composer_json() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join("composer.json"), [0xff, 0xfe, b'{']).unwrap();
        let args = super::RequireArgs {
            packages: vec!["acme/pkg:^1.0".to_string()],
            dev: false,
            no_update: true,
            sort_packages: false,
            prefer_lowest: false,
            prefer_stable: false,
            no_normalize: false,
            project_dir: dir.path().to_path_buf(),
            no_scripts: true,
            no_plugins: true,
            no_install: true,
            ignore_platform_reqs: false,
            ignore_platform_req: Vec::new(),
            no_blocking: false,
            no_security_blocking: false,
            metadata_ttl: None,
            repository: Vec::new(),
        };
        let err = run_require(&args, None, true).unwrap_err();
        assert!(err.to_string().contains("composer.json"), "{err:#}");
    }

    /// `VersionSelector::transformVersion`'s worked examples from its own
    /// doc comment.
    #[test]
    fn recommended_constraint_transforms_versions() {
        let pv = |version: &str, normalized: &str| {
            crate::repository::PackageVersion::from_owned_value(serde_json::json!({
                "name": "acme/pkg",
                "version": version,
                "version_normalized": normalized,
            }))
            .unwrap()
        };
        assert_eq!(
            version_selector::recommended_constraint(&pv("1.2.1", "1.2.1.0")).unwrap(),
            "^1.2"
        );
        assert_eq!(
            version_selector::recommended_constraint(&pv("v3.2.1", "3.2.1.0")).unwrap(),
            "^3.2"
        );
        assert_eq!(
            version_selector::recommended_constraint(&pv("2.0-beta.1", "2.0.0.0-beta1")).unwrap(),
            "^2.0@beta"
        );
        assert_eq!(
            version_selector::recommended_constraint(&pv("dev-master", "dev-master")).unwrap(),
            "dev-master"
        );
    }
}
