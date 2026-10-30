//! `viv add`/`viv rm`: constraint synthesis (a `VersionSelector`
//! port), a `composer.json` edit (add/remove a `require`/`require-dev`
//! entry) that is normalized by default (#145), a partial update of the
//! touched package(s) (`docs/resolver-design.md` stage 5,
//! composer/composer#42), and a chained `install` (`--no-install` opts
//! out), matching `composer require`/`composer remove`'s own chain into
//! `Installer::run()` with `update` set.
//!
//! The default edit is a plain parse-map-serialize followed by the
//! normalize pass, so the intermediate formatting never survives to disk.
//! `--no-normalize` (#400) takes the other path: `manipulator`, a
//! `JsonManipulator` port that splices the change into the original text and
//! leaves every other byte alone. #145 had removed that port; #400 brought
//! it back for this one flag.

use std::cell::Cell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::{Map, Value, json};

use crate::install::{self, InstallArgs};
use crate::link::LinkArgs;
use crate::normalize;
use crate::scripts;
use crate::solver::{
    self,
    pool_builder::{AdvisoryFilter, UpdateAllowMode},
};
use crate::update::audit_config_and_no_blocking;

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
    /// first), even if `config.sort-packages` is not set. Only
    /// `--no-normalize` needs it: normalizing always sorts.
    #[arg(long = "sort-packages")]
    pub sort_packages: bool,
    /// Prefer the lowest package versions that satisfy every constraint.
    #[arg(long)]
    pub prefer_lowest: bool,
    /// Prefer stable releases for the synthesised constraint and the
    /// partial update alike.
    #[arg(long)]
    pub prefer_stable: bool,
    /// Edit only the `require`/`require-dev` entries and leave every other
    /// byte of `composer.json` as it was, instead of normalizing the file.
    /// Not combinable with `--repository`.
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
    #[command(flatten)]
    pub link: LinkArgs,
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
    /// Edit only the `require`/`require-dev` entries and leave every other
    /// byte of `composer.json` as it was, instead of normalizing the file.
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
    #[command(flatten)]
    pub link: LinkArgs,
}

pub fn run_require(args: &RequireArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    // `repositories` is outside what `manipulator` edits.
    if args.no_normalize && !args.repository.is_empty() {
        bail!("--repository edits `repositories`, so it cannot be combined with --no-normalize");
    }
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&composer_json_path).context("reading composer.json")?;
    let mut root: Value = serde_json::from_str(&original).context("parsing composer.json")?;
    if !root.is_object() {
        bail!("composer.json must be a JSON object");
    }
    append_repositories(
        root.as_object_mut()
            .context("composer.json must be a JSON object")?,
        &args.repository,
    )?;

    let link_type = if args.dev { "require-dev" } else { "require" };
    let remove_key = if args.dev { "require" } else { "require-dev" };
    let sort_packages = args.sort_packages
        || root
            .pointer("/config/sort-packages")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let mut manipulator = args
        .no_normalize
        .then(|| manipulator::Manipulator::new(&original))
        .transpose()?;

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
        add_link(&mut root, link_type, name, &constraint)?;
        // `RequireCommand::updateFileCleanly` always removes the same
        // package from the *other* require section too, moving it rather
        // than leaving a stale duplicate (no interactive confirmation
        // gate: that only decides which section a *warning* suggests,
        // never whether the move itself happens).
        remove_sub_node(&mut root, remove_key, name);
        if let Some(manipulator) = &mut manipulator {
            manipulator.add_link(link_type, name, &constraint, sort_packages)?;
            manipulator.remove_sub_node(remove_key, name)?;
        }
        allow_list.push(name.clone());
    }
    remove_main_key_if_empty(&mut root, remove_key);
    if let Some(manipulator) = &mut manipulator {
        manipulator.remove_main_key_if_empty(remove_key)?;
    }

    write_edit(&composer_json_path, &root, &original, manipulator)?;

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
        args.link,
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

