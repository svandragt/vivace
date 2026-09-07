//! Port of `DependencyResolver/PoolOptimizer.php`: removes pool entries no
//! rule generated from `request` could ever distinguish, before rule
//! generation runs over them (`bench/results/profile.md` §2.3: rule
//! generation, not solving, is ~56% of `viv update`'s wall time on a
//! Laravel-sized closure). Run from `pool_builder::build`/`build_partial`
//! right after the raw pool is assembled, exactly where
//! `PoolBuilder::buildPool` calls
//! `$pool = $this->poolOptimizer->optimize($request, $pool)`. That call's
//! only guard (`null === $this->poolOptimizer`, `COMPOSER_POOL_OPTIMIZER=0`,
//! a debugging-only env toggle read in `Installer::createPoolOptimizer`)
//! has no CLI surface in vivace to gate, so this always runs.
//!
//! Two passes, in Composer's own order:
//! - `optimize_by_identical_dependencies`: among same-name versions that
//!   satisfy the same external requirement/conflict constraint and share an
//!   identical dependency fingerprint (requires+conflicts+replaces+provides),
//!   keeps only the one [`DefaultPolicy::select_preferred_packages`] would
//!   pick; the rest can never be distinguished by any rule the solver would
//!   generate, so removing them changes nothing about the answer.
//! - `optimizeImpossiblePackagesAway`: drops versions no *locked* package's
//!   exact require could ever pick. Not reachable from [`optimize`]:
//!   `Request` (this port's cut-down `request.rs`, "full update only") never
//!   models locked packages, and real Composer's own `PoolBuilder::buildPool`
//!   only ever calls `$request->lockPackage()` inside its partial-update
//!   branch (`getUpdateAllowList()` non-empty) — so a full update never
//!   populates `getLockedPackages()` either, and this pass's own guard
//!   (`count($request->getLockedPackages()) === 0`) already no-ops for the
//!   case this crate exercises today. `optimize_impossible_packages_away`
//!   is still ported in full (a locked-package index list parameter) so the
//!   behaviour exists the day `Request` grows one; this module's own tests
//!   exercise it directly with a non-empty list.
//!
//! An alias and the package it aliases are always kept or removed together
//! (`markPackageIrremovable`/`keepPackageInGroup`'s own recursion through
//! `AliasPackage::getAliasOf`) — the alias guard the port has to mirror,
//! since an `AliasPackage` split from its `aliasOf` is not a package
//! Composer's model can express.
//!
//! Skipped: `removedVersionsByPackage` bookkeeping (`Problem.php`'s verbose
//! "X, Y removed by ..." annotation) — `problem.rs`'s own doc already scopes
//! that out (no verbose removed-version listing at all, even for
//! pool-build-time filtering), so there is nothing here for it to feed.
//! Package existence/constraint-match checks `problem.rs` already makes
//! (`Pool::what_provides`) run against the *optimized* pool exactly like
//! Composer's own `Problem` does, so no further wiring is needed there for
//! reasons to still name the right packages.
//!
//! Disjunctive requires (`^7.2 || ^8.0`) still need their own group per
//! branch (`expandDisjunctiveMultiConstraints`'s reason: merging them would
//! let `selectPreferredPackages` pick one branch's best version and prune
//! every candidate the *other* branch needed — not just a worse pick, a
//! removed version the solver actually needs to satisfy that rule). This
//! port splits on a literal `||` in the constraint's pretty text
//! (`pool_builder.rs::extract_stability_flag` already does the same split
//! for the same reason) rather than porting `Intervals::compactConstraint`'s
//! semantic interval merge: strictly conservative, since a literal split
//! only ever creates *more* groups than the semantically compacted one
//! would, never fewer, so it can only leave extra, already-redundant
//! versions behind — never remove one the solver needs. Revisit if a real
//! closure's pool size shows the gap matters.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use anyhow::Result;

use crate::semver::{self, CompiledConstraint, Constraint, NormalizedVersion, VersionKey};
use crate::solver::policy::DefaultPolicy;
use crate::solver::pool::{self, Link, Package, Pool};
use crate::solver::request::Request;
use crate::solver::{ConstraintCache, parse_constraint_cached};

