//! Port of `DependencyResolver/PoolBuilder.php`, `Installer.php`'s request
//! wiring (`createRequest`/`requirePackagesForUpdate`, ~line 900-1070) and
//! `Package/Loader/RootPackageLoader.php`'s alias/stability-flag extraction.
//! `Repository/PlatformRepository.php`'s fixed platform packages live in
//! `platform.rs`, split out once this section grew past that file's own
//! switch statement.
//!
//! Full update only (`docs/resolver-design.md` stage 3): no partial-update
//! allow-list or path-repo unlocking, security-advisory/filter-list pool
//! filters. `Repository::load_closure_seeded` already does the breadth-first,
//! batched, constraint-narrowed metadata load
//! `PoolBuilder::loadPackagesMarkedForLoading`/`markPackageNameForLoading`
//! perform in real Composer (#90), so this only needs to turn that closure
//! into `Package`s and filter/alias them. `pool_optimizer::optimize` runs
//! right after the raw pool is assembled, exactly where
//! `PoolBuilder::buildPool`'s own `runOptimizer` call sits (#76).
//!
//! The root `composer.json` package is a fixed pool member too
//! (`root_package`, `Installer::createRequest`'s `$request->fixPackage($rootPackage)`),
//! but only carrying its `replace`/`provide` links: its own `require`/
//! `require-dev` links become `request.requires` directly instead, the
//! exact same constraint a root `RULE_PACKAGE_REQUIRES` rule would add on
//! top (the root is always force-installed, so `-root` is never true,
//! collapsing that rule to the same "install one of" disjunction the
//! `RULE_ROOT_REQUIRE` rule already states) — so `Package::requires` stays
//! empty to avoid generating it twice. Root `conflict` is still not
//! modelled; add it if a fixture ever needs one. A name the root
//! `replace`s is never fetched at all (`root_replaced_names`, mirroring
//! `PoolBuilder::buildPool`'s `foreach ($package->getReplaces() as $link)
//! $this->loadedPackages[$link->getTarget()] = new MatchAllConstraint();`);
//! a `provide`d name has no such closure-skip (a real package by that name
//! can still be a candidate), it only satisfies other packages' requires
//! through the ordinary `Pool::whatProvides` walk.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{Map, Value};

use crate::audit::{self, AdvisoriesTransport, NoAdvisories};
use crate::lock::AuditConfig;
use crate::repository::{
    ClosureRoot, DevAcceptance, PackageVersion, Repository, Transport, branch_alias_target,
};
use crate::semver;
use crate::solver::platform::cached_platform_packages;
use crate::solver::policy::DefaultPolicy;
use crate::solver::pool::{Link, Package, Pool};
use crate::solver::pool_optimizer;
use crate::solver::request::Request;
use crate::solver::{ConstraintCache, parse_constraint_cached};

/// `BasePackage::STABILITIES` order, least to most stable... actually most
/// stable first, matching `stability_rank`'s ascending "more stable = lower
/// number" scale (`policy.rs`).
const STABILITIES: [&str; 5] = ["stable", "RC", "beta", "alpha", "dev"];

pub struct BuildResult {
    pub pool: Pool,
    pub request: Request,
    /// `RootPackage::getMinimumStability`.
    pub minimum_stability: &'static str,
    /// `RootPackage::getStabilityFlags`, `BasePackage::STABILITIES`-ranked
    /// (`stability_rank`'s scale: 0 stable .. 20 dev) for direct reuse as
    /// the lock's `stability-flags` values.
    pub stability_flags: HashMap<String, u8>,
    /// Root `require`/`require-dev` platform-package pretty constraints, for
    /// the lock's `platform`/`platform-dev` keys
    /// (`Installer::extractPlatformRequirements`).
    pub platform_reqs: Map<String, Value>,
    pub platform_dev_reqs: Map<String, Value>,
    /// `config.platform` verbatim, for the lock's `platform-overrides` key
    /// (only emitted there when non-empty).
    pub platform_overrides: Map<String, Value>,
}

/// `SecurityAdvisoryPoolFilter::filter`'s BC-audit-config inputs (#175):
/// `None` disables the filter outright — the pre-#175 `build`/`build_partial`
/// signatures pass that, so they stay network-free.
pub struct AdvisoryFilter<'a, A: AdvisoriesTransport> {
    pub transport: &'a A,
    pub audit: &'a AuditConfig,
    /// `--no-blocking`/`--no-security-blocking`/`COMPOSER_NO_SECURITY_BLOCKING=1`.
    pub no_blocking: bool,
}

