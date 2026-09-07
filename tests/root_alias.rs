//! A root alias (`composer.json`'s `dev-master as 1.0.0`, recorded in the
//! lock's top-level `aliases` array) found by a compatibility sweep: viv
//! dropped it entirely, writing `installed.php`'s per-package `aliases` from
//! `extra.branch-alias`/`default-branch` metadata alone. Composer's
//! `Locker::getLockedRepository` wraps the locked package in a
//! `CompleteAliasPackage` per lock-`aliases` entry, and
//! `FilesystemRepository::generateInstalledVersions` appends each such
//! alias's pretty version to that package's `aliases` list — on top of, not
//! instead of, a branch alias for the same package
//! (`tests/fixtures/root-alias/composer.lock`'s `default-branch: true`
//! entry has both). Composer's own output (Composer 2.10.2, verified by
//! running `composer install` against an equivalent lock with a local git
//! source) puts the root alias first: `array(0 => '1.0.0', 1 =>
//! '9999999-dev')`. `installed.json` is untouched by a root alias — the
//! `AliasPackage` is skipped when Composer dumps real installed packages.

use std::path::{Path, PathBuf};

use vivace::autoload::installed::installed_php;
use vivace::lock::{Package, read_lock, read_root};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/root-alias")
        .join(name)
}

#[test]
fn installed_php_carries_a_root_alias_alongside_a_branch_alias() {
    let root = read_root(&fixture("composer.json")).unwrap();
    let lock = read_lock(&fixture("composer.lock")).unwrap();
    let packages: Vec<Package> = lock.packages(true).cloned().collect();
    let refs: Vec<&Package> = packages.iter().collect();

    let expected = fs_err::read_to_string(fixture("expected/installed.php")).unwrap();
    assert_eq!(installed_php(&root, &lock, &refs, true).unwrap(), expected);
}
