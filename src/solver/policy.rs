//! Port of `DependencyResolver/DefaultPolicy.php`, exactly (per
//! `docs/resolver-design.md`): alias over aliased, replaced over replacer,
//! same-vendor replacer, then pool insertion order.
//!
//! No `COMPOSER_PREFER_DEV_OVER_PRERELEASE` env toggle (undocumented
//! upstream escape hatch, not exercised by any fixture): default off
//! exactly as `DefaultPolicy`'s constructor defaults it, so skipping it
//! changes nothing this stage's tests observe.
//!
//! `preferredVersions` (`--minimal-changes`'s pin toward each already-locked
//! package's exact version, `Installer::createPolicy`) is ported as
//! [`DefaultPolicy::with_preferred_versions`] and wired into
//! `update.rs::solve` via `solve_update_seeded`/`solve_partial_update_seeded`'s
//! `preferred` parameter: `Installer::createPolicy`'s own
//! `$preferredVersions[$pkg->getName()] = $pkg->getVersion();` loop, minus
//! `AliasPackage`s (not lock entries in `packages`/`packages-dev`, so never
//! built here) and minus the literal `--minimal-changes` allow list (the
//! packages the user asked to move, which must stay free to).
//!
//! `pool_builder::build_partial_seeded` builds its own `DefaultPolicy` the
//! same way, for `pool_optimizer::optimize`: real Composer passes the
//! *same* `$policy` (preferred-versions-aware) into both `Solver` and
//! `createPoolOptimizer` (`Installer.php:534`), because
//! `PoolOptimizer::optimize` itself calls
//! `$this->policy->selectPreferredPackages(...)` when collapsing same-name
//! duplicates (`PoolOptimizer.php:247`) — an optimizer built from a
//! preferred-versions-blind policy would discard the pinned version as a
//! "duplicate" before the solver ever saw it.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::semver::{self, NormalizedVersion, VersionKey};
use crate::solver::pool::Pool;

/// `Constraint::STR_OP_*`, the subset `versionCompare` actually takes.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Gt,
    Lt,
    Eq,
}

pub struct DefaultPolicy {
    prefer_stable: bool,
    prefer_lowest: bool,
    /// `--minimal-changes`: `name -> normalized version` for every
    /// already-locked, non-allow-listed package (`Installer::createPolicy`'s
    /// `$preferredVersions`). `None` (the common case, [`DefaultPolicy::new`])
    /// skips the pin check in `prune_to_best_version` entirely, same as
    /// PHP's own `$this->preferredVersions !== null` guard.
    preferred_versions: Option<HashMap<String, NormalizedVersion>>,
}

impl DefaultPolicy {
    pub fn new(prefer_stable: bool, prefer_lowest: bool) -> DefaultPolicy {
        DefaultPolicy {
            prefer_stable,
            prefer_lowest,
            preferred_versions: None,
        }
    }

    /// `--minimal-changes`'s policy: see the module doc for the pin set's
    /// exact composition.
    pub fn with_preferred_versions(
        prefer_stable: bool,
        prefer_lowest: bool,
        preferred_versions: HashMap<String, NormalizedVersion>,
    ) -> DefaultPolicy {
        DefaultPolicy {
            prefer_stable,
            prefer_lowest,
            preferred_versions: Some(preferred_versions),
        }
    }

    /// `DefaultPolicy::versionCompare`. The dev-branch-via-`matchSpecific`
    /// path and the `CompilingMatcher` path both collapse into one
    /// `VersionKey::cmp` call: that impl's doc comment (see `src/semver.rs`)
    /// is written to be exactly this method's fallback comparator — same
    /// `Ordering`, computed from `a`/`b`'s already-parsed keys instead of
    /// `crate::semver::compare`'s own per-call re-normalisation
    /// (`bench/results/profile.md` §2.7: this was the next hotspot after
    /// `CompiledConstraint`). Its one documented gap (two *unrelated* dev
    /// branches, neither a numeric alias nor `dev-master`-style, fold to
    /// `Equal` rather than "incomparable") would only bite here if a single
    /// require resolved to two different arbitrary feature branches of the
    /// same package — not exercised by any fixture in this stage.
    pub fn version_compare(&self, a: &PackageRef, b: &PackageRef, operator: Op) -> bool {
        if self.prefer_stable && a.stability != b.stability {
            // `COMPOSER_PREFER_DEV_OVER_PRERELEASE` would additionally fold
            // "dev" to "stable" here when `prefer_lowest` is also set (see
            // module doc for why it's skipped: always false in vivace).
            return stability_rank(a.stability) < stability_rank(b.stability);
        }

        let ordering = a.key.cmp(&b.key);
        match operator {
            Op::Gt => ordering == Ordering::Greater,
            Op::Lt => ordering == Ordering::Less,
            Op::Eq => ordering == Ordering::Equal,
        }
    }