/// `SecurityAdvisoryPoolFilter::filter`'s BC-audit-config path: drops a
/// candidate `Package` (everything in `packages[exempt_upto..]`, `Package`s
/// before that are root/platform/a locked package that isn't being updated,
/// this module's own doc comment at the `exempt_upto` assignment) whose
/// version a known security advisory covers (`audit.block-insecure`) or that
/// Packagist marks abandoned (`audit.block-abandoned`), honouring
/// `audit.ignore` and `--no-blocking`. A dev package (`dev-master`, a branch
/// alias, ...) is never advisory-checked
/// (`SecurityAdvisoryPoolFilter::getMatchingAdvisories`'s own
/// `$package->isDev()` skip) but is still abandoned-checked, matching
/// upstream's own asymmetry there.
///
/// The advisories POST failing outright (`--offline`, an unreachable
/// endpoint, an auth error) is not a hard failure: Composer's own
/// `ignore-unreachable` defaults to `true` for the `update` block scope
/// (`IgnoreUnreachable::default()`), so it warns and returns the pool
/// unfiltered for the advisory half rather than failing the command
/// (`SecurityAdvisoryPoolFilter::filter`'s own unreachable-repos warning).
/// Abandoned-blocking never depends on this POST (it reads a field already
/// on the package's own fetched metadata), so it still applies even then.
///
/// ponytail: a version this removes never resurfaces in the solver's own
/// "could not be found"/"no matching package" message the way Composer's
/// own advisory-aware `Problem` wording does (`solver::problem`'s module
/// doc still lists this as not ported) — no fixture here drives a solve to
/// actually fail because of it; widen `problem.rs` if one does.
async fn filter_advisories<A: AdvisoriesTransport>(
    packages: Vec<Package>,
    exempt_upto: usize,
    filter: &AdvisoryFilter<'_, A>,
) -> Result<Vec<Package>> {
    if filter.no_blocking || (!filter.audit.block_insecure && !filter.audit.block_abandoned) {
        return Ok(packages);
    }

    let mut names: Vec<String> = Vec::new();
    for package in &packages[exempt_upto..] {
        if !names.iter().any(|n| n == &package.name) {
            names.push(package.name.clone());
        }
    }

    let response = if filter.audit.block_insecure {
        match audit::fetch_advisories(filter.transport, &names).await {
            Ok(response) => Some(response),
            Err(err) => {
                warn_out(
                    "Security advisory data could not be fetched from some repositories \
                     (ignored per policy.ignore-unreachable); matches may be incomplete:",
                );
                warn_out(&format!("  - {err}"));
                None
            }
        }
    } else {
        None
    };

    let mut to_remove: HashSet<usize> = HashSet::new();
    for (index, package) in packages.iter().enumerate().skip(exempt_upto) {
        if filter.audit.block_abandoned && audit::is_abandoned(&package.raw) {
            to_remove.insert(index);
            continue;
        }
        if !package.is_dev
            && let Some(response) = &response
            && !audit::matching_advisory_ids(
                response,
                &filter.audit.ignore,
                &package.name,
                &package.version,
            )
            .is_empty()
        {
            to_remove.insert(index);
        }
    }
    // #175: an alias and the package it aliases are always kept or removed
    // together, mirroring `pool_optimizer::optimize`'s own alias guard (its
    // module doc explains why). A branch alias is always `is_dev` and so
    // never matches the advisory check itself, but leaving its now-filtered
    // real package's index dangling in `alias_of` is exactly what made
    // `pool_optimizer::optimize`'s later remap panic with "no entry found
    // for key" (`pool_optimizer.rs`'s own alias remap assumes every
    // `alias_of` still points at a package in the same pool).
    for (index, package) in packages.iter().enumerate().skip(exempt_upto) {
        if let Some(alias_of) = package.alias_of
            && (to_remove.contains(&index) || to_remove.contains(&alias_of))
        {
            to_remove.insert(index);
            to_remove.insert(alias_of);
        }
    }

    let mut kept = Vec::with_capacity(packages.len());
    let mut remap: HashMap<usize, usize> = HashMap::with_capacity(packages.len());
    for (index, package) in packages.into_iter().enumerate() {
        if to_remove.contains(&index) {
            continue;
        }
        remap.insert(index, kept.len());
        kept.push(package);
    }
    for package in &mut kept {
        if let Some(alias_of) = package.alias_of {
            package.alias_of = Some(remap[&alias_of]);
        }
    }
    Ok(kept)
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr`
/// lint (`audit.rs`'s own `warn_out` does the same).
fn warn_out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

/// `Installer::doUpdate`'s first solve: root `require` and `require-dev`
/// merged (`Installer.php:1061-1067`), against every package the
/// repository's closure discovers.
pub async fn build<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> Result<BuildResult> {
    build_seeded::<T, NoAdvisories>(
        repo,
        root,
        prefer_stable,
        prefer_lowest,
        &[],
        &HashMap::new(),
        None,
        None,
    )
    .await
}

/// Same as [`build`], but `seed` (already-lowercased package names, #90's
/// prior-lock prefetch) is passed straight through to
/// [`Repository::load_closure_seeded`]; a name `seed` names that `root`
/// doesn't actually require never enters `packages` below, since it only
/// ever lands in `closure` if the walk reaches it. `preferred` is
/// `--minimal-changes`'s pin set, passed straight through to
/// [`build_partial_seeded`] (see its own doc for why it must reach
/// `pool_optimizer::optimize`, not just the solver).
///
/// #111: a full update is exactly [`build_partial_seeded`] with nothing
/// locked out — `Repository::load_closure`'s own `load_closure_skipping(...,
/// &HashSet::new())` (`src/repository.rs:1015-1022`) already proves an empty
/// skip set changes nothing, and an empty `locked_by_name` makes
/// `build_partial_seeded`'s second `ClosureRoot`/locked-entry push both
/// no-ops.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API, only ever called with the default hasher"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors build plus one seed slice, the minimal-changes pin set, the advisory pool \
              filter, and the platform-probe cache dir"
)]
pub async fn build_seeded<T: Transport, A: AdvisoriesTransport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
    seed: &[String],
    preferred: &HashMap<String, semver::NormalizedVersion>,
    advisories: Option<AdvisoryFilter<'_, A>>,
    cache_dir: Option<&Path>,
) -> Result<BuildResult> {
    build_partial_seeded(
        repo,
        root,
        &HashMap::new(),
        &HashSet::new(),
        prefer_stable,
        prefer_lowest,
        seed,
        preferred,
        advisories,
        cache_dir,
    )
    .await
}

