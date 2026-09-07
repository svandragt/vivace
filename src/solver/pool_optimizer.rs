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
//!   case this crate exercises today. [`optimize_impossible_packages_away`]
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

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use crate::semver::{self, CompiledConstraint, Constraint, NormalizedVersion, VersionKey};
use crate::solver::policy::DefaultPolicy;
use crate::solver::pool::{self, Link, Package, Pool};
use crate::solver::request::Request;

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
    constraint: Constraint,
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

/// `PoolOptimizer::optimize`. `policy` is only used for
/// `selectPreferredPackages`'s tie-break within an identical-dependency
/// group: the same `prefer_stable`/`prefer_lowest` policy the solve itself
/// uses (no `--minimal-changes` pin here either — `policy.rs`'s own doc
/// comment explains why nothing wires that into `update.rs` yet).
pub fn optimize(request: &Request, pool: Pool, policy: &DefaultPolicy) -> Result<Optimized> {
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
        )?;
    }
    for package in pool.packages() {
        for link in &package.requires {
            add_disjuncts(
                &mut require_constraints,
                &link.target,
                link.pretty_constraint(),
            )?;
        }
        for link in &package.conflicts {
            add_disjuncts(
                &mut conflict_constraints,
                &link.target,
                link.pretty_constraint(),
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
fn add_disjuncts(groups: &mut ConstraintGroups, name: &str, pretty: &str) -> Result<()> {
    let entry = groups.entry(name.to_string()).or_default();
    for part in pretty.split("||").map(str::trim).filter(|p| !p.is_empty()) {
        if entry.iter().any(|require| require.text == part) {
            continue;
        }
        let constraint = semver::parse_constraint(part)?;
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
    // name -> group hash -> dependency hash -> package indices.
    let mut groups: HashMap<String, HashMap<String, HashMap<String, Vec<usize>>>> = HashMap::new();

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
        let replace_parts: Vec<String> = package
            .replaces
            .iter()
            .filter(|replace| {
                replace
                    .constraint
                    .as_ref()
                    .is_none_or(|c| c.matches(&package.version))
            })
            .map(|replace| format!("require:{}", replace.pretty_constraint()))
            .collect();

        for name in package.names(false) {
            let Some(requires) = require_constraints.get(&name) else {
                continue;
            };
            // Independent of which require disjunct is being checked below
            // (only of `name`), same reasoning as `replace_parts` above.
            let conflict_parts: Vec<String> = conflict_constraints
                .get(&name)
                .into_iter()
                .flatten()
                .filter(|conflict| matches(conflict, &key, &package.version))
                .map(|conflict| format!("conflict:{}", conflict.text))
                .collect();

            for require in requires {
                let mut parts = Vec::new();
                if matches(require, &key, &package.version) {
                    parts.push(format!("require:{}", require.text));
                }
                parts.extend(replace_parts.iter().cloned());
                parts.extend(conflict_parts.iter().cloned());
                if parts.is_empty() {
                    continue;
                }
                groups
                    .entry(name.clone())
                    .or_default()
                    .entry(parts.concat())
                    .or_default()
                    .entry(dep_hash.clone())
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

/// `PoolOptimizer::calculateDependencyHash`.
fn dependency_hash(package: &Package) -> String {
    let mut hash = String::new();
    for (key, links) in [
        ("requires", &package.requires),
        ("conflicts", &package.conflicts),
        ("replaces", &package.replaces),
        ("provides", &package.provides),
    ] {
        if links.is_empty() {
            continue;
        }
        hash.push_str(key);
        hash.push(':');
        let mut sub: Vec<(&str, String)> = links
            .iter()
            .map(|link| (link.target.as_str(), link_constraint_text(link)))
            .collect();
        sub.sort_unstable_by_key(|&(target, _)| target);
        for (target, constraint) in sub {
            hash.push_str(target);
            hash.push('@');
            hash.push_str(&constraint);
        }
    }
    hash
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

    fn version(v: &str) -> semver::NormalizedVersion {
        semver::normalize(v).unwrap()
    }

    fn link_c(target: &str, constraint: &str) -> Link {
        Link {
            target: target.to_string(),
            constraint: Some(semver::parse_constraint(constraint).unwrap()),
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
            raw: serde_json::json!({ "name": name, "version": pretty_version }),
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

        let optimized = optimize(&request, pool, &policy).unwrap();

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

        let optimized = optimize(&request, pool, &policy).unwrap();

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

        let optimized = optimize(&request, pool, &policy).unwrap();

        assert_eq!(optimized.fixed, vec![0]);
        assert_eq!(
            optimized
                .pool
                .package_by_id(pool::id_of(optimized.fixed[0]))
                .pretty_version,
            "8.3.0"
        );
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
        // Root itself must be "used" (referenced by some requirement) for
        // its own requires to apply at all (`isUnusedPackage`'s guard).
        add_disjuncts(&mut require_constraints, "vendor/root", "*").unwrap();
        add_disjuncts(&mut require_constraints, "vendor/dep", "1.0.0").unwrap();

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
