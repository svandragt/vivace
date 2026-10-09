//! Port of `DependencyResolver/Request.php`, cut down to what a full update
//! needs: root requires and fixed (platform) packages, plus the lock-file
//! versions a partial update holds (for `problem.rs`'s messages only). No
//! lockable-package or restricted-packages support: those exist for
//! `install`'s lock-verification pass, out of scope (`docs/resolver-design.md`
//! stage 3 is "full update only").

use std::collections::HashMap;

use crate::semver::{Constraint, NormalizedVersion};

/// One `Request::requireName` call: `pretty_constraint` is the constraint
/// exactly as written in `composer.json` (`"*"` for a root require with no
/// constraint at all, matching `MatchAllConstraint::getPrettyString()`),
/// kept alongside the parsed `Constraint` so `Rule::RULE_ROOT_REQUIRE`'s
/// reason (`rule_set_generator.rs`) and the root-require-problem check
/// (`solver.rs`) can report Composer's own constraint text instead of the
/// parsed form's internal `Display` (`^1.0`, not `[>= 1.0.0.0-dev <
/// 2.0.0.0-dev]`).
pub struct RootRequire {
    pub name: String,
    pub constraint: Option<Constraint>,
    pub pretty_constraint: String,
}

/// Root requires, in the order `PoolBuilder` should see them (root
/// `require` then `require-dev`, `Installer::doUpdate`'s merge for the
/// first solve).
#[derive(Default)]
pub struct Request {
    pub requires: Vec<RootRequire>,
    /// Pool indices of packages `Request::fixPackage` marked irremovable:
    /// the platform packages here (`Installer.php:1022-1035`).
    pub fixed: Vec<usize>,
    /// Packages a partial update holds at their lock-file version
    /// (`Request::lockPackage`), keyed by name. Not pool indices: the pool
    /// optimizer drops a held package no remaining require matches, which
    /// is exactly the case `problem.rs` needs to explain.
    pub locked: HashMap<String, LockedVersion>,
    /// `Request::getUpdateAllowTransitiveRootDependencies` (`-W`).
    pub allow_transitive_root_dependencies: bool,
}

/// A [`Request::locked`] entry's lock-file version.
pub struct LockedVersion {
    pub version: NormalizedVersion,
    pub pretty_version: String,
}
