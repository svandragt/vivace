//! Port of `DependencyResolver/PoolBuilder.php`, `Installer.php`'s request
//! wiring (`createRequest`/`requirePackagesForUpdate`, ~line 900-1070),
//! `Package/Loader/RootPackageLoader.php`'s alias/stability-flag extraction
//! and `Repository/PlatformRepository.php`'s fixed platform packages.
//!
//! Full update only (`docs/resolver-design.md` stage 3): no partial-update
//! allow-list or path-repo unlocking, security-advisory/filter-list pool
//! filters. `Repository::load_closure` already does the breadth-first,
//! batched metadata load `PoolBuilder::loadPackagesMarkedForLoading`
//! performs in real Composer, so this only needs to turn that closure into
//! `Package`s and filter/alias them. `pool_optimizer::optimize` runs right
//! after the raw pool is assembled, exactly where
//! `PoolBuilder::buildPool`'s own `runOptimizer` call sits (#76).
//!
//! Not modelled: the root `composer.json` package itself as a *pool*
//! member (`Installer::createRequest` also does
//! `$request->fixPackage($rootPackage)`). Its own `require`/`require-dev`
//! links become `request.requires` directly, which is the exact same
//! constraint a root package's own `RULE_PACKAGE_REQUIRES` rules would add
//! on top (the root is always force-installed, so `-root` is never true,
//! collapsing that rule to the same "install one of" disjunction the
//! `RULE_ROOT_REQUIRE` rule already states). Root `conflict`/`replace`/
//! `provide` sections would need the root modelled as a real pool package;
//! add it if a fixture ever needs one.

use std::collections::{HashMap, HashSet};
use std::process::Command;

use anyhow::{Context, Result};
use regex::Regex;
use serde_json::{Map, Value};

use crate::repository::{ClosureRoot, DevAcceptance, PackageVersion, Repository, Transport};
use crate::semver;
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

