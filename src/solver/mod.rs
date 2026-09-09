//! Port of Composer 2.10.2's dependency solver
//! (`src/Composer/DependencyResolver/*.php`), full update only: no lock
//! writing, no `viv update`/`viv add` CLI wiring (`docs/resolver-design.md`
//! stage 3). Not reachable from `src/install.rs`/`src/main.rs`: the solver
//! only runs on `update`, so the no-slower-than-Composer rule
//! (`AGENTS.md`'s Performance rule) does not apply to anything in this
//! module.
//!
//! | This module | Upstream |
//! |---|---|
//! | `pool.rs` | `Pool.php`, the parts of `Package.php`/`AliasPackage.php` the solver reads |
//! | `pool_builder.rs` | `PoolBuilder.php`, `Installer.php`'s request wiring, `RootPackageLoader.php`'s alias/stability extraction, `PlatformRepository.php` |
//! | `request.rs` | `Request.php` (full update's requires/fixed packages only) |
//! | `rules.rs` | `Rule.php`, `Rule2Literals.php`, `GenericRule.php`, `MultiConflictRule.php`, `RuleSet.php`, `RuleSetIterator.php` |
//! | `rule_set_generator.rs` | `RuleSetGenerator.php` |
//! | `watch_graph.rs` | `RuleWatchGraph.php`, `RuleWatchChain.php`, `RuleWatchNode.php` |
//! | `decisions.rs` | `Decisions.php` |
//! | `policy.rs` | `DefaultPolicy.php` |
//! | `solver.rs` | `Solver.php` |
//! | `transaction.rs` | `Transaction.php`/`LockTransaction.php` (result extraction only) |
//! | `problem.rs` | a blunt stand-in for `Problem.php`/`SolverProblemsException.php` (full port is stage 5, composer/composer#42) |
//! | `pool_optimizer.rs` | `PoolOptimizer.php`, run from `pool_builder::build`/`build_partial` between pool build and rule generation (#76) |

pub mod decisions;
pub(crate) mod platform;
pub mod policy;
pub mod pool;
pub mod pool_builder;
pub mod pool_optimizer;
pub mod problem;
pub mod request;
pub mod rule_set_generator;
pub mod rules;
// `solver::solver` mirrors `Solver.php`'s own name; the task's owned-file
// list names it `solver.rs` on purpose so a reader can find the port by
// searching for the upstream filename.
#[allow(clippy::module_inception)]
pub mod solver;
pub mod transaction;
pub mod watch_graph;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use serde_json::{Map, Value};

use crate::audit::AdvisoriesTransport;
use crate::repository::{Repository, Transport, branch_alias_target_from_raw};
use crate::semver::{self, Constraint, NormalizedVersion};
use policy::DefaultPolicy;
use pool::{Package, Pool};
use pool_builder::AdvisoryFilter;
use transaction::{AliasEntry, ResolvedPackage};

/// Distinct constraint text -> its parsed `Constraint`, shared for the
/// lifetime of one `pool_builder::build`/`build_partial` call between every
/// `Link` that names it and `pool_optimizer`'s own disjunct groups.
/// `bench/results/profile.md` §2.8: `push_package_version` re-parsing the
/// same handful of constraint strings (`"php": "^7.2.5 || ^8.0.0"`, written
/// near-identically by thousands of package versions) tens of thousands of
/// times over cost 863 ms uncached.
pub type ConstraintCache = HashMap<String, Arc<Constraint>>;

/// Parses `text` once per distinct string, cloning the shared `Arc` on
/// every repeat rather than re-running `semver::parse_constraint`.
pub fn parse_constraint_cached(cache: &mut ConstraintCache, text: &str) -> Result<Arc<Constraint>> {
    if let Some(constraint) = cache.get(text) {
        return Ok(Arc::clone(constraint));
    }
    let constraint = Arc::new(semver::parse_constraint(text)?);
    cache.insert(text.to_string(), Arc::clone(&constraint));
    Ok(constraint)
}

