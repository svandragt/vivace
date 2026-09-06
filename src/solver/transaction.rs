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

use crate::solver::pool::{self, Pool};
use crate::solver::request::Request;

/// One resolved package, ready for a lock or a plain name/version
/// comparison.
#[derive(Debug)]
pub struct ResolvedPackage {
    pub name: String,
    pub pretty_version: String,
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
        })
        .collect()
}
