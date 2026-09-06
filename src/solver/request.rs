//! Port of `DependencyResolver/Request.php`, cut down to what a full update
//! needs: root requires and fixed (platform) packages. No locked/lockable
//! packages, update allow-list or restricted-packages support: those exist
//! for partial updates and `install`'s lock-verification pass, both out of
//! scope (`docs/resolver-design.md` stage 3 is "full update only").

use crate::semver::Constraint;

/// `Request::requireName` pairs, in the order `PoolBuilder` should see them
/// (root `require` then `require-dev`, `Installer::doUpdate`'s merge for the
/// first solve).
pub struct Request {
    pub requires: Vec<(String, Option<Constraint>)>,
    /// Pool indices of packages `Request::fixPackage` marked irremovable:
    /// the platform packages here (`Installer.php:1022-1035`).
    pub fixed: Vec<usize>,
}