/// `Installer::requirePackagesForUpdate`'s non-`updateMirrors` branch: root
/// `require` then `require-dev`, in that order (`Installer.php:1061-1067`).
fn root_requires(
    require: &Map<String, Value>,
    require_dev: &Map<String, Value>,
) -> Result<Vec<crate::solver::request::RootRequire>> {
    let mut requires = Vec::with_capacity(require.len() + require_dev.len());
    for (name, value) in require.iter().chain(require_dev.iter()) {
        let raw = value
            .as_str()
            .with_context(|| format!("require {name}: constraint is not a string"))?;
        requires.push(crate::solver::request::RootRequire {
            name: name.to_ascii_lowercase(),
            constraint: Some(semver::parse_constraint(raw)?),
            pretty_constraint: raw.to_string(),
        });
    }
    Ok(requires)
}

/// A partial update's allow-list mode (`Request::UPDATE_*`), deciding how
/// far an explicitly listed package's transitive dependencies are allowed
/// to move off their locked version.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpdateAllowMode {
    /// `UPDATE_ONLY_LISTED`: only the named packages themselves.
    OnlyListed,
    /// `UPDATE_LISTED_WITH_TRANSITIVE_DEPS_NO_ROOT_REQUIRE`: the named
    /// packages plus their locked dependency closure, except a name also
    /// directly required by root (that one stays locked).
    WithTransitiveDepsNoRootRequire,
    /// `UPDATE_LISTED_WITH_TRANSITIVE_DEPS`: the named packages plus their
    /// full locked dependency closure, root-required or not.
    WithTransitiveDeps,
}

/// `PoolBuilder::isUpdateAllowed`'s expansion, done once up front against
/// the *locked* dependency graph rather than Composer's own live
/// `unlockPackage` cascade during pool building
/// (`PoolBuilder.php:680-760`).
///
/// ponytail: this does not re-run when an allow-listed package's freshly
/// fetched version turns out to need a dependency version the locked graph
/// doesn't have recorded (Composer's `unlockPackage` reacts to that
/// mid-build); it only walks the *existing* lock's requires. A fixture that
/// needs the live cascade should widen this rather than the acceptance
/// tests reaching for it silently.
pub(crate) fn expand_allow_list(
    initial: &[String],
    locked_requires: &HashMap<String, Vec<String>>,
    root_require_names: &HashSet<String>,
    mode: UpdateAllowMode,
) -> HashSet<String> {
    let mut allowed: HashSet<String> = initial.iter().cloned().collect();
    if mode == UpdateAllowMode::OnlyListed {
        return allowed;
    }

    let mut queue: std::collections::VecDeque<String> = initial.iter().cloned().collect();
    while let Some(name) = queue.pop_front() {
        let Some(requires) = locked_requires.get(&name) else {
            continue;
        };
        for target in requires {
            if mode == UpdateAllowMode::WithTransitiveDepsNoRootRequire
                && root_require_names.contains(target)
                && !initial.contains(target)
            {
                continue;
            }
            if allowed.insert(target.clone()) {
                queue.push_back(target.clone());
            }
        }
    }
    allowed
}

/// `Installer::doUpdate`'s partial-update pool: like [`build`], except a
/// name in the current lock but outside `allow_names` is loaded straight
/// from its lock entry (`package_from_lock_entry`) rather than fetched
/// fresh, matching `PoolBuilder::buildPool`'s `getFixedOrLockedPackages`
/// loop (a locked-out name only ever has its one recorded version in the
/// pool, so the solver has nothing else to pick). `allow_names` and every
/// key of `locked_by_name` are already lowercased.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API, only ever called with the default hasher"
)]
pub async fn build_partial<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    locked_by_name: &HashMap<String, Value>,
    allow_names: &HashSet<String>,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> Result<BuildResult> {
    build_partial_seeded::<T, NoAdvisories>(
        repo,
        root,
        locked_by_name,
        allow_names,
        prefer_stable,
        prefer_lowest,
        &[],
        &HashMap::new(),
        None,
        None,
    )
    .await
}

