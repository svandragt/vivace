//! Port of `DependencyResolver/RuleSetGenerator.php`.
//!
//! `addedMap`/`addedPackagesByNames` are PHP associative arrays, so their
//! iteration order (insertion order) feeds directly into the order rules get
//! appended to the `RuleSet`, which in turn is the order `Solver::runSat`
//! walks rules in. Reproducing Composer's exact tie-breaks means preserving
//! that order, not just the set of rules it produces, so both are kept as
//! `Vec`s here (paired with a `HashSet`/linear scan for membership) rather
//! than plain `HashMap`s.

use std::collections::{HashSet, VecDeque};

use crate::solver::pool::{self, Pool};
use crate::solver::request::Request;
use crate::solver::rules::{Reason, RuleKind, RuleSet, RuleType};

/// `RuleSetGenerator::getRulesFor`.
pub fn rules_for(pool: &Pool, request: &Request) -> RuleSet {
    let mut rules = RuleSet::new();
    let mut added_set: HashSet<i32> = HashSet::new();
    let mut added_order: Vec<i32> = Vec::new();
    let mut added_by_name: Vec<(String, Vec<i32>)> = Vec::new();

    add_rules_for_request(
        pool,
        request,
        &mut rules,
        &mut added_set,
        &mut added_order,
        &mut added_by_name,
    );
    add_rules_for_root_aliases(
        pool,
        &mut rules,
        &mut added_set,
        &mut added_order,
        &mut added_by_name,
    );
    add_conflict_rules(pool, &mut rules, &added_order, &added_by_name);

    rules
}

/// `RuleSetGenerator::addRulesForRequest`. No locked/fixed-but-unacceptable
/// handling: those exist for the lock-verification and filter-list paths
/// this stage does not build (see `request.rs`'s module doc).
fn add_rules_for_request(
    pool: &Pool,
    request: &Request,
    rules: &mut RuleSet,
    added_set: &mut HashSet<i32>,
    added_order: &mut Vec<i32>,
    added_by_name: &mut Vec<(String, Vec<i32>)>,
) {
    for &index in &request.fixed {
        let package_id = pool::id_of(index);
        add_rules_for_package(
            pool,
            package_id,
            rules,
            added_set,
            added_order,
            added_by_name,
        );
        create_install_one_of_rule(
            &[package_id],
            Reason::Fixed {
                package_index: index,
            },
            rules,
            RuleType::Request,
        );
    }

    for require in &request.requires {
        let packages = pool.what_provides(&require.name, require.constraint.as_ref());
        if packages.is_empty() {
            continue;
        }
        for &package_id in &packages {
            add_rules_for_package(
                pool,
                package_id,
                rules,
                added_set,
                added_order,
                added_by_name,
            );
        }
        create_install_one_of_rule(
            &packages,
            Reason::RootRequire {
                package_name: require.name.clone(),
                pretty_constraint: require.pretty_constraint.clone(),
            },
            rules,
            RuleType::Request,
        );
    }
}

/// `RuleSetGenerator::addRulesForRootAliases`.
fn add_rules_for_root_aliases(
    pool: &Pool,
    rules: &mut RuleSet,
    added_set: &mut HashSet<i32>,
    added_order: &mut Vec<i32>,
    added_by_name: &mut Vec<(String, Vec<i32>)>,
) {
    for (index, package) in pool.packages().iter().enumerate() {
        let package_id = pool::id_of(index);
        if added_set.contains(&package_id) {
            continue;
        }
        let Some(alias_of) = package.alias_of else {
            continue;
        };
        let alias_of_id = pool::id_of(alias_of);
        if package.is_root_package_alias || added_set.contains(&alias_of_id) {
            add_rules_for_package(
                pool,
                package_id,
                rules,
                added_set,
                added_order,
                added_by_name,
            );
        }
    }
}