    /// `DefaultPolicy::selectPreferredPackages`, minus its two per-pool
    /// memoisation caches (`preferredPackageResultCachePerPool`,
    /// `sortingCachePerPool`): pure speed, and this stage's pools are far
    /// too small for the hashing overhead to pay for itself.
    pub fn select_preferred_packages(
        &self,
        pool: &Pool,
        literals: &[i32],
        required_package: Option<&str>,
    ) -> Vec<i32> {
        let mut literals = literals.to_vec();
        literals.sort_unstable();

        let mut groups = group_literals_by_name(pool, &literals);

        for (_, group) in &mut groups {
            group.sort_by(|&a, &b| Self::compare_by_priority(pool, a, b, required_package, true));
        }

        for (_, group) in &mut groups {
            *group = self.prune_to_best_version(pool, group);
            *group = prune_remote_aliases(pool, group);
        }

        let mut selected: Vec<i32> = groups.into_iter().flat_map(|(_, g)| g).collect();
        selected.sort_by(|&a, &b| Self::compare_by_priority(pool, a, b, required_package, false));
        selected
    }

    /// `DefaultPolicy::compareByPriority`. Takes literals (package ids)
    /// rather than package references, since the tie-break needs each
    /// side's pool id and `Package` does not carry its own (see
    /// `pool.rs`'s module doc: id is pool position, not a struct field).
    /// An associated function, not a method: nothing here reads
    /// `prefer_stable`/`prefer_lowest` (Composer's own `compareByPriority`
    /// does not touch `$this` either, beyond being a method for
    /// visibility).
    fn compare_by_priority(
        pool: &Pool,
        a: i32,
        b: i32,
        required_package: Option<&str>,
        ignore_replace: bool,
    ) -> Ordering {
        let pa = pool.literal_to_package(a);
        let pb = pool.literal_to_package(b);

        if pa.name == pb.name {
            match (pa.is_alias(), pb.is_alias()) {
                (true, false) => return Ordering::Less,
                (false, true) => return Ordering::Greater,
                _ => {}
            }
        }

        if !ignore_replace {
            if replaces(pool, a, b) {
                return Ordering::Greater;
            }
            if replaces(pool, b, a) {
                return Ordering::Less;
            }

            if let Some(required) = required_package
                && let Some(slash) = required.find('/')
            {
                let vendor = &required[..slash];
                let a_same_vendor = pa.name.starts_with(vendor);
                let b_same_vendor = pb.name.starts_with(vendor);
                if a_same_vendor != b_same_vendor {
                    return if a_same_vendor {
                        Ordering::Less
                    } else {
                        Ordering::Greater
                    };
                }
            }
        }

        a.cmp(&b)
    }

    /// `DefaultPolicy::pruneToBestVersion`.
    fn prune_to_best_version(&self, pool: &Pool, literals: &[i32]) -> Vec<i32> {
        if let Some(preferred) = &self.preferred_versions {
            let name = &pool.literal_to_package(literals[0]).name;
            if let Some(preferred_version) = preferred.get(name) {
                let pinned: Vec<i32> = literals
                    .iter()
                    .copied()
                    .filter(|&l| pool.literal_to_package(l).version == *preferred_version)
                    .collect();
                if !pinned.is_empty() {
                    return pinned;
                }
            }
        }

        let operator = if self.prefer_lowest { Op::Lt } else { Op::Gt };
        let mut best_literals = vec![literals[0]];
        let mut best = package_ref(pool, literals[0]);
        for &literal in &literals[1..] {
            let candidate = package_ref(pool, literal);
            if self.version_compare(&candidate, &best, operator) {
                best = candidate;
                best_literals = vec![literal];
            } else if self.version_compare(&candidate, &best, Op::Eq) {
                best_literals.push(literal);
            }
        }
        best_literals
    }
}

