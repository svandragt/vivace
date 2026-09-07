//! `viv require`/`viv remove`: constraint synthesis (a `VersionSelector`
//! port) and a format-preserving `composer.json` edit (a `JsonManipulator`
//! port), then normalizing that edit (`--no-normalize` opts out, #95), a
//! partial update of the touched package(s) (`docs/resolver-design.md`
//! stage 5, composer/composer#42), and a chained `install` (`--no-install`
//! opts out), matching `composer require`/`composer remove`'s own chain into
//! `Installer::run()` with `update` set.
//!
//! The `JsonManipulator` port here is not a byte-for-byte port of
//! `Json/JsonManipulator.php`'s own regexes: those lean on PCRE's recursive
//! `(?(DEFINE)...)` named-group grammar to match one balanced JSON value
//! inline, which the `regex` crate (no backtracking, no recursion) simply
//! cannot express. `manipulator::value_end`/`span_of_key` get the same *result*
//! (the exact byte span of a top-level key's value, or a key inside it) with
//! a small hand-written scanner instead, and the insert/replace text it
//! splices in matches `JsonManipulator`'s own output rules exactly.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde_json::Value;

use crate::auth::Auth;
use crate::fetch::Fetcher;
use crate::install::{self, InstallArgs};
use crate::link::LinkMode;
use crate::normalize;
use crate::repository::{HttpTransport, Repository};
use crate::scripts;
use crate::solver::{self, pool_builder::UpdateAllowMode};

const PACKAGIST_URL: &str = "https://repo.packagist.org";

/// `viv require` flags.
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
    /// Don't normalize `composer.json` (key order, whitespace) after
    /// writing it.
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
    /// (`composer require --no-install`): today's `viv require` behaviour.
    #[arg(long)]
    pub no_install: bool,
}

/// `viv remove` flags.
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
    /// Don't normalize `composer.json` (key order, whitespace) after
    /// writing it.
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
    /// (`composer remove --no-install`): today's `viv remove` behaviour.
    #[arg(long)]
    pub no_install: bool,
}

pub fn run_require(args: &RequireArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&composer_json_path).context("reading composer.json")?;
    let root: Value = serde_json::from_str(&original).context("parsing composer.json")?;

    let link_type = if args.dev { "require-dev" } else { "require" };
    let remove_key = if args.dev { "require" } else { "require-dev" };
    let sort_packages = args.sort_packages
        || root
            .pointer("/config/sort-packages")
            .and_then(Value::as_bool)
            .unwrap_or(false);

    let mut requested = Vec::with_capacity(args.packages.len());
    for spec in &args.packages {
        let (name, constraint) = split_spec(spec);
        requested.push((name.to_ascii_lowercase(), constraint.map(str::to_string)));
    }

    let mut manipulator = manipulator::Manipulator::new(&original)?;
    let mut allow_list = Vec::with_capacity(requested.len());
    for (name, constraint) in &requested {
        let constraint = if let Some(c) = constraint {
            c.clone()
        } else {
            synthesize_constraint(name, &root, &project_dir, cache_dir)?
        };
        manipulator.add_link(link_type, name, &constraint, sort_packages)?;
        // `RequireCommand::updateFileCleanly` always removes the same
        // package from the *other* require section too, moving it rather
        // than leaving a stale duplicate (no interactive confirmation
        // gate: that only decides which section a *warning* suggests,
        // never whether the move itself happens).
        manipulator.remove_sub_node(remove_key, name)?;
        allow_list.push(name.clone());
    }
    manipulator.remove_main_key_if_empty(remove_key)?;

    fs_err::write(&composer_json_path, manipulator.get_contents())?;
    if !args.no_normalize && normalize::maybe_normalize(&composer_json_path)? {
        warn_out(&format!("Normalized {}", composer_json_path.display()));
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
    )
}