/// Builds a pool from `root`'s `require`/`require-dev` (merged, matching
/// `Installer::doUpdate`'s first solve) against everything `repo`'s
/// closure discovers, then solves it. Returns the resolved, non-alias
/// packages: `viv update`'s full-update result before the dev/non-dev
/// split (`docs/resolver-design.md`'s "The dev split is a second solve"),
/// which stage 4 will add.
pub async fn solve_full_update<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> Result<Vec<ResolvedPackage>> {
    let built = pool_builder::build(repo, root, prefer_stable, prefer_lowest).await?;
    let policy = DefaultPolicy::new(prefer_stable, prefer_lowest);
    let installed =
        solver::solve(&policy, &built.pool, &built.request).map_err(anyhow::Error::from)?;
    Ok(transaction::resolved_packages(
        &built.pool,
        &installed,
        &built.request,
    ))
}

/// `viv update`'s full result: the first solve's packages
/// (`require`+`require-dev` merged) split into `non_dev`/`dev` by a second,
/// `require`-only solve (`Installer::doUpdate`'s `extractDevPackages`), plus
/// everything `src/lock_writer.rs` needs to reproduce `Locker::setLockData`'s
/// other keys.
pub struct UpdateResult {
    pub non_dev: Vec<ResolvedPackage>,
    pub dev: Vec<ResolvedPackage>,
    pub aliases: Vec<AliasEntry>,
    /// The combined `prefer-stable`/`prefer-lowest` this solve actually ran
    /// with (`solve_update`'s caller already OR'd the CLI flag with
    /// `composer.json`'s own `prefer-stable`, `Installer::doUpdate`'s
    /// `$this->preferStable || $this->package->getPreferStable()`),
    /// returned so the lock writer doesn't need its own copy.
    pub prefer_stable: bool,
    pub prefer_lowest: bool,
    pub minimum_stability: &'static str,
    pub stability_flags: HashMap<String, u8>,
    pub platform_reqs: Map<String, Value>,
    pub platform_dev_reqs: Map<String, Value>,
    pub platform_overrides: Map<String, Value>,
}

/// `Installer::doUpdate`'s full pipeline: the merged first solve, then
/// `extractDevPackages`'s require-only second solve against a pool built
/// from nothing but the first solve's own result (`pool_builder::clone_package`
/// stands in for the dump/reload round-trip `$resultRepo` does in PHP).
/// Skips the second solve when `require-dev` is empty, matching
/// `extractDevPackages`'s own early return (every package stays `non_dev`).
pub async fn solve_update<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> Result<UpdateResult> {
    let built = pool_builder::build(repo, root, prefer_stable, prefer_lowest).await?;
    resolve(built, root, prefer_stable, prefer_lowest, HashMap::new())
}

/// Same as [`solve_update`], but `seed` (already-lowercased package names,
/// typically the prior `composer.lock`'s own) is passed straight through to
/// [`pool_builder::build_seeded`] (#90): a prefetch hint, never a pool
/// change. `preferred` is `--minimal-changes`'s pin set (see `policy.rs`'s
/// module doc); empty means an ordinary update.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API, only ever called with the default hasher"
)]
pub async fn solve_update_seeded<T: Transport, A: AdvisoriesTransport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
    seed: &[String],
    preferred: HashMap<String, NormalizedVersion>,
    advisories: Option<AdvisoryFilter<'_, A>>,
) -> Result<UpdateResult> {
    let built = pool_builder::build_seeded(
        repo,
        root,
        prefer_stable,
        prefer_lowest,
        seed,
        &preferred,
        advisories,
    )
    .await?;
    resolve(built, root, prefer_stable, prefer_lowest, preferred)
}

/// A partial update: `pkg...`'s allow list, expanded per `mode`
/// (`pool_builder::expand_allow_list`), then the same merged-solve/dev-split
/// pipeline as [`solve_update`] over `pool_builder::build_partial`'s pool.
/// `locked_by_name` is the current lock's `packages`+`packages-dev`,
/// lowercased-name-keyed.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API, only ever called with the default hasher"
)]
pub async fn solve_partial_update<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
    locked_by_name: &HashMap<String, Value>,
    allow_list: &[String],
    mode: pool_builder::UpdateAllowMode,
) -> Result<UpdateResult> {
    solve_partial_update_seeded::<T, crate::audit::NoAdvisories>(
        repo,
        root,
        prefer_stable,
        prefer_lowest,
        locked_by_name,
        allow_list,
        mode,
        &[],
        HashMap::new(),
        None,
    )
    .await
}