/// [`optimize`]'s result: the pruned pool, and `request.fixed`'s indices
/// remapped to match (`Pool::new` reassigns ids from scratch, so pruning
/// the middle of the packages vec shifts everything after it).
pub struct Optimized {
    pub pool: Pool,
    pub fixed: Vec<usize>,
}

/// One require/conflict disjunct, its `Constraint` compiled once
/// (`src/semver.rs`'s `CompiledConstraint`) rather than re-walked on every
/// package version this bucket is checked against (`bench/results/profile.md`
/// §2.6: this loop alone drove 3.29M `Constraint::matches` calls).
struct CompiledRequire {
    text: String,
    constraint: Arc<Constraint>,
    compiled: CompiledConstraint,
}

/// `Some`/`None` mirrors `CompiledConstraint::matches`: the fast path
/// answers directly, a dev-branch `key` falls back to the exact (slower)
/// `Constraint::matches` — see `src/semver.rs`'s module doc.
fn matches(require: &CompiledRequire, key: &VersionKey, version: &NormalizedVersion) -> bool {
    require
        .compiled
        .matches(key)
        .unwrap_or_else(|| require.constraint.matches(version))
}

type ConstraintGroups = HashMap<String, Vec<CompiledRequire>>;

/// One `$groupHashParts[] = "$tag:$text"` entry from PHP's
/// `optimizeByIdenticalDependencies`, folded into a `u64` instead of a
/// `format!`-built `String` (#91): `tag` stands in for the `'require:'` vs
/// `'conflict:'` prefix (a replace shares `require`'s tag, see the call
/// site), and `Hash for str` already length-prefixes its bytes, so two
/// hashed calls can't collide the way two unseparated string concats could.
fn hash_part(tag: u8, text: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    tag.hash(&mut hasher);
    text.hash(&mut hasher);
    hasher.finish()
}

/// `PoolOptimizer::optimize`. `policy` is only used for
/// `selectPreferredPackages`'s tie-break within an identical-dependency
/// group: the same `prefer_stable`/`prefer_lowest` policy the solve itself
/// uses (no `--minimal-changes` pin here either — `policy.rs`'s own doc
/// comment explains why nothing wires that into `update.rs` yet).
pub fn optimize(
    request: &Request,
    pool: Pool,
    policy: &DefaultPolicy,
    cache: &mut ConstraintCache,
) -> Result<Optimized> {
    let alias_groups = alias_groups(pool.packages());

    let mut irremovable: HashSet<usize> = HashSet::new();
    for &index in &request.fixed {
        mark_irremovable(index, pool.packages(), &alias_groups, &mut irremovable);
    }

    let mut require_constraints: ConstraintGroups = HashMap::new();
    let mut conflict_constraints: ConstraintGroups = HashMap::new();
    for root in &request.requires {
        add_disjuncts(
            &mut require_constraints,
            &root.name,
            &root.pretty_constraint,
            cache,
        )?;
    }
    for package in pool.packages() {
        for link in &package.requires {
            add_disjuncts(
                &mut require_constraints,
                &link.target,
                link.pretty_constraint(),
                cache,
            )?;
        }
        for link in &package.conflicts {
            add_disjuncts(
                &mut conflict_constraints,
                &link.target,
                link.pretty_constraint(),
                cache,
            )?;
        }
    }

    let mut to_remove: HashSet<usize> = HashSet::new();
    optimize_by_identical_dependencies(
        &pool,
        &irremovable,
        &alias_groups,
        &require_constraints,
        &conflict_constraints,
        policy,
        &mut to_remove,
    );

    let fixed_set: HashSet<usize> = request.fixed.iter().copied().collect();
    // Locked-package list is always empty here; see the module doc for why.
    optimize_impossible_packages_away(
        pool.packages(),
        &irremovable,
        &alias_groups,
        &fixed_set,
        &[],
        &require_constraints,
        &mut to_remove,
    );

    let owned = pool.into_packages();
    let mut kept = Vec::with_capacity(owned.len());
    let mut remap: HashMap<usize, usize> = HashMap::with_capacity(owned.len());
    for (index, package) in owned.into_iter().enumerate() {
        if to_remove.contains(&index) {
            continue;
        }
        remap.insert(index, kept.len());
        kept.push(package);
    }
    // An alias's `alias_of` is still a pre-pruning pool index at this point
    // (#116): `keep_package`/`mark_irremovable` always keep an alias and its
    // base together, so every surviving alias's base is in `remap` too — but
    // a second pass is needed since an alias can precede its base in `kept`.
    for package in &mut kept {
        if let Some(alias_of) = package.alias_of {
            package.alias_of = Some(remap[&alias_of]);
        }
    }
    let fixed = request.fixed.iter().map(|index| remap[index]).collect();

    Ok(Optimized {
        pool: Pool::new(kept),
        fixed,
    })
}

