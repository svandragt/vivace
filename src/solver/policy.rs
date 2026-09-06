//! Port of `DependencyResolver/DefaultPolicy.php`, exactly (per
//! `docs/resolver-design.md`): alias over aliased, replaced over replacer,
//! same-vendor replacer, then pool insertion order.
//!
//! No `preferredVersions` (`viv require`'s version pinning, out of scope:
//! this stage does no CLI wiring) and no `COMPOSER_PREFER_DEV_OVER_PRERELEASE`
//! env toggle (undocumented upstream escape hatch, not exercised by any
//! fixture); both default off exactly as `DefaultPolicy`'s constructor
//! defaults them, so skipping them changes nothing this stage's tests
//! observe.

use std::cmp::Ordering;

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
}

impl DefaultPolicy {
    pub fn new(prefer_stable: bool, prefer_lowest: bool) -> DefaultPolicy {
        DefaultPolicy {
            prefer_stable,
            prefer_lowest,
        }
    }

    /// `DefaultPolicy::versionCompare`. The dev-branch-via-`matchSpecific`
    /// path and the `CompilingMatcher` path both collapse into one
    /// `semver::compare` call: that facade function's doc comment (see
    /// `src/semver.rs`) is written to be exactly this method's fallback
    /// comparator. Its one documented gap (two *unrelated* dev branches,
    /// neither a numeric alias nor `dev-master`-style, fold to `Equal`
    /// rather than "incomparable") would only bite here if a single
    /// require resolved to two different arbitrary feature branches of the
    /// same package — not exercised by any fixture in this stage.
    pub fn version_compare(&self, a: &PackageRef, b: &PackageRef, operator: Op) -> bool {
        if self.prefer_stable && a.stability != b.stability {
            // `COMPOSER_PREFER_DEV_OVER_PRERELEASE` would additionally fold
            // "dev" to "stable" here when `prefer_lowest` is also set (see
            // module doc for why it's skipped: always false in vivace).
            return stability_rank(a.stability) < stability_rank(b.stability);
        }

        let ordering = crate::semver::compare(&a.version, &b.version);
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

    /// `DefaultPolicy::pruneToBestVersion`, minus `preferredVersions` (see
    /// module doc).
    fn prune_to_best_version(&self, pool: &Pool, literals: &[i32]) -> Vec<i32> {
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
/// need to borrow the `Pool` for the comparison's lifetime.
pub struct PackageRef {
    pub version: crate::semver::NormalizedVersion,
    pub stability: &'static str,
}

fn package_ref(pool: &Pool, literal: i32) -> PackageRef {
    let p = pool.literal_to_package(literal);
    PackageRef {
        version: p.version.clone(),
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
