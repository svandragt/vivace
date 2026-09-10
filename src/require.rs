//! `viv add`/`viv rm`: constraint synthesis (a `VersionSelector`
//! port), a `composer.json` edit (add/remove a `require`/`require-dev`
//! entry), an unconditional normalize of that edit (#145: `add`/`rm`
//! always normalize now, so `--no-normalize` is a deprecated no-op, same
//! shape as `install`/`dump-autoload`'s own, `docs/stability.md`), a
//! partial update of the touched package(s) (`docs/resolver-design.md`
//! stage 5, composer/composer#42), and a chained `install` (`--no-install`
//! opts out), matching `composer require`/`composer remove`'s own chain into
//! `Installer::run()` with `update` set.
//!
//! Before #145, the edit was a format-preserving `JsonManipulator` port so
//! that `--no-normalize` could skip reindenting/resorting `composer.json`.
//! Now that normalizing is unconditional, the intermediate formatting never
//! survives to disk, so the edit is a plain parse-map-serialize instead.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::{Map, Value};

use crate::install::{self, InstallArgs};
use crate::link::LinkMode;
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
    /// Deprecated, no-op (#145): `add` always normalizes `composer.json`
    /// now, same as `install`/`dump-autoload` since 0.6.
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
    /// Deprecated, no-op (#145): `rm` always normalizes `composer.json`
    /// now, same as `install`/`dump-autoload` since 0.6.
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

pub fn run_require(args: &RequireArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&composer_json_path).context("reading composer.json")?;
    let mut root: Value = serde_json::from_str(&original).context("parsing composer.json")?;
    if !root.is_object() {
        bail!("composer.json must be a JSON object");
    }

    let link_type = if args.dev { "require-dev" } else { "require" };
    let remove_key = if args.dev { "require" } else { "require-dev" };

    let mut requested = Vec::with_capacity(args.packages.len());
    for spec in &args.packages {
        let (name, constraint) = split_spec(spec);
        requested.push((name.to_ascii_lowercase(), constraint.map(str::to_string)));
    }

    let mut allow_list = Vec::with_capacity(requested.len());
    for (name, constraint) in &requested {
        let constraint = if let Some(c) = constraint {
            c.clone()
        } else {
            synthesize_constraint(name, &root, &project_dir, cache_dir, offline)?
        };
        add_link(&mut root, link_type, name, &constraint)?;
        // `RequireCommand::updateFileCleanly` always removes the same
        // package from the *other* require section too, moving it rather
        // than leaving a stale duplicate (no interactive confirmation
        // gate: that only decides which section a *warning* suggests,
        // never whether the move itself happens).
        remove_sub_node(&mut root, remove_key, name);
        allow_list.push(name.clone());
    }
    remove_main_key_if_empty(&mut root, remove_key);

    let indent = normalize::detect_indent(&original);
    write_composer_json(&composer_json_path, &root)?;
    if normalize::maybe_normalize(&composer_json_path, &indent)? {
        warn_out(&format!("Normalized {}", composer_json_path.display()));
    }
    if args.no_normalize {
        warn_no_normalize_is_a_noop("add");
    }

    if args.no_update {
        return Ok(());
    }

    partial_update(
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
        args.no_blocking || args.no_security_blocking,
    )
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
    let mut allow_list = Vec::with_capacity(args.packages.len());
    for name in &args.packages {
        let name = name.to_ascii_lowercase();
        remove_sub_node(&mut root, link_type, &name);
        allow_list.push(name);
    }
    // `JsonConfigSource::removeLink` always follows `removeSubNode` with
    // this, dropping `require`/`require-dev` entirely once its last
    // package is gone.
    remove_main_key_if_empty(&mut root, link_type);

    let indent = normalize::detect_indent(&original);
    write_composer_json(&composer_json_path, &root)?;
    if normalize::maybe_normalize(&composer_json_path, &indent)? {
        warn_out(&format!("Normalized {}", composer_json_path.display()));
    }
    if args.no_normalize {
        warn_no_normalize_is_a_noop("rm");
    }

    if args.no_update {
        return Ok(());
    }

    // `RemoveCommand`'s own default: the removed package's dependents may
    // update too, but a dependency also directly required by root stays put
    // (`Request::UPDATE_LISTED_WITH_TRANSITIVE_DEPS_NO_ROOT_REQUIRE`).
    partial_update(
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
        args.no_blocking || args.no_security_blocking,
    )
}