/// `aliasesPerPackage`: aliased (real) package index -> its alias indices.
fn alias_groups(packages: &[Package]) -> HashMap<usize, Vec<usize>> {
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for (index, package) in packages.iter().enumerate() {
        if let Some(real) = package.alias_of {
            groups.entry(real).or_default().push(index);
        }
    }
    groups
}

/// `PoolOptimizer::markPackageIrremovable`.
fn mark_irremovable(
    index: usize,
    packages: &[Package],
    alias_groups: &HashMap<usize, Vec<usize>>,
    irremovable: &mut HashSet<usize>,
) {
    irremovable.insert(index);
    if let Some(alias_of) = packages[index].alias_of {
        mark_irremovable(alias_of, packages, alias_groups, irremovable);
    }
    if let Some(aliases) = alias_groups.get(&index) {
        for &alias_index in aliases {
            irremovable.insert(alias_index);
        }
    }
}

/// `extractRequireConstraintsPerPackage`/`extractConflictConstraintsPerPackage`,
/// both routed through `expandDisjunctiveMultiConstraints` (see the module
/// doc for the literal `||`-split simplification). Dedupes by the
/// disjunct's own trimmed text, matching the PHP associative array's
/// `(string) $expanded` key overwrite.
fn add_disjuncts(
    groups: &mut ConstraintGroups,
    name: &str,
    pretty: &str,
    cache: &mut ConstraintCache,
) -> Result<()> {
    // `get_mut` first (not `entry(name.to_string())` straight away) so the
    // common case — a name bucket that already exists — doesn't pay a
    // `String` allocation just to look it up (`bench/results/profile.md`
    // §2.6: this runs once per require/conflict link across the
    // *unoptimized* pool, tens of thousands of calls where the same
    // package name recurs constantly). `HashMap::entry_ref` would do this
    // in one lookup, but it isn't stable yet.
    if !groups.contains_key(name) {
        groups.insert(name.to_string(), Vec::new());
    }
    let entry = groups.get_mut(name).expect("just inserted");
    for part in pretty.split("||").map(str::trim).filter(|p| !p.is_empty()) {
        if entry.iter().any(|require| require.text == part) {
            continue;
        }
        let constraint = parse_constraint_cached(cache, part)?;
        let compiled = CompiledConstraint::compile(&constraint);
        entry.push(CompiledRequire {
            text: part.to_string(),
            constraint,
            compiled,
        });
    }
    Ok(())
}

