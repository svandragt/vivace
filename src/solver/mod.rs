//! Port of Composer 2.10.2's dependency solver
//! (`src/Composer/DependencyResolver/*.php`), full update only: no lock
//! writing, no `viv update`/`viv require` CLI wiring (`docs/resolver-design.md`
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
//!
//! Skipped outright: `PoolOptimizer.php` (pure speed, no semantic effect,
//! per the design doc).

pub mod decisions;
pub mod policy;
pub mod pool;
pub mod pool_builder;
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

use anyhow::Result;
use serde_json::{Map, Value};

use crate::repository::{Repository, Transport};
use policy::DefaultPolicy;
use pool::Pool;
use transaction::{AliasEntry, ResolvedPackage};

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
    let built = pool_builder::build(repo, root).await?;
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
    let built = pool_builder::build(repo, root).await?;
    resolve(built, root, prefer_stable, prefer_lowest)
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

    let built = pool_builder::build_partial(repo, root, locked_by_name, &allow_names).await?;
    resolve(built, root, prefer_stable, prefer_lowest)
}

/// The merged-solve-then-dev-split pipeline shared by [`solve_update`] and
/// [`solve_partial_update`]: only how `built`'s pool/request came to be
/// differs between a full and a partial update.
fn resolve(
    built: pool_builder::BuildResult,
    root: &Value,
    prefer_stable: bool,
    prefer_lowest: bool,
) -> Result<UpdateResult> {
    let policy = DefaultPolicy::new(prefer_stable, prefer_lowest);
    let installed =
        solver::solve(&policy, &built.pool, &built.request).map_err(anyhow::Error::from)?;
    let aliases = transaction::used_aliases(&built.pool, &installed);
    let first_solve = transaction::resolved_packages(&built.pool, &installed, &built.request);

    let require_dev_empty = root
        .get("require-dev")
        .and_then(Value::as_object)
        .is_none_or(Map::is_empty);

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
            )?);
        }
        for &id in &installed {
            if fixed_ids.contains(&id) {
                continue;
            }
            let package = built.pool.package_by_id(id);
            if package.is_alias() {
                continue;
            }
            second_packages.push(pool_builder::clone_package(package)?);
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
