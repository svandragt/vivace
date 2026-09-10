//! Port of `DependencyResolver/Pool.php` plus the parts of `Package.php`,
//! `CompletePackage.php` and `AliasPackage.php` the solver actually reads.
//!
//! Composer models an alias as a `BasePackage` subclass wrapping the aliased
//! package (`AliasPackage::$aliasOf`), with both objects living in the pool
//! (`ComposerRepository.php:1350-1353` adds the alias *and* its `aliasOf` as
//! separate pool entries). Rust has no subclassing, so [`Package`] collapses
//! both into one struct: an alias sets `alias_of` to the aliased package's
//! index in [`Pool::packages`], added immediately after it (mirroring the
//! insertion order above), rather than wrapping a nested object.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::Value;

use crate::semver::{self, Constraint, NormalizedVersion};

/// `Package\Link`: a require/conflict/provide/replace edge. Only the fields
/// the solver and rule generator read; no getSource/getDescription, since
/// those exist for `Rule::getPrettyString` (Problem.php, stage 5).
///
/// `constraint` is `Arc`-shared rather than owned outright: the same
/// constraint text (`"php": "^7.2.5 || ^8.0.0"`) recurs across thousands of
/// package versions, and `Constraint` itself isn't `Clone` (it wraps a
/// `Box<dyn semver_php::Constraint>`), so sharing one parse is the only way
/// to cache it at all (`bench/results/profile.md` §2.8,
/// `solver::ConstraintCache`).
#[derive(Clone)]
pub struct Link {
    pub target: String,
    pub constraint: Option<Arc<Constraint>>,
    /// The constraint exactly as written in `composer.json`/the provider
    /// file, `Link::getPrettyConstraint()`; `None` prints as `"*"`
    /// (`MatchAllConstraint::getPrettyString()`).
    pub pretty_constraint: Option<String>,
}

impl Link {
    pub fn pretty_constraint(&self) -> &str {
        self.pretty_constraint.as_deref().unwrap_or("*")
    }
}

/// Collapses `Package`/`CompletePackage`/`AliasPackage` (see the module
/// doc). `version`/`stability`/`is_dev` always describe *this* object: for
/// an alias that is the alias's own version, matching
/// `AliasPackage::getVersion`/`getStability`/`isDev`, not the aliased
/// package's.
pub struct Package {
    pub name: String,
    pub version: NormalizedVersion,
    pub pretty_version: String,
    pub stability: &'static str,
    pub is_dev: bool,
    pub requires: Vec<Link>,
    pub conflicts: Vec<Link>,
    pub provides: Vec<Link>,
    pub replaces: Vec<Link>,
    /// `Some(i)` if this is an `AliasPackage`, `i` its `getAliasOf()`'s pool
    /// index (`RuleSetGenerator::addRulesForPackage`'s `AliasPackage` case).
    pub alias_of: Option<usize>,
    /// `AliasPackage::isRootPackageAlias`: a root `composer.json` require's
    /// `X as Y`, as opposed to a branch-alias (`extra.branch-alias`).
    pub is_root_package_alias: bool,
    /// `AliasPackage::hasSelfVersionRequires`.
    pub has_self_version_requires: bool,
    /// The provider-file version entry this package came from, untouched
    /// (`Repository::PackageVersion::raw`): stage 4's lock writer re-emits
    /// its fields in `ArrayDumper` order rather than the pool/solver reading
    /// them again. Platform packages carry a synthetic stand-in (never
    /// reached: `transaction::resolved_packages` drops fixed packages
    /// before a lock ever sees them).
    ///
    /// `Arc`, not `Value`: `push_package_version` pushes a branch-alias or
    /// root-alias version as two or three `Package`s sharing the same raw
    /// entry, and `clone_package`'s dev-split second solve clones every
    /// first-solve package again — a deep JSON clone each time would be one
    /// of the largest per-package costs in `pool_builder::build` (`bench/
    /// results/profile.md`'s "string cloning in `PackageVersion` -> `Package`
    /// conversion" candidate), where an `Arc::clone` is a pointer copy.
    pub raw: Arc<Value>,
}

impl Package {
    pub fn is_alias(&self) -> bool {
        self.alias_of.is_some()
    }

    /// `BasePackage::getNames($provides)`: this package's own name plus,
    /// when `provides` is true, every `provide` target, plus every
    /// `replace` target.
    pub fn names(&self, provides: bool) -> Vec<String> {
        let mut names = vec![self.name.clone()];
        if provides {
            for link in &self.provides {
                if !names.contains(&link.target) {
                    names.push(link.target.clone());
                }
            }
        }
        for link in &self.replaces {
            if !names.contains(&link.target) {
                names.push(link.target.clone());
            }
        }
        names
    }

    /// `BasePackage::getPrettyString` (no repository name prefix: vivace
    /// has one repository at this stage).
    pub fn pretty_string(&self) -> String {
        format!("{} {}", self.name, self.pretty_version)
    }
}