/// Same as [`build_partial`], but `seed` is passed straight through to
/// [`Repository::load_closure_seeded`] (#90); see [`build_seeded`] for why a
/// seed can never change the pool, only how quickly it's built. `preferred`
/// is `--minimal-changes`'s pin set (`Installer::createPolicy`'s
/// `$preferredVersions`, `policy.rs`'s module doc): the same `DefaultPolicy`
/// this builds for [`pool_optimizer::optimize`] below must carry it too,
/// matching `Installer.php:534` passing one preferred-versions-aware
/// `$policy` into both `Solver` and `createPoolOptimizer` — otherwise the
/// optimizer's own duplicate-version collapse
/// (`PoolOptimizer::optimize`'s `selectPreferredPackages` call,
/// `PoolOptimizer.php:247`) can discard the pinned version as a "duplicate"
/// before the solver ever sees it (#61).
#[expect(
    clippy::implicit_hasher,
    reason = "internal API, only ever called with the default hasher"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors build_partial plus one seed slice, the minimal-changes pin set, the \
              advisory pool filter, and the platform-probe cache dir"
)]
pub async fn build_partial_seeded<T: Transport, A: AdvisoriesTransport>(
    repo: &Repository<T>,
    root: &Value,
    locked_by_name: &HashMap<String, Value>,
    allow_names: &HashSet<String>,
    prefer_stable: bool,
    prefer_lowest: bool,
    seed: &[String],
    preferred: &HashMap<String, semver::NormalizedVersion>,
    advisories: Option<AdvisoryFilter<'_, A>>,
    cache_dir: Option<&Path>,
) -> Result<BuildResult> {
    let require = string_map(root, "require");
    let require_dev = string_map(root, "require-dev");

    let minimum_stability = root
        .get("minimum-stability")
        .and_then(Value::as_str)
        .map_or("stable", normalize_stability);

    let mut stability_flags: HashMap<String, &'static str> = HashMap::new();
    let mut root_aliases: HashMap<String, Vec<(String, String, String)>> = HashMap::new();
    for (name, value) in require.iter().chain(require_dev.iter()) {
        let raw = value
            .as_str()
            .with_context(|| format!("require {name}: constraint is not a string"))?;
        extract_alias(name, raw, &mut root_aliases)?;
        extract_stability_flag(name, raw, minimum_stability, &mut stability_flags);
    }

    let acceptable: HashSet<&'static str> = STABILITIES
        .iter()
        .copied()
        .filter(|s| stability_rank(s) <= stability_rank(minimum_stability))
        .collect();
    let dev_acceptance =
        if acceptable.contains("dev") || stability_flags.values().any(|&s| s == "dev") {
            DevAcceptance::Both
        } else {
            DevAcceptance::NonDevOnly
        };

    let mut skip: HashSet<String> = locked_by_name
        .keys()
        .filter(|name| !allow_names.contains(*name))
        .cloned()
        .collect();
    skip.extend(root_replaced_names(root));

    // `PoolBuilder::buildPool`'s `getFixedOrLockedPackages` loop calls
    // `loadPackage` on every locked-out package too (`propagateUpdate =
    // false`), and that still runs `markPackageNameForLoading` over its own
    // `require` links (`PoolBuilder.php:520-551`) unless the required name is
    // itself locked-out. So an allow-listed name reachable only through a
    // locked parent's require (`psr/log` behind `laravel/framework`, #79)
    // still needs discovering: seed the closure walk with every locked-out
    // package's own requires as a second root, alongside the real one. Names
    // already in `skip` are pre-seeded into `discovered`
    // (`Repository::load_closure_skipping`), so a locked-out require of
    // another locked-out package is silently deduplicated, matching the real
    // `isset($this->skippedLoad[$require])` no-op branch.
    let mut locked_out_requires: Map<String, Value> = Map::new();
    for name in &skip {
        if let Some(entry) = locked_by_name.get(name).and_then(Value::as_object) {
            locked_out_requires.extend(map_field(entry, "require"));
        }
    }

    let empty_require_dev = Map::new();
    let roots = [
        ClosureRoot {
            require: &require,
            require_dev: &require_dev,
        },
        ClosureRoot {
            require: &locked_out_requires,
            require_dev: &empty_require_dev,
        },
    ];
    let mut constraint_cache: ConstraintCache = ConstraintCache::new();
    let closure = repo
        .load_closure_seeded(
            &roots,
            dev_acceptance,
            &skip,
            seed,
            &|name, stability| is_acceptable(name, stability, &acceptable, &stability_flags),
            &mut constraint_cache,
        )
        .await?;

    let platform_overrides = root
        .pointer("/config/platform")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let platform_started = Instant::now();
    let mut packages = cached_platform_packages(&platform_overrides, cache_dir)?;
    tracing::debug!(
        elapsed_ms = platform_started.elapsed().as_millis(),
        "detected platform packages (shells out to `php`)"
    );
    packages.push(root_package(root, &mut constraint_cache)?);
    let fixed: Vec<usize> = (0..packages.len()).collect();

    for name in &skip {
        if let Some(entry) = locked_by_name.get(name) {
            packages.push(package_from_lock_entry(entry, &mut constraint_cache)?);
        }
    }
    // Everything pushed so far (platform, root, locked-out-and-not-updated)
    // is exempt from the advisory/abandoned filter below
    // (`SecurityAdvisoryPoolFilter::filter`'s `!$package instanceof
    // RootPackageInterface && !PlatformRepository::isPlatformPackage(...) &&
    // !$request->isLockedPackage($package)`); everything pushed after this
    // point is a fresh closure-fetched candidate and is checked.
    let exempt_upto = packages.len();

    let push_started = Instant::now();
    for versions in closure.into_values() {
        for version in versions {
            push_package_version(
                &mut packages,
                version,
                &acceptable,
                &stability_flags,
                &root_aliases,
                &mut constraint_cache,
            )?;
        }
    }
    tracing::debug!(
        elapsed_ms = push_started.elapsed().as_millis(),
        packages = packages.len(),
        "converted the metadata closure into pool packages"
    );

    let advisory_started = Instant::now();
    let packages = if let Some(advisories) = &advisories {
        filter_advisories(packages, exempt_upto, advisories).await?
    } else {
        packages
    };
    tracing::debug!(
        elapsed_ms = advisory_started.elapsed().as_millis(),
        packages = packages.len(),
        "filtered the pool for advisories (--offline: should bail fast)"
    );
    let pool = Pool::new(packages);
    let request = Request {
        requires: root_requires(&require, &require_dev)?,
        fixed,
    };
    let policy = if preferred.is_empty() {
        DefaultPolicy::new(prefer_stable, prefer_lowest)
    } else {
        DefaultPolicy::with_preferred_versions(prefer_stable, prefer_lowest, preferred.clone())
    };
    let optimize_started = Instant::now();
    let optimized = pool_optimizer::optimize(&request, pool, &policy, &mut constraint_cache)?;
    tracing::debug!(
        elapsed_ms = optimize_started.elapsed().as_millis(),
        pool_packages = optimized.pool.len(),
        "pruned the pool before rule generation"
    );
    let Request { requires, .. } = request;

    Ok(BuildResult {
        pool: optimized.pool,
        request: Request {
            requires,
            fixed: optimized.fixed,
        },
        minimum_stability,
        stability_flags: stability_flags
            .into_iter()
            .map(|(name, stability)| (name, stability_rank(stability)))
            .collect(),
        platform_reqs: extract_platform_requirements(&require),
        platform_dev_reqs: extract_platform_requirements(&require_dev),
        platform_overrides,
    })
}