pub fn run_remove(args: &RemoveArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&composer_json_path).context("reading composer.json")?;

    let link_type = if args.dev { "require-dev" } else { "require" };
    let mut manipulator = manipulator::Manipulator::new(&original)?;
    let mut allow_list = Vec::with_capacity(args.packages.len());
    for name in &args.packages {
        let name = name.to_ascii_lowercase();
        manipulator.remove_sub_node(link_type, &name)?;
        allow_list.push(name);
    }
    // `JsonConfigSource::removeLink` always follows `removeSubNode` with
    // this, dropping `require`/`require-dev` entirely once its last
    // package is gone.
    manipulator.remove_main_key_if_empty(link_type)?;

    fs_err::write(&composer_json_path, manipulator.get_contents())?;
    if !args.no_normalize && normalize::maybe_normalize(&composer_json_path)? {
        warn_out(&format!("Normalized {}", composer_json_path.display()));
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
fn partial_update(
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

    let secure_http = root
        .pointer("/config/secure-http")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let prefer_stable = prefer_stable
        || root
            .get("prefer-stable")
            .and_then(Value::as_bool)
            .unwrap_or(false);

    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    let auth = Auth::load(project_dir)?;
    let fetcher = Fetcher::new(auth)?.secure_http(secure_http);
    let transport = HttpTransport { fetcher: &fetcher };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let repo = runtime.block_on(Repository::load(PACKAGIST_URL, &cache_dir, transport))?;

    // A brand new `composer.json` (no lock yet) cannot do a partial update
    // (`PoolBuilder::buildPool` requires a locked repository); fall back to
    // a full update, matching `RequireCommand`'s own "no lock present" skip
    // of `setUpdateAllowList`.
    let result = if lock_path.exists() {
        let lock_bytes = fs_err::read(&lock_path)?;
        let lock: Value = serde_json::from_slice(&lock_bytes).context("parsing composer.lock")?;
        let locked_by_name = locked_packages_by_name(&lock);
        runtime.block_on(solver::solve_partial_update(
            &repo,
            &root,
            prefer_stable,
            prefer_lowest,
            &locked_by_name,
            names,
            mode,
        ))?
    } else {
        runtime.block_on(solver::solve_update(
            &repo,
            &root,
            prefer_stable,
            prefer_lowest,
        ))?
    };

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
fn synthesize_constraint(
    name: &str,
    root: &Value,
    project_dir: &Path,
    cache_dir: Option<&Path>,
) -> Result<String> {
    let secure_http = root
        .pointer("/config/secure-http")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    let auth = Auth::load(project_dir)?;
    let fetcher = Fetcher::new(auth)?.secure_http(secure_http);
    let transport = HttpTransport { fetcher: &fetcher };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let repo = runtime.block_on(Repository::load(PACKAGIST_URL, &cache_dir, transport))?;
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
fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((name, constraint)) => (name, Some(constraint)),
        None => (spec, None),
    }
}

/// `Package\Version\VersionSelector`, cut down to what constraint synthesis
/// needs: no platform-requirement filtering (`--ignore-platform-req(s)`
/// isn't wired for `require` at this stage) and no branch-alias dev-version
/// handling (`findRecommendedRequireVersion`'s `isDev()` branch) — a bare
/// `viv require vendor/pkg` on a stable release is the common case this
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

/// A `Json/JsonManipulator.php` port for the two edits `require`/`remove`
/// need: see the module doc for why this is a hand-written scanner rather
/// than a literal regex port.
mod manipulator {
    use anyhow::{Result, bail};
    use serde_json::{Map, Value};

    pub(super) struct Manipulator {
        contents: String,
        newline: &'static str,
        indent: String,
    }

    impl Manipulator {
        pub(super) fn new(text: &str) -> Result<Manipulator> {
            let trimmed = text.trim();
            let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
            let contents = if trimmed.is_empty() {
                format!("{{{newline}}}")
            } else {
                trimmed.to_string()
            };
            if !(contents.starts_with('{') && contents.ends_with('}')) {
                bail!("composer.json must be a JSON object");
            }
            let indent = detect_indent(&contents);
            Ok(Manipulator {
                contents,
                newline,
                indent,
            })
        }

        pub(super) fn get_contents(&self) -> String {
            format!("{}{}", self.contents, self.newline)
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
                let mut sorted: Vec<(String, Value)> = links.into_iter().collect();
                sort_packages_by_name(&mut sorted);
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

    /// `JsonManipulator::sortPackages`: platform packages first
    /// (`php`/`hhvm`/`ext-*`/`lib-*` ahead of everything else, each group
    /// alphabetical), then every other package name alphabetically.
    fn sort_packages_by_name(entries: &mut [(String, Value)]) {
        entries.sort_by_key(|(name, _)| sort_key(name));
    }

    fn sort_key(name: &str) -> String {
        if crate::repository::is_platform_package(name) {
            let prefix = if name.eq_ignore_ascii_case("php") || name.starts_with("php-") {
                "0"
            } else if name.eq_ignore_ascii_case("hhvm") {
                "1"
            } else if name.starts_with("ext-") {
                "2"
            } else if name.starts_with("lib-") {
                "3"
            } else {
                "4"
            };
            format!("{prefix}-{name}")
        } else {
            format!("5-{name}")
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

    /// `JsonFile::detectIndenting`: the first line starting with whitespace
    /// then a `"`, that leading whitespace; `"    "` (four spaces) if no
    /// line matches.
    fn detect_indent(contents: &str) -> String {
        for line in contents.lines() {
            let ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
            if ws_len > 0 && line[ws_len..].starts_with('"') {
                return line[..ws_len].to_string();
            }
        }
        "    ".to_string()
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
    use serde_json::Map;

    use super::manipulator::Manipulator;
    use super::version_selector;

    /// `tests/fixtures/require-psr-container/composer.json.{before,after}`:
    /// recorded from a real Composer 2.10.2 `require` (see
    /// `tests/require.rs`'s module doc). `psr/container` already sits in
    /// `require-dev` there, so this exercises `add_link`'s
    /// existing-links-non-empty insert branch, `remove_sub_node`'s
    /// last-entry-empties-the-object branch, and
    /// `remove_main_key_if_empty` dropping the now-empty `require-dev` key
    /// (with its own dangling-comma cleanup) all in one edit, matching
    /// `RequireCommand::updateFileCleanly`'s always-remove-from-the-other-key
    /// behaviour.
    #[test]
    fn add_link_matches_composers_recorded_require() {
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
    /// `viv remove psr/container --dev`'s edit, recorded the same way.
    #[test]
    fn remove_sub_node_matches_composers_recorded_remove() {
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
    fn add_link_creates_a_brand_new_require_section() {
        let original = "{\n    \"name\": \"acme/pkg\"\n}\n";
        let mut manipulator = Manipulator::new(original).unwrap();
        manipulator
            .add_link("require", "psr/log", "^3.0", false)
            .unwrap();
        assert_eq!(
            manipulator.get_contents(),
            "{\n    \"name\": \"acme/pkg\",\n    \"require\": {\n        \"psr/log\": \"^3.0\"\n    }\n}\n"
        );
    }

    #[test]
    fn add_link_updates_an_existing_constraint_in_place() {
        let original = "{\n    \"require\": {\n        \"psr/log\": \"^2.0\"\n    }\n}\n";
        let mut manipulator = Manipulator::new(original).unwrap();
        manipulator
            .add_link("require", "psr/log", "^3.0", false)
            .unwrap();
        assert_eq!(
            manipulator.get_contents(),
            "{\n    \"require\": {\n        \"psr/log\": \"^3.0\"\n    }\n}\n"
        );
    }

    #[test]
    fn add_link_sorts_platform_packages_first() {
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

    #[test]
    fn remove_sub_node_leaves_other_entries_untouched() {
        let original = "{\n    \"require\": {\n        \"monolog/monolog\": \"^3.0\",\n        \"psr/log\": \"^3.0\"\n    }\n}\n";
        let mut manipulator = Manipulator::new(original).unwrap();
        manipulator.remove_sub_node("require", "psr/log").unwrap();
        assert_eq!(
            manipulator.get_contents(),
            "{\n    \"require\": {\n        \"monolog/monolog\": \"^3.0\"\n    }\n}\n"
        );
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