/// Writes the edited `composer.json`: the `--no-normalize` splice as it
/// stands (#400), or `root` serialised and normalized with the indent
/// `original` had (an intermediate rewrite may already have changed it).
fn write_edit(
    path: &Path,
    root: &Value,
    original: &str,
    manipulator: Option<manipulator::Manipulator>,
) -> Result<()> {
    if let Some(manipulator) = manipulator {
        fs_err::write(path, manipulator.get_contents())?;
        return Ok(());
    }
    let indent = normalize::detect_indent(original);
    write_composer_json(path, root)?;
    if normalize::maybe_normalize(path, &indent)? {
        warn_out(&format!("Normalized {}", path.display()));
    }
    Ok(())
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
    let mut root: Value = serde_json::from_str(&original).context("parsing composer.json")?;
    if !root.is_object() {
        bail!("composer.json must be a JSON object");
    }

    let link_type = if args.dev { "require-dev" } else { "require" };
    let mut manipulator = args
        .no_normalize
        .then(|| manipulator::Manipulator::new(&original))
        .transpose()?;
    let mut allow_list = Vec::with_capacity(args.packages.len());
    for name in &args.packages {
        let name = name.to_ascii_lowercase();
        if remove_sub_node(&mut root, link_type, &name) {
            if let Some(manipulator) = &mut manipulator {
                manipulator.remove_sub_node(link_type, &name)?;
            }
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
        // Nothing was actually removed: leave composer.json untouched
        // rather than rewrite it into its own normalised form, so a no-op
        // stays a no-op in git too (#241).
        return Ok(());
    }
    // `JsonConfigSource::removeLink` always follows `removeSubNode` with
    // this, dropping `require`/`require-dev` entirely once its last
    // package is gone.
    remove_main_key_if_empty(&mut root, link_type);
    if let Some(manipulator) = &mut manipulator {
        manipulator.remove_main_key_if_empty(link_type)?;
    }

    write_edit(&composer_json_path, &root, &original, manipulator)?;

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
        args.link,
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
    link: LinkArgs,
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
    // #385: the solve reads the pinned php `viv run` uses, not `PATH`'s; #392: online, a
    // missing build is installed first.
    let php_override =
        crate::php::platform_php_installing(None, project_dir, Some(&cache_dir), offline)?;
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
        runtime
            .block_on(solver::solve_partial_update_seeded(
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
                php_override.as_deref(),
                &ignore,
            ))
            .map_err(|err| crate::php::annotate_solver_error(err, php_override.as_deref(), &root))?
    } else {
        runtime
            .block_on(solver::solve_update_seeded(
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
                php_override.as_deref(),
                &ignore,
            ))
            .map_err(|err| crate::php::annotate_solver_error(err, php_override.as_deref(), &root))?
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
            link,
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

/// `JsonConfigSource::addLink`/`RequireCommand::updateFileCleanly`, cut down
/// to the default path, where `maybe_normalize` runs afterwards (#145): no
/// format-preserving edit, just parse into a [`Value`], edit the map, and
/// let `normalize` reindent and resort (including the require/require-dev
/// section itself, so there is no `sort-packages` handling to port either).
/// `--no-normalize` uses `manipulator` instead (#400).
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
/// plain pretty-printer (not a format-preserving one) is enough. Not used for
/// the `--no-normalize` path, which writes `manipulator`'s text.
/// `pub(crate)`: `update.rs`'s own `bump-after-update` rewrite (#205) reuses
/// this same write-then-normalize path rather than a second manifest
/// writer.
pub(crate) fn write_composer_json(path: &Path, root: &Value) -> Result<()> {
    let mut contents = serde_json::to_string_pretty(root)?;
    contents.push('\n');
    fs_err::write(path, contents)?;
    Ok(())
}

/// A `Json/JsonManipulator.php` port for the edits `add`/`rm` make with
/// `--no-normalize` (#400), splicing into the original text so every other
/// byte stays. It is a hand-written scanner, not a regex port: Composer's
/// regexes match one balanced JSON value with PCRE's recursive
/// `(?(DEFINE)...)` groups, which the `regex` crate cannot express. The
/// scanner finds the same byte spans, and the text it splices in follows
/// `JsonManipulator`'s own output rules.
mod manipulator {
    use anyhow::{Result, bail};
    use serde_json::{Map, Value};

    use crate::normalize;

    pub(super) struct Manipulator {
        /// The file's own whitespace before the first and after the last
        /// byte of the JSON object, put back by `get_contents`.
        leading: String,
        trailing: String,
        contents: String,
        newline: &'static str,
        indent: String,
    }

    impl Manipulator {
        pub(super) fn new(text: &str) -> Result<Manipulator> {
            let contents = text.trim();
            if !(contents.starts_with('{') && contents.ends_with('}')) {
                bail!("composer.json must be a JSON object");
            }
            let leading = text[..text.len() - text.trim_start().len()].to_string();
            let trailing = text[text.trim_end().len()..].to_string();
            Ok(Manipulator {
                leading,
                trailing,
                newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
                indent: normalize::detect_indent(contents),
                contents: contents.to_string(),
            })
        }

        pub(super) fn get_contents(&self) -> String {
            format!("{}{}{}", self.leading, self.contents, self.trailing)
        }

        /// `JsonManipulator::addLink`.
        pub(super) fn add_link(
            &mut self,
            link_type: &str,
            package: &str,
            constraint: &str,
            sort_packages: bool,
        ) -> Result<()> {
            let Some(span) = span_of_key(&self.contents, link_type) else {
                self.add_main_key(link_type, package, constraint);
                return Ok(());
            };

            let mut links: Map<String, Value> = serde_json::from_str(&self.contents[span.clone()])?;
            let existing_key = links
                .keys()
                .find(|k| k.eq_ignore_ascii_case(package))
                .cloned();
            let replacement = if sort_packages {
                if let Some(existing_key) = &existing_key {
                    links.remove(existing_key);
                }
                links.insert(package.to_string(), Value::String(constraint.to_string()));
                normalize::sort_package_links(&mut links);
                let sorted: Vec<(String, Value)> = links.into_iter().collect();
                self.format_object(&sorted, 0)
            } else if let Some(existing_key) = existing_key {
                if let Some(key_span) =
                    span_of_string_value(&self.contents[span.clone()], &existing_key)
                {
                    let mut updated = self.contents[span.clone()].to_string();
                    updated.replace_range(key_span, &json_quote(constraint));
                    updated
                } else {
                    unreachable!("existing_key came from parsing the same span")
                }
            } else {
                insert_before_close(
                    &self.contents[span.clone()],
                    &self.indent,
                    self.newline,
                    package,
                    constraint,
                )
            };

            self.contents.replace_range(span, &replacement);
            Ok(())
        }

        /// `JsonManipulator::removeSubNode`, `mainNode` always `require`/
        /// `require-dev` here (no dotted `config.`/`extra.` sub-name split:
        /// that only applies to those two main nodes in the original).
        pub(super) fn remove_sub_node(&mut self, main_node: &str, name: &str) -> Result<()> {
            let Some(span) = span_of_key(&self.contents, main_node) else {
                return Ok(());
            };
            let links: Map<String, Value> = serde_json::from_str(&self.contents[span.clone()])?;
            let Some(existing_key) = links.keys().find(|k| k.eq_ignore_ascii_case(name)).cloned()
            else {
                return Ok(());
            };
            // `serde_json`'s `preserve_order` feature keeps `links` (and so
            // `remaining`) in the JSON's own key order: `removeSubNode`
            // never resorts, only `addLink`'s `sortPackages` path does.
            let remaining: Vec<(String, Value)> = links
                .into_iter()
                .filter(|(k, _)| k != &existing_key)
                .collect();
            let replacement = if remaining.is_empty() {
                format!("{{{}{}}}", self.newline, self.indent)
            } else {
                self.format_object(&remaining, 0)
            };
            self.contents.replace_range(span, &replacement);
            Ok(())
        }

        /// `JsonManipulator::removeMainKeyIfEmpty`.
        pub(super) fn remove_main_key_if_empty(&mut self, key: &str) -> Result<()> {
            self.contents = remove_main_key_if_empty(&self.contents, key)?;
            Ok(())
        }

        /// `JsonManipulator::addMainKey`'s "no existing key" tail: append
        /// just before the closing `}`, comma-separated from whatever the
        /// last top-level key was (or bare, for a brand new `{}`).
        fn add_main_key(&mut self, key: &str, package: &str, constraint: &str) {
            let entry = vec![(package.to_string(), Value::String(constraint.to_string()))];
            let value = self.format_object(&entry, 0);
            let line = format!("\"{key}\": {value}");
            let close = self
                .contents
                .rfind('}')
                .expect("constructor already validated the trailing brace");
            let is_empty = self.contents[..close]
                .trim_start_matches('{')
                .trim()
                .is_empty();
            let mut replacement = String::new();
            if is_empty {
                replacement.push('{');
                replacement.push_str(self.newline);
                replacement.push_str(&self.indent);
                replacement.push_str(&line);
                replacement.push_str(self.newline);
            } else {
                let before_close = self.contents[..close].trim_end_matches([' ', '\t', '\r', '\n']);
                let trailer = &self.contents[before_close.len()..close];
                replacement.push_str(before_close);
                replacement.push(',');
                replacement.push_str(self.newline);
                replacement.push_str(&self.indent);
                replacement.push_str(&line);
                replacement.push_str(trailer);
            }
            replacement.push('}');
            self.contents.replace_range(.., &replacement);
        }

        /// `JsonManipulator::format`, `depth` levels of `self.indent`
        /// already applied to the surrounding context (`0` when replacing a
        /// whole top-level value).
        fn format_object(&self, entries: &[(String, Value)], depth: usize) -> String {
            if entries.is_empty() {
                return format!("{{{}{}}}", self.newline, self.indent.repeat(depth + 1));
            }
            let mut out = format!("{{{}", self.newline);
            let lines: Vec<String> = entries
                .iter()
                .map(|(k, v)| {
                    format!(
                        "{}{}: {}",
                        self.indent.repeat(depth + 2),
                        json_quote(k),
                        json_scalar(v)
                    )
                })
                .collect();
            out.push_str(&lines.join(&format!(",{}", self.newline)));
            out.push_str(self.newline);
            out.push_str(&self.indent.repeat(depth + 1));
            out.push('}');
            out
        }
    }

    fn json_quote(s: &str) -> String {
        serde_json::to_string(s).expect("string always encodes")
    }

    fn json_scalar(v: &Value) -> String {
        match v {
            Value::String(s) => json_quote(s),
            other => other.to_string(),
        }
    }

    /// The byte range (relative to `contents`) of a balanced JSON value
    /// (object, array, string, or scalar run) starting at byte offset
    /// `start`, which must already be the value's first non-whitespace
    /// byte. A hand-written scanner standing in for the recursive-regex
    /// `(?&json)` production Composer's PCRE grammar uses (see the module
    /// doc): same result, since it's walking the same grammar, just with an
    /// explicit stack instead of regex recursion.
    fn value_end(contents: &str, start: usize) -> Option<usize> {
        let bytes = contents.as_bytes();
        match bytes.get(start)? {
            b'"' => string_end(contents, start),
            b'{' | b'[' => {
                let close = if bytes[start] == b'{' { b'}' } else { b']' };
                let mut depth = 1usize;
                let mut i = start + 1;
                while i < bytes.len() {
                    match bytes[i] {
                        b'"' => i = string_end(contents, i)?,
                        b'{' | b'[' => {
                            depth += 1;
                            i += 1;
                        }
                        c if c == close => {
                            depth -= 1;
                            i += 1;
                            if depth == 0 {
                                return Some(i);
                            }
                        }
                        b'}' | b']' => {
                            depth -= 1;
                            i += 1;
                            if depth == 0 {
                                return Some(i);
                            }
                        }
                        _ => i += 1,
                    }
                }
                None
            }
            _ => {
                // A bare scalar (number/bool/null): ends at the next comma,
                // closing bracket, or whitespace.
                let mut i = start;
                while i < bytes.len() && !matches!(bytes[i], b',' | b'}' | b']') {
                    i += 1;
                }
                Some(i)
            }
        }
    }

    fn string_end(contents: &str, start: usize) -> Option<usize> {
        let bytes = contents.as_bytes();
        debug_assert_eq!(bytes[start], b'"');
        let mut i = start + 1;
        while i < bytes.len() {
            match bytes[i] {
                b'\\' => i += 2,
                b'"' => return Some(i + 1),
                _ => i += 1,
            }
        }
        None
    }

    /// Every top-level `"key": value` pair in `contents` (an object
    /// literal), as `(key, key_start, value_end)` byte offsets in
    /// insertion order: `key_start` is the key's opening quote, `value_end`
    /// the byte just past the value. The shared scan
    /// [`span_of_key`]/[`remove_main_key_if_empty`] both search.
    fn top_level_entries(contents: &str) -> Option<Vec<(String, usize, usize)>> {
        let bytes = contents.as_bytes();
        let mut i = contents.find('{')? + 1;
        let mut depth = 1i32;
        let mut entries = Vec::new();
        while i < bytes.len() && depth > 0 {
            match bytes[i] {
                b'"' if depth == 1 => {
                    let key_start = i;
                    let key_end = string_end(contents, i)?;
                    let mut j = key_end;
                    while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                        j += 1;
                    }
                    if bytes.get(j) != Some(&b':') {
                        i = key_end;
                        continue;
                    }
                    j += 1;
                    while j < bytes.len() && (bytes[j] as char).is_whitespace() {
                        j += 1;
                    }
                    let value_end_pos = value_end(contents, j)?;
                    let found_key: String =
                        serde_json::from_str(&contents[key_start..key_end]).ok()?;
                    entries.push((found_key, key_start, value_end_pos));
                    i = value_end_pos;
                }
                b'"' => i = string_end(contents, i)?,
                b'{' | b'[' => {
                    depth += 1;
                    i += 1;
                }
                b'}' | b']' => {
                    depth -= 1;
                    i += 1;
                }
                _ => i += 1,
            }
        }
        Some(entries)
    }

    /// The byte range of the *value* belonging to a top-level `"key"` in
    /// `contents` (an object literal, `contents` itself included in the
    /// range's braces). `None` if `key` is absent from the top level.
    fn span_of_key(contents: &str, key: &str) -> Option<std::ops::Range<usize>> {
        let entries = top_level_entries(contents)?;
        let (_, key_start, value_end) = entries.into_iter().find(|(k, ..)| k == key)?;
        let key_end = string_end(contents, key_start)?;
        let mut j = key_end;
        let bytes = contents.as_bytes();
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        j += 1; // the ':'
        while j < bytes.len() && (bytes[j] as char).is_whitespace() {
            j += 1;
        }
        Some(j..value_end)
    }

    /// The byte range of the *string value* belonging to `key` inside
    /// `object_text` (a whole `{...}` object literal), for an in-place
    /// value replacement that otherwise leaves `object_text` untouched.
    fn span_of_string_value(object_text: &str, key: &str) -> Option<std::ops::Range<usize>> {
        span_of_key(object_text, key)
    }

    /// `JsonManipulator::removeMainKey`: delete a whole top-level `"key":
    /// value` pair, including the comma that used to separate it from its
    /// neighbour (the *preceding* comma if this was the last entry, the
    /// *following* one otherwise, so no dangling comma is left either way).
    fn remove_main_key(contents: &str, key: &str) -> Option<String> {
        let entries = top_level_entries(contents)?;
        let index = entries.iter().position(|(k, ..)| k == key)?;
        let (_, key_start, value_end) = entries[index];
        let bytes = contents.as_bytes();

        // `\s*,?\s*` after the removed value: skip whitespace, an optional
        // comma, then whitespace again, landing `end` exactly on the next
        // token (or the closing `}` with no comma at all).
        let mut end_start = value_end;
        while end_start < bytes.len() && (bytes[end_start] as char).is_whitespace() {
            end_start += 1;
        }
        if bytes.get(end_start) == Some(&b',') {
            end_start += 1;
            while end_start < bytes.len() && (bytes[end_start] as char).is_whitespace() {
                end_start += 1;
            }
        }
        let mut start = contents[..key_start].to_string();
        let end = &contents[end_start..];

        // `removeMainKey`'s own dangling-comma cleanup: only when the
        // removed key was the *last* one (`end` is just the closing `}`)
        // and `start` itself ends in a comma from the entry before it
        // (`rtrim($start, $this->indent)` afterwards drops the indent
        // spaces that used to lead into the removed key, approximated here
        // as "trailing space/tab characters", matching `rtrim`'s
        // charlist-not-substring semantics for the common space/tab
        // indents this stage's fixtures use).
        let start_trimmed_end = start.trim_end();
        if end == "}" && start_trimmed_end.ends_with(',') {
            let comma_at = start_trimmed_end.len() - 1;
            start.replace_range(comma_at..=comma_at, "");
            start = start.trim_end_matches([' ', '\t']).to_string();
        }
        Some(format!("{start}{end}"))
    }

    /// `JsonManipulator::removeMainKeyIfEmpty`: only when `key`'s value
    /// parsed to an empty object/array.
    fn remove_main_key_if_empty(contents: &str, key: &str) -> Result<String> {
        let Some(span) = span_of_key(contents, key) else {
            return Ok(contents.to_string());
        };
        let value: Value = serde_json::from_str(&contents[span])?;
        let is_empty = match &value {
            Value::Object(o) => o.is_empty(),
            Value::Array(a) => a.is_empty(),
            _ => false,
        };
        if !is_empty {
            return Ok(contents.to_string());
        }
        Ok(remove_main_key(contents, key).unwrap_or_else(|| contents.to_string()))
    }

    fn insert_before_close(
        object_text: &str,
        indent: &str,
        newline: &str,
        package: &str,
        constraint: &str,
    ) -> String {
        let close = object_text
            .rfind('}')
            .expect("object_text is a JSON object");
        let is_empty = object_text[1..close].trim().is_empty();
        if is_empty {
            return format!(
                "{{{newline}{indent}{indent}{}: {}{newline}{indent}}}",
                json_quote(package),
                json_quote(constraint)
            );
        }
        let before_close = object_text[..close].trim_end_matches([' ', '\t']);
        let before_close = before_close.trim_end_matches(['\r', '\n']);
        let trailer = &object_text[before_close.len()..];
        format!(
            "{before_close},{newline}{indent}{indent}{}: {}{trailer}",
            json_quote(package),
            json_quote(constraint)
        )
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::manipulator::Manipulator;
    use super::version_selector;
    use super::{add_link, remove_main_key_if_empty, remove_sub_node, run_require};

    /// `tests/fixtures/require-psr-container/composer.json.{before,after}`:
    /// recorded from a real Composer 2.10.2 `require` (see
    /// `tests/require.rs`'s module doc). `psr/container` already sits in
    /// `require-dev` there, so this covers an insert into a non-empty
    /// `require`, `remove_sub_node` emptying `require-dev`, and
    /// `remove_main_key_if_empty` dropping it with its dangling comma.
    #[test]
    fn manipulator_matches_composers_recorded_require() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/require-psr-container"
        );
        let before = fs_err::read_to_string(format!("{dir}/composer.json.before")).unwrap();
        let want = fs_err::read_to_string(format!("{dir}/composer.json.after")).unwrap();

        let mut manipulator = Manipulator::new(&before).unwrap();
        manipulator
            .add_link("require", "psr/container", "^2.0", false)
            .unwrap();
        manipulator
            .remove_sub_node("require-dev", "psr/container")
            .unwrap();
        manipulator.remove_main_key_if_empty("require-dev").unwrap();

        assert_eq!(manipulator.get_contents(), want);
    }

    /// `tests/fixtures/remove-psr-container/composer.json.{before,after}`:
    /// `viv rm psr/container --dev`'s edit, recorded the same way.
    #[test]
    fn manipulator_matches_composers_recorded_remove() {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/remove-psr-container"
        );
        let before = fs_err::read_to_string(format!("{dir}/composer.json.before")).unwrap();
        let want = fs_err::read_to_string(format!("{dir}/composer.json.after")).unwrap();

        let mut manipulator = Manipulator::new(&before).unwrap();
        manipulator
            .remove_sub_node("require-dev", "psr/container")
            .unwrap();
        manipulator.remove_main_key_if_empty("require-dev").unwrap();

        assert_eq!(manipulator.get_contents(), want);
    }

    #[test]
    fn manipulator_sorts_platform_packages_first() {
        let original = "{\n    \"require\": {\n        \"monolog/monolog\": \"^3.0\",\n        \"php\": \">=8.1\"\n    }\n}\n";
        let mut manipulator = Manipulator::new(original).unwrap();
        manipulator
            .add_link("require", "psr/log", "^3.0", true)
            .unwrap();
        assert_eq!(
            manipulator.get_contents(),
            "{\n    \"require\": {\n        \"php\": \">=8.1\",\n        \"monolog/monolog\": \"^3.0\",\n        \"psr/log\": \"^3.0\"\n    }\n}\n"
        );
    }

    /// An edit keeps the whitespace around the object as the file has it,
    /// whatever the newline style or number of final newlines.
    #[test]
    fn manipulator_keeps_the_whitespace_around_the_object() {
        for (before, after) in [
            (
                "{\n  \"require\": {\n    \"a/b\": \"^1\"\n  }\n}",
                "{\n  \"require\": {\n    \"a/b\": \"^1\",\n    \"c/d\": \"^2\"\n  }\n}",
            ),
            (
                "\n{\r\n  \"require\": {\r\n    \"a/b\": \"^1\"\r\n  }\r\n}\r\n\r\n",
                "\n{\r\n  \"require\": {\r\n    \"a/b\": \"^1\",\r\n    \"c/d\": \"^2\"\r\n  }\r\n}\r\n\r\n",
            ),
        ] {
            let mut manipulator = Manipulator::new(before).unwrap();
            manipulator.add_link("require", "c/d", "^2", false).unwrap();
            assert_eq!(manipulator.get_contents(), after);
        }
    }

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
            link: crate::link::LinkArgs::default(),
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