/// Turns one `composer.lock` package entry (`ArrayDumper`-shaped: the same
/// `require`/`conflict`/`provide`/`replace` fields a provider-file version
/// has) into a pool [`Package`], for a partial update's locked-out names.
/// No branch-alias or root-alias reconstruction (`push_package_version`'s
/// two extra cases): a lock entry that is itself a branch alias already
/// carries that alias's own version/requires, and nothing in a partial
/// update looks up a *further* alias of a package it isn't refetching.
fn package_from_lock_entry(entry: &Value, cache: &mut ConstraintCache) -> Result<Package> {
    let obj = entry
        .as_object()
        .context("lock package entry is not an object")?;
    let name = obj
        .get("name")
        .and_then(Value::as_str)
        .context("lock package entry missing name")?
        .to_ascii_lowercase();
    let pretty_version = obj
        .get("version")
        .and_then(Value::as_str)
        .context("lock package entry missing version")?
        .to_string();
    let version = semver::normalize(&pretty_version)?;
    let stability = semver::stability(version.as_str());
    let is_dev = stability == "dev";

    let requires = parse_links(&map_field(obj, "require"), &name, &pretty_version, cache)?;
    let conflicts = parse_links(&map_field(obj, "conflict"), &name, &pretty_version, cache)?;
    let provides = parse_links(&map_field(obj, "provide"), &name, &pretty_version, cache)?;
    let replaces = parse_links(&map_field(obj, "replace"), &name, &pretty_version, cache)?;

    Ok(Package {
        name,
        version,
        pretty_version,
        stability,
        is_dev,
        requires,
        conflicts,
        provides,
        replaces,
        alias_of: None,
        is_root_package_alias: false,
        has_self_version_requires: false,
        raw: Arc::new(entry.clone()),
    })
}

fn map_field(obj: &Map<String, Value>, key: &str) -> Map<String, Value> {
    obj.get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `Installer::extractPlatformRequirements`: the root require/require-dev
/// entries that target a platform package, pretty constraint text
/// unchanged, for the lock's `platform`/`platform-dev` keys.
fn extract_platform_requirements(links: &Map<String, Value>) -> Map<String, Value> {
    links
        .iter()
        .filter(|(name, _)| crate::repository::is_platform_package(name))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

/// The dev-split second solve's request (`Installer::requirePackagesForUpdate`
/// with `$requireDevSection = false`): root `require` only, against the
/// same `fixed_count` platform packages the first solve's pool starts with
/// (`clone_package` copies them verbatim, so their pool indices line up).
pub(crate) fn require_only_request(root: &Value, fixed_count: usize) -> Result<Request> {
    let require = string_map(root, "require");
    let mut requires = Vec::with_capacity(require.len());
    for (name, value) in &require {
        let raw = value
            .as_str()
            .with_context(|| format!("require {name}: constraint is not a string"))?;
        requires.push(crate::solver::request::RootRequire {
            name: name.to_ascii_lowercase(),
            constraint: Some(semver::parse_constraint(raw)?),
            pretty_constraint: raw.to_string(),
        });
    }
    Ok(Request {
        requires,
        fixed: (0..fixed_count).collect(),
    })
}

fn string_map(root: &Value, key: &str) -> Map<String, Value> {
    root.get(key)
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// `VersionParser::normalizeStability`.
fn normalize_stability(raw: &str) -> &'static str {
    match raw.to_ascii_lowercase().as_str() {
        "rc" => "RC",
        "beta" => "beta",
        "alpha" => "alpha",
        "dev" => "dev",
        _ => "stable",
    }
}

fn stability_rank(stability: &str) -> u8 {
    match stability {
        "stable" => 0,
        "RC" => 5,
        "beta" => 10,
        "alpha" => 15,
        "dev" => 20,
        _ => unreachable!("unknown stability {stability:?}"),
    }
}

/// `RootPackageLoader::extractAliases`: `"X as Y"` in a require's
/// constraint text names a root alias, recorded here as `package -> [(X
/// normalized, Y pretty, Y normalized)]` (`RepositorySet::getRootAliasesPerPackage`'s
/// shape, keyed by target package rather than by package+version so
/// `push_package_version` can look up by name alone).
fn extract_alias(
    name: &str,
    raw: &str,
    root_aliases: &mut HashMap<String, Vec<(String, String, String)>>,
) -> Result<()> {
    static ALIAS: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?:^|\| *|, *)([^,\s#|]+)(?:#\S+)? +as +([^,\s|]+)(?:$| *\|| *,)").unwrap()
    });

    let Some(m) = ALIAS.captures(raw) else {
        return Ok(());
    };
    let version = semver::normalize(&m[1])?.as_str().to_string();
    let alias_normalized = semver::normalize(&m[2])?.as_str().to_string();
    root_aliases
        .entry(name.to_ascii_lowercase())
        .or_default()
        .push((version, m[2].to_string(), alias_normalized));
    Ok(())
}

/// `RootPackageLoader::extractStabilityFlags`, simplified: splits on `||`
/// for OR-groups (skipping the AND-splitting on `,`/space Composer also
/// does; a root require with an explicit `@stability` *and* a separate
/// AND-ed version constraint in the same require is not exercised by any
/// fixture here). An explicit `@stability` suffix wins outright; otherwise
/// an unstable-looking bare version (`dev-main`, `2.x-dev`, `1.0-beta1`)
/// sets the flag only if it is less stable than the minimum.
fn extract_stability_flag(
    name: &str,
    raw: &str,
    minimum_stability: &str,
    stability_flags: &mut HashMap<String, &'static str>,
) {
    static STABILITY_SUFFIX: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)^[^@]*?@(stable|rc|beta|alpha|dev)$").unwrap()
    });
    static ALIAS_SUFFIX: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"^([^,\s@]+) as .+$").unwrap());

    let name = name.to_ascii_lowercase();
    let mut matched_explicit = false;

    for constraint in raw.split("||").map(str::trim) {
        if let Some(m) = STABILITY_SUFFIX.captures(constraint) {
            let stability = normalize_stability(&m[1]);
            set_flag_if_less_stable(stability_flags, &name, stability);
            matched_explicit = true;
        }
    }
    if matched_explicit {
        return;
    }

    for constraint in raw.split("||").map(str::trim) {
        let bare = ALIAS_SUFFIX.replace(constraint, "$1");
        if bare.contains(['@', ',', ' ']) {
            continue;
        }
        let stability = semver::stability(&bare);
        if stability == "stable" {
            continue;
        }
        if stability_rank(stability) <= stability_rank(minimum_stability) {
            continue;
        }
        set_flag_if_less_stable(stability_flags, &name, stability);
    }
}

