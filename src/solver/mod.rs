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

use anyhow::Result;
use serde_json::Value;

use crate::repository::{Repository, Transport};
use policy::DefaultPolicy;
use transaction::ResolvedPackage;

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