/// `PoolOptimizer::optimizeByIdenticalDependencies`.
#[allow(clippy::too_many_arguments)]
fn optimize_by_identical_dependencies(
    pool: &Pool,
    irremovable: &HashSet<usize>,
    alias_groups: &HashMap<usize, Vec<usize>>,
    require_constraints: &ConstraintGroups,
    conflict_constraints: &ConstraintGroups,
    policy: &DefaultPolicy,
    to_remove: &mut HashSet<usize>,
) {
    // name -> group hash -> dependency hash -> package indices. The group
    // hash used to be `Vec<String>` joined with `format!`/`.concat()` per
    // package/name/disjunct (#91: 3.29M `matches` calls' worth of that ran
    // through `format!` too); `hash_part` folds each disjunct straight into
    // a `u64` from its already-owned `&str` instead, so the per-(name,
    // disjunct) cost is a hash, not an allocation. `dependency_hash` is
    // `u64` for the same reason — it used to be cloned once per (name,
    // disjunct) group entry, per package, before #91.
    type IdenticalDependencyGroups = HashMap<String, HashMap<Vec<u64>, HashMap<u64, Vec<usize>>>>;
    let mut groups: IdenticalDependencyGroups = HashMap::new();

    for (index, package) in pool.packages().iter().enumerate() {
        if irremovable.contains(&index) {
            continue;
        }
        to_remove.insert(index);

        let dep_hash = dependency_hash(package);
        // Parsed once per package rather than once per (name, disjunct)
        // check below — `CompiledConstraint::matches`' whole point.
        let key = semver::parse_version_key(&package.version);

        // Independent of which require disjunct is being checked below
        // (only of `package` itself), so computed once per package rather
        // than once per bucket the PHP source's own nested loop recomputes
        // it in (`bench/results/profile.md` §2.6: this loop drove ~2.8×
        // more `matches` calls than there were disjuncts to check).
        //
        // Tag `0` (not a distinct "replace" tag): PHP's own
        // `optimizeByIdenticalDependencies` hashes a replace's constraint
        // into the same `'require:'`-prefixed part a require would use
        // ("Use the same hash part as the regular require hash because
        // that's what the replacement does"), so a replace and a require
        // with identical constraint text must fold to the same `u64` here.
        let replace_parts: Vec<u64> = package
            .replaces
            .iter()
            .filter(|replace| {
                replace
                    .constraint
                    .as_ref()
                    .is_none_or(|c| c.matches(&package.version))
            })
            .map(|replace| hash_part(0, replace.pretty_constraint()))
            .collect();

        for name in package.names(false) {
            let Some(requires) = require_constraints.get(&name) else {
                continue;
            };
            // Independent of which require disjunct is being checked below
            // (only of `name`), same reasoning as `replace_parts` above.
            // Tag `1`: conflicts get PHP's separate `'conflict:'` prefix.
            let conflict_parts: Vec<u64> = conflict_constraints
                .get(&name)
                .into_iter()
                .flatten()
                .filter(|conflict| matches(conflict, &key, &package.version))
                .map(|conflict| hash_part(1, &conflict.text))
                .collect();

            for require in requires {
                let mut parts = Vec::new();
                if matches(require, &key, &package.version) {
                    parts.push(hash_part(0, &require.text));
                }
                parts.extend(replace_parts.iter().copied());
                parts.extend(conflict_parts.iter().copied());
                if parts.is_empty() {
                    continue;
                }
                if !groups.contains_key(&name) {
                    groups.insert(name.clone(), HashMap::new());
                }
                groups
                    .get_mut(&name)
                    .expect("just inserted")
                    .entry(parts)
                    .or_default()
                    .entry(dep_hash)
                    .or_default()
                    .push(index);
            }
        }
    }

    for group_hashes in groups.into_values() {
        for dep_hashes in group_hashes.into_values() {
            for indices in dep_hashes.into_values() {
                if let [only] = indices[..] {
                    keep_package(only, pool.packages(), alias_groups, to_remove);
                    continue;
                }
                let literals: Vec<i32> = indices.iter().map(|&i| pool::id_of(i)).collect();
                for literal in policy.select_preferred_packages(pool, &literals, None) {
                    keep_package(
                        pool::index_of(literal),
                        pool.packages(),
                        alias_groups,
                        to_remove,
                    );
                }
            }
        }
    }
}

/// `PoolOptimizer::keepPackageInGroup`, minus the removed-version
/// bookkeeping (see the module doc).
fn keep_package(
    index: usize,
    packages: &[Package],
    alias_groups: &HashMap<usize, Vec<usize>>,
    to_remove: &mut HashSet<usize>,
) {
    if !to_remove.remove(&index) {
        return;
    }
    let package = &packages[index];
    if let Some(alias_of) = package.alias_of {
        to_remove.remove(&alias_of);
        if let Some(siblings) = alias_groups.get(&alias_of) {
            for &sibling in siblings {
                to_remove.remove(&sibling);
            }
        }
        return;
    }
    if let Some(siblings) = alias_groups.get(&index) {
        for &sibling in siblings {
            to_remove.remove(&sibling);
        }
    }
}