/// `DependencyResolver/Pool.php`. No `unacceptableFixedOrLockedPackages`,
/// `removedVersions*` or security/abandoned/filter-list bookkeeping: those
/// back `Problem.php`'s verbose messages (stage 5) and the security
/// advisory/filter-list pool filters, neither in scope for this stage.
pub struct Pool {
    packages: Vec<Package>,
    /// Every name a package can be looked up under (its own name, plus
    /// every `provide`/`replace` target) -> the ids of packages that could
    /// match it, in insertion order. `bench/results/profile.md` §8: on
    /// bench/laravel's 647-package post-optimizer pool, `what_provides`'s
    /// old full scan-and-filter (below) was 94% of rule generation's own
    /// time (7,396 calls, 60 ms of 65 ms) — the candidate set for a given
    /// name is almost always a small fraction of the pool, so building this
    /// index once (`O(pool size)`) and scanning only its candidates per
    /// call is the same result for far less work.
    by_name: HashMap<String, Vec<i32>>,
}

impl Pool {
    /// `packages` in insertion order; ids are 1-based positions
    /// (`Pool::setPackages`), so `id = index + 1`.
    pub fn new(packages: Vec<Package>) -> Self {
        let mut by_name: HashMap<String, Vec<i32>> = HashMap::new();
        for (index, package) in packages.iter().enumerate() {
            let id = id_of(index);
            by_name.entry(package.name.clone()).or_default().push(id);
            for link in package.provides.iter().chain(package.replaces.iter()) {
                let ids = by_name.entry(link.target.clone()).or_default();
                if ids.last() != Some(&id) {
                    ids.push(id);
                }
            }
        }
        Pool { packages, by_name }
    }

    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// Consumes the pool, returning its packages in insertion order.
    /// `pool_optimizer::optimize` uses this to filter the vec and rebuild a
    /// pruned [`Pool`] via [`Pool::new`], which reassigns fresh 1-based ids
    /// (`Pool::setPackages`'s own re-numbering in
    /// `PoolOptimizer::applyRemovalsToPool`).
    pub fn into_packages(self) -> Vec<Package> {
        self.packages
    }

    pub fn len(&self) -> usize {
        self.packages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.packages.is_empty()
    }

    /// `Pool::packageById`. 1-based like Composer's package ids.
    pub fn package_by_id(&self, id: i32) -> &Package {
        &self.packages[index_of(id)]
    }

    /// A literal's absolute value is a package id (`Pool::literalToPackage`).
    pub fn literal_to_package(&self, literal: i32) -> &Package {
        self.package_by_id(literal.abs())
    }

    /// `Pool::whatProvides`, without the `providerCache` memoisation
    /// (Composer's own cache is keyed on `(name, constraint)` and would
    /// still redo the constraint match on a miss): `by_name` already
    /// narrows the scan to the packages that can possibly match `name`, so
    /// caching by constraint on top would only save re-checking a handful
    /// of candidates against a constraint this call already has in hand.
    pub fn what_provides(&self, name: &str, constraint: Option<&Constraint>) -> Vec<i32> {
        let Some(candidates) = self.by_name.get(name) else {
            return Vec::new();
        };
        candidates
            .iter()
            .copied()
            .filter(|&id| package_matches(self.package_by_id(id), name, constraint))
            .collect()
    }
}

/// `Pool::match`: name equality checks the candidate's own version against
/// `constraint` as a single point (`CompilingMatcher::match` with
/// `OP_EQ`); a provide/replace target checks constraint-vs-constraint
/// overlap (`ConstraintInterface::matches`, i.e. interval intersection).
/// Skips the PHP source's `isset($replaces[0])` fast-path switch between
/// indexed and associative link storage: with no shortcut taken either
/// way, checking every provide/replace link directly is the same result
/// for a fraction of the code.
fn package_matches(candidate: &Package, name: &str, constraint: Option<&Constraint>) -> bool {
    if candidate.name == name {
        return match constraint {
            None => true,
            Some(c) => c.matches(&candidate.version),
        };
    }

    for link in candidate.provides.iter().chain(candidate.replaces.iter()) {
        if link.target != name {
            continue;
        }
        let ok = match (constraint, &link.constraint) {
            (None, _) | (Some(_), None) => true,
            // A provide/replace with no constraint of its own behaves like
            // Composer's `self.version` default: matches whatever is asked
            // (`MatchAllConstraint::matches` is always true).
            (Some(c), Some(link_c)) => semver::have_intersections(c, link_c),
        };
        if ok {
            return true;
        }
    }
    false
}

/// Pool ids are 1-based positions (`Pool::setPackages`'s `$id = 1;
/// ...$id++`); centralised here so the `usize`<->`i32` conversion happens
/// in one documented place. Ids fit `i32` comfortably: no pool this stage
/// builds gets anywhere near `i32::MAX` packages.
#[allow(clippy::cast_possible_wrap, clippy::cast_possible_truncation)]
pub fn id_of(index: usize) -> i32 {
    (index + 1) as i32
}

#[allow(clippy::cast_sign_loss)]
pub fn index_of(id: i32) -> usize {
    (id - 1) as usize
}