fn set_flag_if_less_stable(
    flags: &mut HashMap<String, &'static str>,
    name: &str,
    stability: &'static str,
) {
    if let Some(existing) = flags.get(name)
        && stability_rank(existing) > stability_rank(stability)
    {
        return;
    }
    flags.insert(name.to_string(), stability);
}

fn is_acceptable(
    name: &str,
    stability: &str,
    acceptable: &HashSet<&'static str>,
    flags: &HashMap<String, &'static str>,
) -> bool {
    match flags.get(name) {
        Some(&flag) => stability_rank(stability) <= stability_rank(flag),
        None => acceptable.contains(stability),
    }
}

/// `Preg::replace('{(\.9{7})+}', '.x', $aliasNormalized)`.
fn pretty_alias(normalized: &str) -> String {
    static NINES: std::sync::LazyLock<Regex> =
        std::sync::LazyLock::new(|| Regex::new(r"(\.9{7})+").unwrap());
    NINES.replace(normalized, ".x").into_owned()
}

/// An alias's copy of the aliased package's links (`AliasPackage`'s
/// constructor copies `getRequires`/`getConflicts`/`getProvides`/
/// `getReplaces` verbatim): `Link::constraint`'s `Arc` (`solver::
/// ConstraintCache`) makes this a pointer clone rather than a reparse.
fn clone_links(links: &[Link]) -> Vec<Link> {
    links
        .iter()
        .map(|link| Link {
            target: link.target.clone(),
            constraint: link.constraint.clone(),
            pretty_constraint: link.pretty_constraint.clone(),
        })
        .collect()
}

/// Clones one [`Package`] into a pool built from scratch (the dev-split
/// second solve's pool: `Installer::extractDevPackages`'s `$resultRepo`,
/// re-dumping and reloading each first-solve package; vivace clones the
/// already-built `Package` instead of round-tripping through the array
/// dumper/loader, an equivalent transform since nothing about the package
/// changes). Drops any alias wrapping (`alias_of` reset to `None`):
/// `LockTransaction::getNewLockPackages` skips `AliasPackage` objects before
/// `$resultRepo` is built, so this is only ever called on a non-alias
/// package. The caller in `solver::resolve` then re-derives the branch alias
/// from `extra.branch-alias`, as Composer's `PoolBuilder` does when it
/// reloads that repository (#172).
pub(crate) fn clone_package(package: &Package) -> Package {
    Package {
        name: package.name.clone(),
        version: package.version.clone(),
        pretty_version: package.pretty_version.clone(),
        stability: package.stability,
        is_dev: package.is_dev,
        requires: clone_links(&package.requires),
        conflicts: clone_links(&package.conflicts),
        provides: clone_links(&package.provides),
        replaces: clone_links(&package.replaces),
        alias_of: None,
        is_root_package_alias: false,
        has_self_version_requires: package.has_self_version_requires,
        raw: package.raw.clone(),
    }
}

/// `ArrayLoader::parseLinks`: `self.version`, resolved to the package's own
/// version (`AliasPackage::replaceSelfVersionDependencies`'s same rule, an
/// exact-version constraint rather than a range parse, leaning on
/// `parse_constraint` treating a bare version string as `== version`).
/// Shared by [`parse_links`] and [`crate::repository`]'s closure walk, which
/// must apply the same substitution before it ever gets to
/// `parse_constraint_cached` — Composer does this at package-load time, so
/// its own require-walk never sees the literal either.
pub(crate) fn link_constraint_text<'a>(own_pretty_version: &'a str, raw: &'a str) -> &'a str {
    if raw == "self.version" {
        own_pretty_version
    } else {
        raw
    }
}

fn parse_links(
    map: &Map<String, Value>,
    own_name: &str,
    own_pretty_version: &str,
    cache: &mut ConstraintCache,
) -> Result<Vec<Link>> {
    let mut links = Vec::with_capacity(map.len());
    for (target, value) in map {
        let target = target.to_ascii_lowercase();
        if target == own_name {
            continue;
        }
        let raw = value
            .as_str()
            .with_context(|| format!("{own_name}: link constraint for {target} is not a string"))?;
        // `pretty_constraint` gets the resolved text too, matching
        // `replaceSelfVersionDependencies`'s own
        // `$constraint->setPrettyString($prettyVersion)`: this Link is
        // solver-internal (the lock writer reads `Package::raw`'s untouched
        // JSON instead, which keeps the literal `"self.version"` string),
        // but `clone_links` reparses `pretty_constraint` verbatim for a
        // branch-alias/root-alias copy or the dev-split second solve, and a
        // literal `"self.version"` isn't parseable on that second pass.
        let text = link_constraint_text(own_pretty_version, raw);
        links.push(Link {
            target,
            constraint: Some(parse_constraint_cached(cache, text)?),
            pretty_constraint: Some(text.to_string()),
        });
    }
    Ok(links)
}

