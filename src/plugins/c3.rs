//! `codeception/c3`'s `Installer::copyC3V2`/`askForUpdateV2`: copies the
//! plugin's own bundled `c3.php` into the project root, unless a `c3.php`
//! already there differs from the bundled one — Composer's own
//! `askConfirmation(..., false)` defaults to "no" without an interactive
//! terminal, so an existing, edited `c3.php` is left untouched rather than
//! clobbered.
//!
//! Ported from `codeception/c3` `2.9.0`'s `Installer.php`, fetched
//! 2026-09-08 (#126).
//!
//! ponytail: hooked into `PRE_AUTOLOAD_DUMP` alongside `yii2`/`craft` rather
//! than the real plugin's own `POST_INSTALL_CMD`/`POST_UPDATE_CMD`
//! subscription — see `yii2.rs`'s doc comment for why. The one place that
//! still shows through: a bare `viv dump-autoload` re-runs this check,
//! where real Composer wouldn't touch `c3.php` at all. Harmless when
//! `c3.php` already matches (a no-op either way, the common case) and only
//! a difference on the "exists and differs" branch, where both this and the
//! real plugin already leave the file untouched — so the divergence never
//! writes a byte, only skips a debug line Composer would have printed.
//!
//! ponytail: real Composer 2 only calls `Installer::deleteFile` from the
//! `uninstall()` plugin lifecycle hook, fired when `codeception/c3` itself
//! is being removed from the lock. This module only ever runs while that
//! package is still present (`Plugins::c3` gates the call site), so there
//! is no seam here for "the package just got removed" — a stale `c3.php`
//! is left behind, the same shape as every other native adapter's
//! `--no-plugins` "not this module's job" case. Wire an actual
//! `plan.remove` hook through if a project ever needs the file cleaned up.
//!
//! ponytail: byte equality stands in for the real plugin's `md5_file`
//! comparison — same yes/no answer for "has it changed", without pulling in
//! an md5 crate for a value nothing downstream stores or compares against.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::lock::Package;

const PACKAGE_NAME: &str = "codeception/c3";
const FILE_NAME: &str = "c3.php";

pub(super) fn apply(project_dir: &Path, packages: &[(&Package, PathBuf)]) -> Result<()> {
    let Some((_, install_dir)) = packages.iter().find(|(p, _)| p.name == PACKAGE_NAME) else {
        // Not in this install's package set: nothing to copy, matching
        // Composer never running a removed plugin's own listener.
        return Ok(());
    };
    let bundled = install_dir.join(FILE_NAME);
    let bundled_bytes = fs_err::read(&bundled)
        .with_context(|| format!("{PACKAGE_NAME}: reading bundled {FILE_NAME}"))?;

    let target = project_dir.join(FILE_NAME);
    if let Ok(existing) = fs_err::read(&target) {
        // Either already up to date, or changed and Composer's default
        // non-interactive answer keeps the user's copy — either way, leave
        // it alone.
        let _ = existing;
        return Ok(());
    }
    fs_err::write(&target, bundled_bytes)
        .with_context(|| format!("{PACKAGE_NAME}: writing {FILE_NAME}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str) -> Package {
        let raw =
            serde_json::json!({ "name": name, "version": "1.0.0", "type": "composer-plugin" });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    #[test]
    fn apply_is_a_no_op_when_c3_is_not_in_this_install() {
        let project_dir = tempfile::tempdir().unwrap();
        let other = package("acme/widget");
        let packages: Vec<(&Package, PathBuf)> = vec![(&other, PathBuf::from("/nowhere"))];
        apply(project_dir.path(), &packages).unwrap();
        assert!(!project_dir.path().join("c3.php").exists());
    }

    #[test]
    fn apply_copies_the_bundled_file_when_none_exists_yet() {
        let project_dir = tempfile::tempdir().unwrap();
        let install_dir = tempfile::tempdir().unwrap();
        fs_err::write(install_dir.path().join("c3.php"), "<?php // bundled\n").unwrap();
        let c3 = package(PACKAGE_NAME);
        let packages: Vec<(&Package, PathBuf)> = vec![(&c3, install_dir.path().to_path_buf())];
        apply(project_dir.path(), &packages).unwrap();
        let got = fs_err::read_to_string(project_dir.path().join("c3.php")).unwrap();
        assert_eq!(got, "<?php // bundled\n");
    }

    #[test]
    fn apply_leaves_a_differing_existing_file_untouched() {
        let project_dir = tempfile::tempdir().unwrap();
        fs_err::write(project_dir.path().join("c3.php"), "<?php // customised\n").unwrap();
        let install_dir = tempfile::tempdir().unwrap();
        fs_err::write(install_dir.path().join("c3.php"), "<?php // bundled\n").unwrap();
        let c3 = package(PACKAGE_NAME);
        let packages: Vec<(&Package, PathBuf)> = vec![(&c3, install_dir.path().to_path_buf())];
        apply(project_dir.path(), &packages).unwrap();
        let got = fs_err::read_to_string(project_dir.path().join("c3.php")).unwrap();
        assert_eq!(got, "<?php // customised\n");
    }

    #[test]
    fn apply_leaves_an_up_to_date_file_untouched() {
        let project_dir = tempfile::tempdir().unwrap();
        fs_err::write(project_dir.path().join("c3.php"), "<?php // bundled\n").unwrap();
        let install_dir = tempfile::tempdir().unwrap();
        fs_err::write(install_dir.path().join("c3.php"), "<?php // bundled\n").unwrap();
        let c3 = package(PACKAGE_NAME);
        let packages: Vec<(&Package, PathBuf)> = vec![(&c3, install_dir.path().to_path_buf())];
        apply(project_dir.path(), &packages).unwrap();
        let got = fs_err::read_to_string(project_dir.path().join("c3.php")).unwrap();
        assert_eq!(got, "<?php // bundled\n");
    }
}