/// Shared tail of both commands: reload the (just-edited) `composer.json`,
/// dispatch `pre-update-cmd`, solve a partial update allow-listing `names`,
/// write the lock, chain into `install` (`--no-install` opts out), and
/// dispatch `post-update-cmd` — matching `composer require`/`composer
/// remove`'s own chain into `Installer::run()` with `update` set.
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
    no_blocking: bool,
) -> Result<()> {
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root: Value = serde_json::from_slice(&composer_json).context("parsing composer.json")?;
    let lock_path = project_dir.join("composer.lock");

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
    let repo = runtime.block_on(crate::update::build_repository(&root, &cache_dir, &fetcher))?;
    let (audit_config, no_blocking) = audit_config_and_no_blocking(&root, no_blocking)?;
    let advisories_transport = crate::audit::HttpTransport { fetcher: &fetcher };
    let advisories = Some(AdvisoryFilter {
        transport: &advisories_transport,
        audit: &audit_config,
        no_blocking,
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
            prefer_stable,
            prefer_lowest,
            &locked_by_name,
            names,
            mode,
            &[],
            HashMap::new(),
            advisories,
            Some(&cache_dir),
        ))?
    } else {
        runtime.block_on(solver::solve_update_seeded(
            &repo,
            &root,
            prefer_stable,
            prefer_lowest,
            &[],
            HashMap::new(),
            advisories,
            Some(&cache_dir),
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
            no_scripts,
            no_normalize: false,
            no_plugins,
            no_progress: false,
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

/// `--no-normalize`'s deprecation notice on `add`/`rm` (#145): now that
/// both always normalize, same shape as `install.rs`'s own
/// `warn_no_normalize_is_a_noop` for `install`/`dump-autoload` (#95).
fn warn_no_normalize_is_a_noop(command: &str) {
    warn_out(&normalize::no_normalize_is_a_noop_message(
        command,
        "0.8",
        &format!("{command} always normalizes composer.json now"),
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
    let repo = runtime.block_on(crate::update::build_repository(root, &cache_dir, &fetcher))?;
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
/// to what `add`/`rm` need now that `maybe_normalize` always runs
/// afterwards (#145): no format-preserving edit, just parse into a
/// [`Value`], edit the map, and let `normalize` reindent and resort
/// (including the require/require-dev section itself, so there is no
/// `sort-packages` handling to port either).
fn add_link(root: &mut Value, link_type: &str, package: &str, constraint: &str) -> Result<()> {
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

/// `JsonManipulator::removeSubNode`, `main_node` always `require`/
/// `require-dev` here. A no-op if `main_node` is absent or doesn't hold
/// `package` (case-insensitively).
fn remove_sub_node(root: &mut Value, main_node: &str, package: &str) {
    let Some(Value::Object(links)) = root.get_mut(main_node) else {
        return;
    };
    if let Some(existing_key) = links
        .keys()
        .find(|k| k.eq_ignore_ascii_case(package))
        .cloned()
    {
        links.remove(&existing_key);
    }
}

/// `JsonManipulator::removeMainKeyIfEmpty`: drop `key` from the root object
/// if its value parsed to an empty object or array.
fn remove_main_key_if_empty(root: &mut Value, key: &str) {
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
fn write_composer_json(path: &Path, root: &Value) -> Result<()> {
    let mut contents = serde_json::to_string_pretty(root)?;
    contents.push('\n');
    fs_err::write(path, contents)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, json};

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
            no_blocking: false,
            no_security_blocking: false,
        };
        let err = run_require(&args, None, true).unwrap_err();
        assert!(err.to_string().contains("composer.json"), "{err:#}");
    }

    /// `VersionSelector::transformVersion`'s worked examples from its own
    /// doc comment.
    #[test]
    fn recommended_constraint_transforms_versions() {
        let pv = |version: &str, normalized: &str| crate::repository::PackageVersion {
            name: "acme/pkg".to_string(),
            version: version.to_string(),
            version_normalized: normalized.to_string(),
            require: Map::default(),
            require_dev: Map::default(),
            replace: Map::default(),
            provide: Map::default(),
            conflict: Map::default(),
            default_branch: false,
            dist: None,
            source: None,
            branch_alias: None,
            time: None,
            raw: serde_json::json!({}),
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