/// Filters, aliases (branch and root) and pushes one provider-file entry.
/// A branch-alias version pushes both the alias and the real package it
/// aliases (`ComposerRepository.php:1350-1353`); a root-aliased version
/// additionally pushes a root-alias wrapper. Order matters here: it fixes
/// each pushed package's pool id, and pool id is `Solver`/`DefaultPolicy`'s
/// final tie-break.
fn push_package_version(
    packages: &mut Vec<Package>,
    pv: PackageVersion,
    acceptable: &HashSet<&'static str>,
    stability_flags: &HashMap<String, &'static str>,
    root_aliases: &HashMap<String, Vec<(String, String, String)>>,
    cache: &mut ConstraintCache,
) -> Result<()> {
    let name = pv.name.to_ascii_lowercase();
    let version = semver::normalize(&pv.version)?;
    let stability = semver::stability(version.as_str());

    if !is_acceptable(&name, stability, acceptable, stability_flags) {
        return Ok(());
    }

    let requires = parse_links(&pv.require, &name, &pv.version, cache)?;
    let conflicts = parse_links(&pv.conflict, &name, &pv.version, cache)?;
    let provides = parse_links(&pv.provide, &name, &pv.version, cache)?;
    let replaces = parse_links(&pv.replace, &name, &pv.version, cache)?;
    let is_dev = stability == "dev";
    let alias_target = branch_alias_target(&pv);
    // Moved once, not deep-cloned per push: a branch-alias or root-alias
    // version pushes two or three `Package`s off this one `pv`, and the
    // pool holds 45k+ of these on a Laravel-sized closure — an `Arc::clone`
    // for the extra pushes instead of re-cloning the whole JSON tree
    // (`bench/results/profile.md`'s named candidate).
    let raw = Arc::new(pv.raw);
    let pretty_version = pv.version;

    let real_index = if let Some(alias_normalized) = alias_target {
        let alias_index = packages.len();
        let real_index = alias_index + 1;
        packages.push(Package {
            name: name.clone(),
            version: semver::normalize(&alias_normalized)?,
            pretty_version: pretty_alias(&alias_normalized),
            stability: semver::stability(&alias_normalized),
            is_dev: true,
            requires: clone_links(&requires),
            conflicts: clone_links(&conflicts),
            provides: clone_links(&provides),
            replaces: clone_links(&replaces),
            alias_of: Some(real_index),
            is_root_package_alias: false,
            // Not detected: `RuleSetGenerator`/`Problem` are the only
            // readers (a nicer conflict message), neither in this stage's
            // scope. `clone_links` still resolves a `self.version` link
            // correctly (`parse_links`'s doc comment) even though this flag
            // doesn't track it.
            has_self_version_requires: false,
            raw: Arc::clone(&raw),
        });
        packages.push(Package {
            name: name.clone(),
            version,
            pretty_version: pretty_version.clone(),
            stability,
            is_dev,
            requires,
            conflicts,
            provides,
            replaces,
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
            raw: Arc::clone(&raw),
        });
        real_index
    } else {
        let index = packages.len();
        packages.push(Package {
            name: name.clone(),
            version,
            pretty_version: pretty_version.clone(),
            stability,
            is_dev,
            requires,
            conflicts,
            provides,
            replaces,
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
            raw,
        });
        index
    };

    // Root alias: matched against the plain (non-branch-alias) version
    // only. Composer also matches a root alias against an *already*
    // branch-aliased package's own alias version
    // (`PoolBuilder::loadPackage` runs once per pool entry, and a
    // branch-aliased provider file entry produces two); no fixture in this
    // stage's corpus combines the two, so only the common case is ported.
    if let Some(aliases) = root_aliases.get(&name) {
        let real = &packages[real_index];
        let real_version = real.version.as_str().to_string();
        for (target_version, alias_pretty, alias_normalized) in aliases {
            if target_version != &real_version {
                continue;
            }
            let real = &packages[real_index];
            let requires = clone_links(&real.requires);
            let conflicts = clone_links(&real.conflicts);
            let provides = clone_links(&real.provides);
            let replaces = clone_links(&real.replaces);
            let raw = real.raw.clone();
            packages.push(Package {
                name: name.clone(),
                version: semver::normalize(alias_normalized)?,
                pretty_version: alias_pretty.clone(),
                stability: semver::stability(alias_normalized),
                is_dev: semver::stability(alias_normalized) == "dev",
                requires,
                conflicts,
                provides,
                replaces,
                alias_of: Some(real_index),
                is_root_package_alias: true,
                has_self_version_requires: false,
                raw,
            });
        }
    }

    Ok(())
}

/// Lowercased targets of root `replace` (`RootPackageLoader`'s `replace`
/// section, `ArrayLoader::parseLinks`): never fetched by the closure walk,
/// matching `PoolBuilder::buildPool`'s `loadedPackages[$link->getTarget()]
/// = new MatchAllConstraint()` for every fixed package's replace link. Not
/// `provide`: a `provide`d name still admits a real package of that name as
/// a candidate, only a `replace` conflicts with every version of the name.
pub(crate) fn root_replaced_names(root: &Value) -> HashSet<String> {
    string_map(root, "replace")
        .keys()
        .map(|name| name.to_ascii_lowercase())
        .collect()
}