/// Same as [`solve_partial_update`], but `seed` is passed straight through
/// to [`pool_builder::build_partial_seeded`] (#90); see
/// [`solve_update_seeded`] for why a seed can never change the pool.
/// `preferred` is `--minimal-changes`'s pin set, same as
/// [`solve_update_seeded`]'s own.
#[expect(
    clippy::implicit_hasher,
    reason = "internal API, only ever called with the default hasher"
)]
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors solve_partial_update plus one seed slice, the minimal-changes pin set, \
              and the advisory pool filter"
)]
pub async fn solve_partial_update_seeded<T: Transport, A: AdvisoriesTransport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
    locked_by_name: &HashMap<String, Value>,
    allow_list: &[String],
    mode: pool_builder::UpdateAllowMode,
    seed: &[String],
    preferred: HashMap<String, NormalizedVersion>,
    advisories: Option<AdvisoryFilter<'_, A>>,
) -> Result<UpdateResult> {
    let locked_requires: HashMap<String, Vec<String>> = locked_by_name
        .iter()
        .map(|(name, entry)| {
            let requires = entry
                .get("require")
                .and_then(Value::as_object)
                .map(|m| m.keys().map(|k| k.to_ascii_lowercase()).collect())
                .unwrap_or_default();
            (name.clone(), requires)
        })
        .collect();
    let root_require_names: HashSet<String> = root
        .get("require")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|m| m.keys())
        .chain(
            root.get("require-dev")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|m| m.keys()),
        )
        .map(|k| k.to_ascii_lowercase())
        .collect();
    let allow_list: Vec<String> = allow_list.iter().map(|n| n.to_ascii_lowercase()).collect();
    let allow_names =
        pool_builder::expand_allow_list(&allow_list, &locked_requires, &root_require_names, mode);

    let built = pool_builder::build_partial_seeded(
        repo,
        root,
        locked_by_name,
        &allow_names,
        prefer_stable,
        prefer_lowest,
        seed,
        &preferred,
        advisories,
    )
    .await?;
    resolve(built, root, prefer_stable, prefer_lowest, preferred)
}

/// `pool_builder::push_package_version`'s alias-construction branch,
/// re-derived for a package already reduced to a pool [`Package`] rather
/// than a fresh `PackageVersion` (`resolve`'s dev-split second solve, which
/// has no `PackageVersion` for its cloned winners to read a branch alias
/// off — see the call site's doc comment for why this must exist at all).
fn branch_alias_package(
    base: &Package,
    base_index: usize,
    alias_normalized: &str,
) -> Result<Package> {
    Ok(Package {
        name: base.name.clone(),
        version: semver::normalize(alias_normalized)?,
        pretty_version: pretty_alias_version(alias_normalized),
        stability: semver::stability(alias_normalized),
        is_dev: true,
        requires: base.requires.clone(),
        conflicts: base.conflicts.clone(),
        provides: base.provides.clone(),
        replaces: base.replaces.clone(),
        alias_of: Some(base_index),
        is_root_package_alias: false,
        has_self_version_requires: false,
        raw: Arc::clone(&base.raw),
    })
}

/// `pool_builder`'s own private `pretty_alias`: the `9999999`-filled
/// numeric branch target (`2.0.9999999.9999999-dev`) collapsed back to its
/// `x`-form (`2.0.x-dev`) for display. Duplicated rather than exposed from
/// `pool_builder` (off limits: another agent is mid-fix there) — three
/// lines, and this alias never survives into user-facing output anyway
/// (`transaction::resolved_packages` drops every `is_alias()` package
/// before a lock or message ever sees one).
fn pretty_alias_version(normalized: &str) -> String {
    static NINES: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"(\.9{7})+").unwrap());
    NINES.replace(normalized, ".x").into_owned()
}