/// `PoolOptimizer::calculateDependencyHash`. `u64`, not the joined `String`
/// PHP builds (and its own `PoolOptimizer` never reads back as text either
/// — only ever compared for equality as a grouping key): folding the same
/// content into a running hash instead of a string buffer drops the
/// allocation this used to hand to `optimize_by_identical_dependencies`'s
/// per-(name, disjunct) group entry, once per package there (#91).
fn dependency_hash(package: &Package) -> u64 {
    let mut hasher = DefaultHasher::new();
    for (key, links) in [
        ("requires", &package.requires),
        ("conflicts", &package.conflicts),
        ("replaces", &package.replaces),
        ("provides", &package.provides),
    ] {
        if links.is_empty() {
            continue;
        }
        key.hash(&mut hasher);
        let mut sub: Vec<(&str, String)> = links
            .iter()
            .map(|link| (link.target.as_str(), link_constraint_text(link)))
            .collect();
        sub.sort_unstable_by_key(|&(target, _)| target);
        for (target, constraint) in sub {
            target.hash(&mut hasher);
            constraint.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn link_constraint_text(link: &Link) -> String {
    link.constraint
        .as_ref()
        .map_or_else(|| "*".to_string(), ToString::to_string)
}

/// `PoolOptimizer::optimizeImpossiblePackagesAway`. `fixed` and `locked` are
/// pool indices (`Request::isFixedPackage`/`isLockedPackage`, folded into
/// index sets since this port has no object identity to hash against);
/// [`optimize`] always calls this with `locked` empty (see the module doc),
/// so it is a no-op there and exercised directly by this module's own tests
/// instead.
#[allow(clippy::too_many_arguments)]
fn optimize_impossible_packages_away(
    packages: &[Package],
    irremovable: &HashSet<usize>,
    alias_groups: &HashMap<usize, Vec<usize>>,
    fixed: &HashSet<usize>,
    locked: &[usize],
    require_constraints: &ConstraintGroups,
    to_remove: &mut HashSet<usize>,
) {
    if locked.is_empty() {
        return;
    }

    let mut by_name: HashMap<String, HashMap<usize, &Package>> = HashMap::new();
    for (index, package) in packages.iter().enumerate() {
        if irremovable.contains(&index) {
            continue;
        }
        if alias_groups.contains_key(&index) || package.is_alias() {
            continue;
        }
        if fixed.contains(&index) || locked.contains(&index) {
            continue;
        }
        by_name
            .entry(package.name.clone())
            .or_default()
            .insert(index, package);
    }

    for &locked_index in locked {
        let locked_package = &packages[locked_index];
        let is_unused = locked_package
            .names(false)
            .iter()
            .all(|name| !require_constraints.contains_key(name));
        if is_unused {
            continue;
        }

        for link in &locked_package.requires {
            let Some(candidates) = by_name.get_mut(&link.target) else {
                continue;
            };
            candidates.retain(|&id, required| {
                let matches = link
                    .constraint
                    .as_ref()
                    .is_none_or(|c| c.matches(&required.version));
                if !matches {
                    to_remove.insert(id);
                }
                matches
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::request::RootRequire;
    use crate::solver::rule_set_generator;

    fn version(v: &str) -> semver::NormalizedVersion {
        semver::normalize(v).unwrap()
    }

    fn link_c(target: &str, constraint: &str) -> Link {
        Link {
            target: target.to_string(),
            constraint: Some(Arc::new(semver::parse_constraint(constraint).unwrap())),
            pretty_constraint: Some(constraint.to_string()),
        }
    }

    fn package(name: &str, pretty_version: &str) -> Package {
        let normalized = version(pretty_version);
        Package {
            stability: semver::stability(normalized.as_str()),
            is_dev: false,
            name: name.to_string(),
            version: normalized,
            pretty_version: pretty_version.to_string(),
            requires: Vec::new(),
            conflicts: Vec::new(),
            provides: Vec::new(),
            replaces: Vec::new(),
            alias_of: None,
            is_root_package_alias: false,
            has_self_version_requires: false,
            raw: std::sync::Arc::new(
                serde_json::json!({ "name": name, "version": pretty_version }),
            ),
        }
    }

    fn require(name: &str, constraint: &str) -> Request {
        Request {
            requires: vec![RootRequire {
                name: name.to_string(),
                constraint: Some(semver::parse_constraint(constraint).unwrap()),
                pretty_constraint: constraint.to_string(),
            }],
            fixed: Vec::new(),
        }
    }

    /// Ten patch versions with an identical dependency set collapse to the
    /// one `DefaultPolicy` would pick (highest, no `prefer-lowest`):
    /// `optimizeByIdenticalDependencies`'s whole point.
    #[test]
    fn collapses_versions_with_identical_dependencies() {
        let mut deps = package("vendor/dep", "1.0.0");
        deps.requires = vec![link_c("vendor/leaf", "^1.0")];
        let mut packages = vec![deps];
        for patch in 1..10 {
            let mut p = package("vendor/dep", &format!("1.0.{patch}"));
            p.requires = vec![link_c("vendor/leaf", "^1.0")];
            packages.push(p);
        }
        packages.push(package("vendor/leaf", "1.0.0"));
        let pool = Pool::new(packages);
        let request = require("vendor/dep", "^1.0");
        let policy = DefaultPolicy::new(false, false);

        let optimized = optimize(&request, pool, &policy, &mut ConstraintCache::new()).unwrap();

        let dep_versions: Vec<&str> = optimized
            .pool
            .packages()
            .iter()
            .filter(|p| p.name == "vendor/dep")
            .map(|p| p.pretty_version.as_str())
            .collect();
        assert_eq!(dep_versions, vec!["1.0.9"], "{dep_versions:?}");
        assert!(
            optimized
                .pool
                .packages()
                .iter()
                .any(|p| p.name == "vendor/leaf"),
            "leaf must survive: it is still required by the kept version"
        );
    }

    /// A disjunctive require (`^1.0 || ^2.0`) keeps the best version of
    /// *each* branch, not just the overall best: collapsing across the `||`
    /// would prune every 1.x candidate away, breaking a rule that could
    /// still pick one (the module doc's correctness argument for splitting).
    #[test]
    fn keeps_the_best_version_of_each_disjunctive_branch() {
        let v1 = package("vendor/dep", "1.5.0");
        let v2 = package("vendor/dep", "2.5.0");
        let pool = Pool::new(vec![v1, v2]);
        let request = require("vendor/dep", "^1.0 || ^2.0");
        let policy = DefaultPolicy::new(false, false);

        let optimized = optimize(&request, pool, &policy, &mut ConstraintCache::new()).unwrap();

        let mut dep_versions: Vec<&str> = optimized
            .pool
            .packages()
            .iter()
            .map(|p| p.pretty_version.as_str())
            .collect();
        dep_versions.sort_unstable();
        assert_eq!(dep_versions, vec!["1.5.0", "2.5.0"], "{dep_versions:?}");
    }

    /// A fixed (platform) package survives even though nothing else about
    /// it would earn it a keep (`markPackageIrremovable`).
    #[test]
    fn fixed_packages_are_never_removed() {
        let php = package("php", "8.3.0");
        let unused_other_version = package("php", "8.2.0");
        let pool = Pool::new(vec![php, unused_other_version]);
        let request = Request {
            requires: Vec::new(),
            fixed: vec![0],
        };
        let policy = DefaultPolicy::new(false, false);

        let optimized = optimize(&request, pool, &policy, &mut ConstraintCache::new()).unwrap();

        assert_eq!(optimized.fixed, vec![0]);
        assert_eq!(
            optimized
                .pool
                .package_by_id(pool::id_of(optimized.fixed[0]))
                .pretty_version,
            "8.3.0"
        );
    }

    /// #116: a pruned identical-dependency duplicate shifts every later
    /// index, so a surviving alias's `alias_of` (still pointing at its
    /// *pre*-pruning index) must be rewritten through the same remap
    /// `request.fixed` gets — otherwise `rule_set_generator` indexes the
    /// pruned pool with a stale index and panics.
    #[test]
    fn remaps_alias_of_after_pruning() {
        // Five identical-dependency duplicates ahead of `base` in the pool
        // (indices 0..=4) so `base`'s pre-pruning index (5) lands *past* the
        // pruned pool's length (3) once they're collapsed away — the same
        // shape as #116's real phpunit/phpunit panic (pool shrunk from 143
        // to 89, a stale `alias_of` of 118 indexed out of bounds).
        let mut packages = Vec::new();
        for patch in 0..5 {
            let mut dup = package("vendor/dep", &format!("1.0.{patch}"));
            dup.requires = vec![link_c("vendor/leaf", "^1.0")];
            packages.push(dup);
        }
        let mut base = package("vendor/dep", "1.0.5");
        base.requires = vec![link_c("vendor/leaf", "^1.0")];
        packages.push(base); // index 5, the pre-pruning `alias_of` target.
        let mut alias = package("vendor/dep", "1.0.5");
        alias.requires = vec![link_c("vendor/leaf", "^1.0")];
        alias.alias_of = Some(5);
        packages.push(alias); // index 6.
        packages.push(package("vendor/leaf", "1.0.0")); // index 7.
        let pool = Pool::new(packages);
        let request = require("vendor/dep", "^1.0");
        let policy = DefaultPolicy::new(false, false);

        let optimized = optimize(&request, pool, &policy, &mut ConstraintCache::new()).unwrap();

        assert_eq!(
            optimized.pool.packages().len(),
            3,
            "the five identical-dependency duplicates must still be pruned"
        );
        let alias_package = optimized
            .pool
            .packages()
            .iter()
            .find(|p| p.is_alias())
            .expect("alias must survive alongside its base");
        let base_index = alias_package
            .alias_of
            .expect("alias_of must still be set after remap");
        assert_eq!(
            optimized.pool.packages()[base_index].pretty_version,
            "1.0.5",
            "alias_of must point at the base's *post*-pruning index, not its stale pre-pruning one"
        );

        // The stale index would have panicked `rule_set_generator` (#116)
        // exactly the way it did over the real phpunit/phpunit pool.
        let rules = rule_set_generator::rules_for(&optimized.pool, &request);
        assert!(!rules.is_empty());
    }

    /// `optimizeImpossiblePackagesAway`: a locked package's own require
    /// rules out every candidate version that could never satisfy it,
    /// exercised directly since `Request` has no locked-package field yet
    /// (the module doc explains why `optimize` never reaches this with a
    /// non-empty `locked`).
    #[test]
    fn impossible_packages_away_removes_versions_no_locked_require_can_pick() {
        let mut root = package("vendor/root", "1.0.0");
        root.requires = vec![link_c("vendor/dep", "1.0.0")];
        let dep_match = package("vendor/dep", "1.0.0");
        let dep_impossible = package("vendor/dep", "2.0.0");
        let packages = vec![root, dep_match, dep_impossible];

        let mut require_constraints: ConstraintGroups = HashMap::new();
        let mut cache = ConstraintCache::new();
        // Root itself must be "used" (referenced by some requirement) for
        // its own requires to apply at all (`isUnusedPackage`'s guard).
        add_disjuncts(&mut require_constraints, "vendor/root", "*", &mut cache).unwrap();
        add_disjuncts(&mut require_constraints, "vendor/dep", "1.0.0", &mut cache).unwrap();

        let mut to_remove = HashSet::new();
        optimize_impossible_packages_away(
            &packages,
            &HashSet::new(),
            &HashMap::new(),
            &HashSet::new(),
            &[0],
            &require_constraints,
            &mut to_remove,
        );

        assert_eq!(to_remove, HashSet::from([2]), "{to_remove:?}");
    }
}