/// The root `composer.json` package as a fixed pool member
/// (`Installer::createRequest`'s `$request->fixPackage($rootPackage)`,
/// `RootPackageLoader`): carries `replace`/`provide` as `Link`s so
/// `Pool::whatProvides` satisfies another package's require of a replaced
/// or provided name exactly as a real Composer rule would
/// (`RuleSetGenerator::addRulesForPackage`'s `$this->pool->whatProvides`
/// call is name-based, blind to whether the provider is the root). No
/// `requires`/`conflicts`: see this module's doc comment for why the root's
/// own requires stay modelled as `request.requires` only.
pub(crate) fn root_package(root: &Value, cache: &mut ConstraintCache) -> Result<Package> {
    let name = root
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("__root__")
        .to_ascii_lowercase();
    // `RootPackageLoader::load`: `$config['version'] = '1.0.0'` when
    // nothing (no `version` key, no VCS guess) supplies one.
    let pretty_version = root
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("1.0.0")
        .to_string();
    let version = semver::normalize(&pretty_version)?;
    let stability = semver::stability(version.as_str());

    let provides = parse_links(&string_map(root, "provide"), &name, &pretty_version, cache)?;
    let replaces = parse_links(&string_map(root, "replace"), &name, &pretty_version, cache)?;
    // Never reached: `transaction::resolved_packages` drops fixed packages
    // before a lock ever sees them (platform packages' same stand-in).
    let raw = Arc::new(serde_json::json!({ "name": name, "version": pretty_version }));

    Ok(Package {
        name,
        stability,
        is_dev: stability == "dev",
        version,
        pretty_version,
        requires: Vec::new(),
        conflicts: Vec::new(),
        provides,
        replaces,
        alias_of: None,
        is_root_package_alias: false,
        has_self_version_requires: false,
        raw,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, non-alias pool package: `raw` carries `abandoned` so
    /// `audit::is_abandoned` has something to read.
    fn package(name: &str, pretty_version: &str, abandoned: bool) -> Package {
        let version = semver::normalize(pretty_version).unwrap();
        Package {
            stability: semver::stability(version.as_str()),
            is_dev: false,
            name: name.to_string(),
            version,
            pretty_version: pretty_version.to_string(),
            requires: Vec::new(),
            conflicts: Vec::new(),
            provides: Vec::new(),
            replaces: Vec::new(),
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
            raw: Arc::new(serde_json::json!({
                "name": name,
                "version": pretty_version,
                "abandoned": abandoned,
            })),
        }
    }

    /// A [`Package`] answering a fixed advisory-response body: mirrors
    /// `tests/update.rs`'s `AdvisoriesFixture`, kept local since a unit test
    /// can't reach across the `tests/` boundary.
    struct FixtureAdvisories(Value);

    impl AdvisoriesTransport for FixtureAdvisories {
        #[allow(
            clippy::unused_async_trait_impl,
            reason = "the fixture answers synchronously; the trait is async for production"
        )]
        async fn post_advisories(&self, _packages: &[String]) -> Result<Value> {
            Ok(self.0.clone())
        }
    }

    fn insecure_audit() -> AuditConfig {
        AuditConfig {
            block_insecure: true,
            block_abandoned: false,
            ..AuditConfig::default()
        }
    }

    fn abandoned_audit() -> AuditConfig {
        AuditConfig {
            block_insecure: false,
            block_abandoned: true,
            ..AuditConfig::default()
        }
    }

    /// #175: a root-alias pool entry's `alias_of` still points at its real
    /// package's index once the advisory filter drops that real package
    /// from the middle of the `Vec` — before this fix, nothing remapped or
    /// cascaded that dangling index, which is exactly what made
    /// `pool_optimizer::optimize`'s own remap (`pool_optimizer.rs:191`)
    /// panic with "no entry found for key" downstream. An alias and the
    /// package it aliases must be kept or removed together, matching
    /// `pool_optimizer::optimize`'s own alias guard.
    #[tokio::test]
    async fn filter_advisories_drops_an_alias_alongside_its_filtered_real_package() {
        let real = package("vendor/pkg", "3.11.0", false);
        let mut alias = package("vendor/pkg", "4.0.0", false);
        alias.alias_of = Some(0);
        alias.is_root_package_alias = true;
        let packages = vec![real, alias];

        let advisories = FixtureAdvisories(serde_json::json!({
            "advisories": {
                "vendor/pkg": [{
                    "advisoryId": "PKSA-test-0001",
                    "packageName": "vendor/pkg",
                    "affectedVersions": ">=3.11.0,<3.11.1",
                    "title": "fixture",
                    "cve": null,
                    "link": null,
                    "reportedAt": "2024-01-01 00:00:00",
                }],
            },
        }));
        let audit = insecure_audit();
        let filter = AdvisoryFilter {
            transport: &advisories,
            audit: &audit,
            no_blocking: false,
        };

        let kept = filter_advisories(packages, 0, &filter).await.unwrap();
        assert!(
            kept.is_empty(),
            "the alias must not outlive its filtered real package: {} left",
            kept.len(),
        );
    }

    /// #175: filtering a package positioned *before* an unrelated alias
    /// pair must shift the alias's own surviving `alias_of` down to match —
    /// the same remap `pool_optimizer::optimize` already does for its own
    /// removals.
    #[tokio::test]
    async fn filter_advisories_remaps_alias_of_after_an_earlier_removal() {
        let abandoned = package("vendor/other", "1.0.0", true);
        let real = package("vendor/pkg", "1.0.0", false);
        let mut alias = package("vendor/pkg", "2.0.0", false);
        alias.alias_of = Some(1);
        alias.is_root_package_alias = true;
        let packages = vec![abandoned, real, alias];

        let audit = abandoned_audit();
        let filter = AdvisoryFilter {
            transport: &NoAdvisories,
            audit: &audit,
            no_blocking: false,
        };

        let kept = filter_advisories(packages, 0, &filter).await.unwrap();
        assert_eq!(kept.len(), 2, "expected the real+alias pair to survive");
        assert_eq!(kept[1].alias_of, Some(0), "stale index was never remapped");
    }
}