/// The merged-solve-then-dev-split pipeline shared by [`solve_update`] and
/// [`solve_partial_update`]: only how `built`'s pool/request came to be
/// differs between a full and a partial update.
fn resolve(
    built: pool_builder::BuildResult,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
    preferred: HashMap<String, NormalizedVersion>,
) -> Result<UpdateResult> {
    let policy = if preferred.is_empty() {
        DefaultPolicy::new(prefer_stable, prefer_lowest)
    } else {
        DefaultPolicy::with_preferred_versions(prefer_stable, prefer_lowest, preferred)
    };
    let installed =
        solver::solve(&policy, &built.pool, &built.request).map_err(anyhow::Error::from)?;
    let aliases = transaction::used_aliases(&built.pool, &installed);
    let first_solve = transaction::resolved_packages(&built.pool, &installed, &built.request);

    let require_dev_empty = root
        .get("require-dev")
        .and_then(Value::as_object)
        .is_none_or(Map::is_empty);

    // #159: the dev-split second solve as a whole (cloning the winners into
    // a second pool, re-solving, then partitioning `first_solve` by name) —
    // `solver::solve`'s own "solved pool" debug line already times the SAT
    // part alone, this wraps the surrounding pool-clone/partition work too.
    let dev_split_started = Instant::now();
    let (non_dev, dev) = if require_dev_empty {
        (first_solve, Vec::new())
    } else {
        let fixed_count = built.request.fixed.len();
        let fixed_ids: HashSet<i32> = built
            .request
            .fixed
            .iter()
            .map(|&index| pool::id_of(index))
            .collect();
        let mut second_packages = Vec::with_capacity(fixed_count + installed.len());
        for index in 0..fixed_count {
            second_packages.push(pool_builder::clone_package(
                built.pool.package_by_id(pool::id_of(index)),
            ));
        }
        for &id in &installed {
            if fixed_ids.contains(&id) {
                continue;
            }
            let package = built.pool.package_by_id(id);
            if package.is_alias() {
                continue;
            }
            let base_index = second_packages.len();
            second_packages.push(pool_builder::clone_package(package));
            // `clone_package`'s own doc says the second solve's repository
            // never carries `AliasPackage` *objects* — true, but Composer's
            // real `PoolBuilder` still re-derives a branch alias from every
            // loaded package's own `extra.branch-alias` metadata, dev-split
            // pool included; skipping that here left a `dev-*` winner whose
            // branch alias is the only pool entry actually satisfying a
            // numeric root require (#172: `dev-master`'s `2.0.x-dev` alias
            // satisfying `~2.0.54`) unrepresented in the second pool, so the
            // require-only solve failed where the first solve (and
            // Composer) succeeded.
            if let Some(alias_normalized) =
                branch_alias_target_from_raw(&package.pretty_version, &package.raw)
            {
                second_packages.push(branch_alias_package(
                    &second_packages[base_index],
                    base_index,
                    &alias_normalized,
                )?);
            }
        }
        let second_pool = Pool::new(second_packages);
        let second_request = pool_builder::require_only_request(root, fixed_count)?;
        let installed2 =
            solver::solve(&policy, &second_pool, &second_request).map_err(anyhow::Error::from)?;
        let non_dev = transaction::resolved_packages(&second_pool, &installed2, &second_request);
        let non_dev_names: HashSet<&str> = non_dev.iter().map(|p| p.name.as_str()).collect();
        let dev = first_solve
            .into_iter()
            .filter(|p| !non_dev_names.contains(p.name.as_str()))
            .collect();
        (non_dev, dev)
    };
    tracing::debug!(
        elapsed_ms = dev_split_started.elapsed().as_millis(),
        require_dev_empty,
        "dev-split second solve (no-op when require-dev is empty)"
    );

    Ok(UpdateResult {
        non_dev,
        dev,
        aliases,
        prefer_stable,
        prefer_lowest,
        minimum_stability: built.minimum_stability,
        stability_flags: built.stability_flags,
        platform_reqs: built.platform_reqs,
        platform_dev_reqs: built.platform_dev_reqs,
        platform_overrides: built.platform_overrides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `bench/results/profile.md` §2.8's fix: a repeated constraint string
    /// must reuse the exact same parsed `Constraint` (`Arc::ptr_eq`, not
    /// just an equivalent one — `Constraint` has no `PartialEq` to compare
    /// against), while two distinct strings never share an entry.
    #[test]
    fn caches_by_string_without_colliding_across_distinct_constraints() {
        let mut cache = ConstraintCache::new();

        let first = parse_constraint_cached(&mut cache, "^1.0").unwrap();
        let repeat = parse_constraint_cached(&mut cache, "^1.0").unwrap();
        assert!(
            Arc::ptr_eq(&first, &repeat),
            "a repeated constraint string must return the same cached Arc"
        );

        let other = parse_constraint_cached(&mut cache, "^2.0").unwrap();
        assert!(
            !Arc::ptr_eq(&first, &other),
            "distinct constraint strings must not collide"
        );
        assert_eq!(cache.len(), 2);
    }
}
