//! A minimal `LockTransaction`/`Transaction` stand-in: this stage produces
//! a name -> version map for `viv update` to compare against a lock, not
//! `vendor/` operations, so `Transaction::calculateOperations`'s
//! install/update/remove diffing and plugin-ordering (both install-path
//! concerns another lane owns) are out of scope. `LockTransaction::setResultPackages`'s
//! part is the part that matters here: walk the solved decisions, keep the
//! positive (installed) ones, drop `AliasPackage` entries
//! (`getNewLockPackages` skips them, `LockTransaction.php:110-112`) and
//! fixed (platform) packages (`unlockableMap`, populated from
//! `Request::getFixedPackagesMap`: `getNewLockPackages` only ever reads
//! from the `non-dev`/`dev` buckets, which exclude anything in that map)
//! since neither is ever a `composer.lock` entry.

use std::collections::HashSet;

use serde_json::Value;

use crate::solver::pool::{self, Pool};
use crate::solver::request::Request;

/// One resolved package, ready for a lock or a plain name/version
/// comparison.
#[derive(Debug)]
pub struct ResolvedPackage {
    pub name: String,
    pub pretty_version: String,
    /// The pool package's own provider-file entry (`pool::Package::raw`),
    /// for the lock writer's `ArrayDumper`-order re-emission.
    pub raw: Value,
}

/// One used root alias (`LockTransaction::getAliases`'s per-entry shape):
/// `package` is the aliased name, `version` the real version being
/// aliased, `alias`/`alias_normalized` the alias's own pretty/normalized
/// version.
#[derive(Debug, Clone)]
pub struct AliasEntry {
    pub package: String,
    pub version: String,
    pub alias: String,
    pub alias_normalized: String,
}

/// `solver::solve`'s installed pool ids, turned into the non-alias,
/// non-fixed packages a lock file would list.
pub fn resolved_packages(
    pool: &Pool,
    installed: &[i32],
    request: &Request,
) -> Vec<ResolvedPackage> {
    let fixed: HashSet<i32> = request
        .fixed
        .iter()
        .map(|&index| pool::id_of(index))
        .collect();
    installed
        .iter()
        .filter(|id| !fixed.contains(id))
        .map(|&id| pool.package_by_id(id))
        .filter(|package| !package.is_alias())
        .map(|package| ResolvedPackage {
            name: package.name.clone(),
            pretty_version: package.pretty_version.clone(),
            raw: package.raw.clone(),
        })
        .collect()
}

/// `LockTransaction::getAliases`: every installed `AliasPackage` that came
/// from a root alias (`is_root_package_alias`, not a plain branch-alias),
/// sorted by `package` (`strcmp`). Reads directly off the solved decisions
/// rather than matching against a separate root-aliases list, since a root
/// alias `Package` already carries its own alias fields
/// (`push_package_version`'s root-alias branch).
pub fn used_aliases(pool: &Pool, installed: &[i32]) -> Vec<AliasEntry> {
    let mut aliases: Vec<AliasEntry> = installed
        .iter()
        .map(|&id| pool.package_by_id(id))
        .filter(|package| package.is_root_package_alias)
        .map(|package| AliasEntry {
            package: package.name.clone(),
            version: pool
                .package_by_id(pool::id_of(
                    package
                        .alias_of
                        .expect("root-alias package always has alias_of"),
                ))
                .version
                .as_str()
                .to_string(),
            alias: package.pretty_version.clone(),
            alias_normalized: package.version.as_str().to_string(),
        })
        .collect();
    aliases.sort_by(|a, b| a.package.cmp(&b.package));
    aliases
}