/// `RuleSetGenerator::addRulesForPackage`, `SplQueue`-driven breadth-first
/// walk over a package and its requires (and, for an alias, the package it
/// aliases).
fn add_rules_for_package(
    pool: &Pool,
    start: i32,
    rules: &mut RuleSet,
    added_set: &mut HashSet<i32>,
    added_order: &mut Vec<i32>,
    added_by_name: &mut Vec<(String, Vec<i32>)>,
) {
    let mut queue: VecDeque<i32> = VecDeque::new();
    queue.push_back(start);

    while let Some(package_id) = queue.pop_front() {
        if !added_set.insert(package_id) {
            continue;
        }
        added_order.push(package_id);

        let package = pool.package_by_id(package_id);

        if let Some(alias_of) = package.alias_of {
            let alias_of_id = pool::id_of(alias_of);
            queue.push_back(alias_of_id);

            create_require_rule(
                package_id,
                &[alias_of_id],
                Reason::PackageAlias {
                    alias_index: alias_of,
                },
                rules,
            );
            create_require_rule(
                alias_of_id,
                &[package_id],
                Reason::PackageInverseAlias {
                    alias_of_index: alias_of,
                },
                rules,
            );

            // If the alias has no `self.version` requires, the aliased
            // package's own requires (processed when its turn comes off
            // the queue) already cover everything this alias needs.
            if !package.has_self_version_requires {
                continue;
            }
        } else {
            for name in package.names(false) {
                match added_by_name.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, ids)) => ids.push(package_id),
                    None => added_by_name.push((name, vec![package_id])),
                }
            }
        }

        for link in &package.requires {
            let providers = pool.what_provides(&link.target, link.constraint.as_ref());
            create_require_rule(
                package_id,
                &providers,
                Reason::PackageRequires {
                    source_index: pool::index_of(package_id),
                    target: link.target.clone(),
                    pretty_constraint: link.pretty_constraint().to_string(),
                },
                rules,
            );
            for &provider in &providers {
                queue.push_back(provider);
            }
        }
    }
}

/// `RuleSetGenerator::addConflictRules`.
fn add_conflict_rules(
    pool: &Pool,
    rules: &mut RuleSet,
    added_order: &[i32],
    added_by_name: &[(String, Vec<i32>)],
) {
    for &package_id in added_order {
        let package = pool.package_by_id(package_id);
        for link in &package.conflicts {
            if !added_by_name.iter().any(|(n, _)| n == &link.target) {
                continue;
            }
            let conflicts = pool.what_provides(&link.target, link.constraint.as_ref());
            for &conflict_id in &conflicts {
                let conflict = pool.package_by_id(conflict_id);
                // For an alias, only add the conflict when the name matches
                // exactly: otherwise the aliased package (queued alongside
                // it, see `add_rules_for_package`) already conflicts, and a
                // second rule against the alias itself would be redundant.
                if !conflict.is_alias() || conflict.name == link.target {
                    create_rule2_literals(
                        package_id,
                        conflict_id,
                        Reason::PackageConflict {
                            source_index: pool::index_of(package_id),
                            target_index: pool::index_of(conflict_id),
                            target: link.target.clone(),
                            pretty_constraint: link.pretty_constraint().to_string(),
                        },
                        rules,
                    );
                }
            }
        }
    }

    for (name, packages) in added_by_name {
        if packages.len() > 1 {
            create_multi_conflict_rule(packages, Reason::PackageSameName(name.clone()), rules);
        }
    }
}

/// `RuleSetGenerator::createRequireRule`: `(-package|provider1|provider2|...)`,
/// dropped entirely (not just the self literal) if the package is its own
/// provider (`RuleSetGenerator.php:65-68`'s "self fulfilling rule" check).
fn create_require_rule(package_id: i32, providers: &[i32], reason: Reason, rules: &mut RuleSet) {
    if providers.contains(&package_id) {
        return;
    }
    let mut literals = vec![-package_id];
    literals.extend_from_slice(providers);
    rules.add(literals, RuleKind::Normal, reason, RuleType::Package);
}

/// `RuleSetGenerator::createInstallOneOfRule`.
fn create_install_one_of_rule(
    packages: &[i32],
    reason: Reason,
    rules: &mut RuleSet,
    rule_type: RuleType,
) {
    rules.add(packages.to_vec(), RuleKind::Normal, reason, rule_type);
}

/// `RuleSetGenerator::createRule2Literals`: dropped if issuer and provider
/// are the same package (self-conflict).
fn create_rule2_literals(issuer: i32, provider: i32, reason: Reason, rules: &mut RuleSet) {
    if issuer == provider {
        return;
    }
    rules.add(
        vec![-issuer, -provider],
        RuleKind::Normal,
        reason,
        RuleType::Package,
    );
}

/// `RuleSetGenerator::createMultiConflictRule`: two packages collapse to a
/// plain two-literal rule (`Rule2Literals` in the PHP source), matching
/// `MultiConflictRule`'s own constructor guard of needing 3+ literals.
fn create_multi_conflict_rule(packages: &[i32], reason: Reason, rules: &mut RuleSet) {
    let literals: Vec<i32> = packages.iter().map(|p| -p).collect();
    let kind = if literals.len() == 2 {
        RuleKind::Normal
    } else {
        RuleKind::MultiConflictRule
    };
    rules.add(literals, kind, reason, RuleType::Package);
}