/// Just enough of a `Package` for `version_compare` to read, so it does not
/// need to borrow the `Pool` for the comparison's lifetime. `key` is parsed
/// once here rather than once per comparison (`version_compare` used to
/// call `crate::semver::compare` on `version` directly, re-normalising both
/// sides on every call it made).
pub struct PackageRef {
    pub key: VersionKey,
    pub stability: &'static str,
}

fn package_ref(pool: &Pool, literal: i32) -> PackageRef {
    let p = pool.literal_to_package(literal);
    PackageRef {
        key: semver::parse_version_key(&p.version),
        stability: p.stability,
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

/// `DefaultPolicy::replaces`: does `source` replace a package with `target`'s
/// name? Constraints are ignored, matching the PHP comment: this is for
/// prioritisation only, not constraint verification.
fn replaces(pool: &Pool, source: i32, target: i32) -> bool {
    let source = pool.literal_to_package(source);
    let target_name = &pool.literal_to_package(target).name;
    source
        .replaces
        .iter()
        .any(|link| &link.target == target_name)
}

/// `DefaultPolicy::groupLiteralsByName`, preserving first-seen order like
/// PHP's associative array.
fn group_literals_by_name(pool: &Pool, literals: &[i32]) -> Vec<(String, Vec<i32>)> {
    let mut groups: Vec<(String, Vec<i32>)> = Vec::new();
    for &literal in literals {
        let name = &pool.literal_to_package(literal).name;
        match groups.iter_mut().find(|(n, _)| n == name) {
            Some((_, group)) => group.push(literal),
            None => groups.push((name.clone(), vec![literal])),
        }
    }
    groups
}

/// `DefaultPolicy::pruneRemoteAliases`: once a locally (root-)aliased
/// package is among the candidates, only local aliases remain eligible.
fn prune_remote_aliases(pool: &Pool, literals: &[i32]) -> Vec<i32> {
    let has_local_alias = literals.iter().any(|&l| {
        let p = pool.literal_to_package(l);
        p.is_alias() && p.is_root_package_alias
    });
    if !has_local_alias {
        return literals.to_vec();
    }
    literals
        .iter()
        .copied()
        .filter(|&l| {
            let p = pool.literal_to_package(l);
            p.is_alias() && p.is_root_package_alias
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solver::pool::Package;

    fn package(name: &str, pretty_version: &str) -> Package {
        let normalized = crate::semver::normalize(pretty_version).unwrap();
        Package {
            stability: crate::semver::stability(normalized.as_str()),
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
            raw: std::sync::Arc::new(serde_json::json!({})),
        }
    }

    /// `--minimal-changes`'s pin: among several candidates for the same
    /// name, the one matching the preferred (locked) version wins even
    /// though a higher version is otherwise available.
    #[test]
    fn preferred_versions_pins_the_locked_version() {
        let low = package("vendor/pkg", "1.0.0");
        let high = package("vendor/pkg", "2.0.0");
        let pool = Pool::new(vec![low, high]);
        let literals = vec![1, 2];

        let mut preferred = HashMap::new();
        preferred.insert(
            "vendor/pkg".to_string(),
            crate::semver::normalize("1.0.0").unwrap(),
        );
        let policy = DefaultPolicy::with_preferred_versions(false, false, preferred);
        let selected = policy.select_preferred_packages(&pool, &literals, None);
        assert_eq!(selected, vec![1]);

        // Without the pin, the higher version wins as usual.
        let unpinned = DefaultPolicy::new(false, false);
        let selected = unpinned.select_preferred_packages(&pool, &literals, None);
        assert_eq!(selected, vec![2]);
    }

    /// A pin for a name with no matching candidate version falls back to
    /// the normal stable/lowest comparison (`pruneToBestVersion`'s own
    /// `if (\count($bestLiterals) > 0)` guard).
    #[test]
    fn preferred_versions_falls_back_when_the_pin_has_no_candidate() {
        let low = package("vendor/pkg", "1.0.0");
        let high = package("vendor/pkg", "2.0.0");
        let pool = Pool::new(vec![low, high]);
        let literals = vec![1, 2];

        let mut preferred = HashMap::new();
        preferred.insert(
            "vendor/pkg".to_string(),
            crate::semver::normalize("3.0.0").unwrap(),
        );
        let policy = DefaultPolicy::with_preferred_versions(false, false, preferred);
        let selected = policy.select_preferred_packages(&pool, &literals, None);
        assert_eq!(selected, vec![2]);
    }
}