/// `Installer::doUpdate`'s first solve: root `require` and `require-dev`
/// merged (`Installer.php:1061-1067`), against every package the
/// repository's closure discovers.
pub async fn build<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
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

    // `ComposerRepository::loadAsyncPackages`'s `~dev` skip logic
    // (`docs/resolver-design.md`'s Metadata section), applied once for the
    // whole closure rather than per name: `Repository::load_closure` takes
    // one `DevAcceptance` for its whole breadth-first walk (stage 2's
    // API), and over-fetching a `~dev` file for a name that turns out not
    // to need it just costs a request the stability filter below discards
    // the results of, never a wrong answer.
    let dev_acceptance =
        if acceptable.contains("dev") || stability_flags.values().any(|&s| s == "dev") {
            DevAcceptance::Both
        } else {
            DevAcceptance::NonDevOnly
        };

    let roots = [ClosureRoot {
        require: &require,
        require_dev: &require_dev,
    }];
    let closure = repo.load_closure(&roots, dev_acceptance).await?;

    let platform_overrides = root
        .pointer("/config/platform")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut packages = platform_packages(&platform_overrides)?;
    let fixed: Vec<usize> = (0..packages.len()).collect();

    let mut constraint_cache: ConstraintCache = ConstraintCache::new();
    for versions in closure.values() {
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

    let pool = Pool::new(packages);
    let request = Request {
        requires: root_requires(&require, &require_dev)?,
        fixed,
    };
    let policy = DefaultPolicy::new(prefer_stable, prefer_lowest);
    let optimized = pool_optimizer::optimize(&request, pool, &policy, &mut constraint_cache)?;
    // `optimize` only borrows `request`; `requires` is still ours to move
    // into the final, remapped `Request` (`fixed` alone changes, `optimize`
    // reindexes it to match the pruned pool).
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

    let skip: HashSet<String> = locked_by_name
        .keys()
        .filter(|name| !allow_names.contains(*name))
        .cloned()
        .collect();

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
    let closure = repo
        .load_closure_skipping(&roots, dev_acceptance, &skip)
        .await?;

    let platform_overrides = root
        .pointer("/config/platform")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let mut packages = platform_packages(&platform_overrides)?;
    let fixed: Vec<usize> = (0..packages.len()).collect();

    let mut constraint_cache: ConstraintCache = ConstraintCache::new();
    for name in &skip {
        if let Some(entry) = locked_by_name.get(name) {
            packages.push(package_from_lock_entry(entry, &mut constraint_cache)?);
        }
    }

    for versions in closure.values() {
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

    let pool = Pool::new(packages);
    let request = Request {
        requires: root_requires(&require, &require_dev)?,
        fixed,
    };
    let policy = DefaultPolicy::new(prefer_stable, prefer_lowest);
    let optimized = pool_optimizer::optimize(&request, pool, &policy, &mut constraint_cache)?;
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
        raw: entry.clone(),
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

/// `ArrayLoader::getBranchAlias`: `extra.branch-alias` names, for a `dev-*`
/// version, the normalized target branch it stands in for.
fn branch_alias_target(pv: &PackageVersion) -> Option<String> {
    if !(pv.version.starts_with("dev-") || pv.version.ends_with("-dev")) {
        return None;
    }
    let target = pv
        .branch_alias
        .as_ref()?
        .as_object()?
        .get(&pv.version)?
        .as_str()?;
    if !target.ends_with("-dev") {
        return None;
    }
    if target == "9999999-dev" {
        return Some(target.to_string());
    }
    let branch_name = &target[..target.len() - 4];
    let normalized = semver::normalize_branch(branch_name);
    if !normalized.ends_with("-dev") {
        return None;
    }
    Some(normalized)
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
/// changes). Drops any alias wrapping (`alias_of` reset to `None`): the
/// second solve's repository never carries `AliasPackage` entries either
/// (`LockTransaction::getNewLockPackages` skips them before `$resultRepo` is
/// built), so this is only ever called on a non-alias package.
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
        // `self.version`, resolved to the package's own version like
        // `AliasPackage::replaceSelfVersionDependencies` (an exact-version
        // constraint, not a range parse, leaning on `parse_constraint`
        // treating a bare version string as `== version` rather than
        // constructing that constraint directly). `pretty_constraint` gets
        // the resolved text too, matching `replaceSelfVersionDependencies`'s
        // own `$constraint->setPrettyString($prettyVersion)`: this Link is
        // solver-internal (the lock writer reads `Package::raw`'s untouched
        // JSON instead, which keeps the literal `"self.version"` string),
        // but `clone_links` reparses `pretty_constraint` verbatim for a
        // branch-alias/root-alias copy or the dev-split second solve, and a
        // literal `"self.version"` isn't parseable on that second pass.
        let text = if raw == "self.version" {
            own_pretty_version
        } else {
            raw
        };
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
    pv: &PackageVersion,
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

    let real_index = if let Some(alias_normalized) = branch_alias_target(pv) {
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
            raw: pv.raw.clone(),
        });
        packages.push(Package {
            name: name.clone(),
            version,
            pretty_version: pv.version.clone(),
            stability,
            is_dev,
            requires,
            conflicts,
            provides,
            replaces,
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
            raw: pv.raw.clone(),
        });
        real_index
    } else {
        let index = packages.len();
        packages.push(Package {
            name: name.clone(),
            version,
            pretty_version: pv.version.clone(),
            stability,
            is_dev,
            requires,
            conflicts,
            provides,
            replaces,
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
            raw: pv.raw.clone(),
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

/// `Installer::createRequest`'s platform-fixing loop, backed by a
/// deliberately small stand-in for `Repository/PlatformRepository.php`:
/// the running PHP's own version plus its loaded extensions, each given
/// the interpreter's version rather than the real per-library version
/// `PlatformRepository::initialize` painstakingly parses from
/// `phpinfo()`-style extension info.
///
/// ponytail: exact library versions (`lib-openssl`, `lib-icu`, ...) are not
/// reproduced; a `composer.json` pinning one of those (`"lib-openssl":
/// "^1.1"`, rather than the common `"ext-openssl": "*"`) will not resolve.
/// Widen `extension_package` if a fixture needs it.
///
/// `overrides` is `config.platform` verbatim (`Installer::doUpdate`'s
/// `$this->config->get('platform')`): a name -> pretty-version string pins
/// that platform package's version instead of detecting it from the host,
/// and a name -> `false` removes it outright (`PlatformRepository`'s own
/// `platform-overrides`/`platform` handling). Hermetic tests use this to
/// avoid depending on the host `php` build; a name not already detected on
/// the host is not added (ponytail: only pinning an existing platform
/// package is supported, not inventing a new one).
fn platform_packages(overrides: &Map<String, Value>) -> Result<Vec<Package>> {
    let php_pretty = detect_php_version().unwrap_or_else(|| "8.3.0".to_string());

    let mut pretty: Vec<(String, String)> = vec![
        ("php".to_string(), php_pretty.clone()),
        ("composer-plugin-api".to_string(), "2.9.0".to_string()),
        ("composer-runtime-api".to_string(), "2.2.2".to_string()),
    ];

    for extension in detect_extensions() {
        let lower = extension.to_ascii_lowercase();
        if lower == "core" || lower == "standard" {
            continue;
        }
        let package_name = format!("ext-{}", lower.replace(' ', "-"));
        pretty.push((package_name, php_pretty.clone()));
    }

    for (name, value) in overrides {
        match value {
            Value::String(version) => {
                if let Some(entry) = pretty.iter_mut().find(|(n, _)| n == name) {
                    entry.1.clone_from(version);
                } else {
                    pretty.push((name.clone(), version.clone()));
                }
            }
            Value::Bool(false) => pretty.retain(|(n, _)| n != name),
            _ => {}
        }
    }

    pretty
        .into_iter()
        .map(|(name, version)| {
            let normalized = semver::normalize(&version)?;
            Ok(platform_package(&name, &version, normalized))
        })
        .collect()
}

fn platform_package(
    name: &str,
    pretty_version: &str,
    version: semver::NormalizedVersion,
) -> Package {
    Package {
        name: name.to_string(),
        stability: semver::stability(version.as_str()),
        is_dev: false,
        version,
        pretty_version: pretty_version.to_string(),
        requires: Vec::new(),
        conflicts: Vec::new(),
        provides: Vec::new(),
        replaces: Vec::new(),
        alias_of: None,
        is_root_package_alias: false,
        has_self_version_requires: false,
        raw: serde_json::json!({ "name": name, "version": pretty_version }),
    }
}

fn detect_php_version() -> Option<String> {
    let output = Command::new("php")
        .args(["-r", "echo PHP_VERSION;"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    (!version.is_empty()).then_some(version)
}

fn detect_extensions() -> Vec<String> {
    let Some(output) = Command::new("php")
        .args(["-r", "echo implode(',', get_loaded_extensions());"])
        .output()
        .ok()
        .filter(|o| o.status.success())
    else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(output.stdout) else {
        return Vec::new();
    };
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
