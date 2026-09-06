//! Diff the lock against `vendor/composer/installed.json` to decide what to
//! keep, install, and remove.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::lock::{Lock, Package};

/// One `packages[]` entry of `installed.json`, as much of it as planning needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledEntry {
    pub name: String,
    pub version: String,
    pub reference: Option<String>,
    /// Absolute path of the package dir.
    pub install_path: PathBuf,
    /// `abandoned` (a replacement name, or `true` with none suggested).
    /// Compared alongside version/reference so an abandonment change alone
    /// still triggers a reinstall, not a silent keep.
    pub abandoned: Option<Value>,
}

#[derive(Debug, Default)]
pub struct Plan {
    pub keep: Vec<Package>,
    pub install: Vec<Package>,
    pub remove: Vec<InstalledEntry>,
}

impl Plan {
    /// Nothing to fetch, link or delete; only the autoloader may need work.
    pub fn is_noop(&self) -> bool {
        self.install.is_empty() && self.remove.is_empty()
    }
}

/// A locked package is kept when `installed.json` has an entry with the same
/// name, `version`, `dist.reference` and dev flag; otherwise it is
/// (re)installed. Installed entries without a locked counterpart are
/// removed. No `installed.json` means a fresh install of everything.
pub fn plan(lock: &Lock, dev: bool, vendor_dir: &Path) -> Result<Plan> {
    let mut installed = read_installed(&vendor_dir.join("composer/installed.json"), vendor_dir)?;
    let mut plan = Plan::default();
    for package in lock.packages(dev) {
        // A dist-less git-source package (#13) has no `dist.reference`;
        // fall back to `source.reference` so a bumped commit on an
        // unchanged version (a dev branch, say) still reinstalls instead of
        // comparing two `None`s and calling it a match.
        let reference = package
            .dist
            .as_ref()
            .and_then(|d| d.reference.clone())
            .or_else(|| package.source.as_ref().and_then(|s| s.reference.clone()));
        let abandoned = package.raw.get("abandoned").cloned();
        match installed.remove(&package.name) {
            Some((entry, was_dev))
                if entry.reference == reference
                    && entry.version == package.version
                    && was_dev == package.dev
                    && entry.abandoned == abandoned =>
            {
                // installed.json can agree with the lock while the package's
                // own directory is gone (deleted by hand, a half-finished
                // previous install): a metapackage owns no directory and is
                // always kept on a match, everything else must still be on
                // disk to be kept.
                if package.r#type != "metapackage" && !entry.install_path.is_dir() {
                    plan.install.push(package.clone());
                } else {
                    plan.keep.push(package.clone());
                }
            }
            // A stale entry's dir is replaced by the install itself, so it
            // does not also go on the remove list.
            _ => plan.install.push(package.clone()),
        }
    }
    plan.remove = installed.into_values().map(|(entry, _)| entry).collect();
    plan.remove.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(plan)
}

/// Installed entries by name, with their dev flag. Entries without an
/// `install-path` (metapackages) own no directory and are skipped.
fn read_installed(
    path: &Path,
    vendor_dir: &Path,
) -> Result<HashMap<String, (InstalledEntry, bool)>> {
    let content = match fs_err::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(err) => return Err(err.into()),
    };
    let file: InstalledFile =
        serde_json::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
    let composer_dir = path
        .parent()
        .expect("installed.json lives in vendor/composer");
    let mut installed = HashMap::new();
    for entry in file.packages {
        let Some(install_path) = entry.install_path else {
            continue;
        };
        // Composer lowercases package names throughout; installed.json
        // written by an older Composer version might not have.
        let name = entry.name.to_lowercase();
        let dev = file.dev_package_names.contains(&name);
        let install_path = normalise(&composer_dir.join(install_path));
        if !install_path.starts_with(vendor_dir) {
            anyhow::bail!(
                "{name}: install-path escapes {} ({})",
                vendor_dir.display(),
                install_path.display()
            );
        }
        let reference = entry
            .dist
            .and_then(|d| d.reference)
            .or_else(|| entry.source.and_then(|s| s.reference));
        installed.insert(
            name.clone(),
            (
                InstalledEntry {
                    name,
                    version: entry.version,
                    reference,
                    install_path,
                    abandoned: entry.abandoned,
                },
                dev,
            ),
        );
    }
    Ok(installed)
}

/// Fold `..` and `.` lexically so `vendor/composer/../a/a` becomes
/// `vendor/a/a` without touching the filesystem (the dir may be gone).
fn normalise(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[derive(Deserialize)]
struct InstalledFile {
    #[serde(default)]
    packages: Vec<InstalledPackage>,
    #[serde(default, rename = "dev-package-names")]
    dev_package_names: Vec<String>,
}

#[derive(Deserialize)]
struct InstalledPackage {
    name: String,
    version: String,
    dist: Option<InstalledDist>,
    source: Option<InstalledSource>,
    #[serde(rename = "install-path")]
    install_path: Option<String>,
    abandoned: Option<Value>,
}

#[derive(Deserialize)]
struct InstalledDist {
    reference: Option<String>,
}

/// A dist-less git-source package's `installed.json` entry (#13) carries
/// its reference under `source`, not `dist`.
#[derive(Deserialize)]
struct InstalledSource {
    reference: Option<String>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn lock(entries: &[(&str, &str, bool)]) -> Lock {
        let packages = entries
            .iter()
            .map(|(name, reference, dev)| {
                let raw = json!({
                    "name": name,
                    "version": "1.0.0",
                    "dist": {"type": "zip", "url": "https://example.test/a.zip", "reference": reference, "shasum": ""},
                });
                let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
                package.dev = *dev;
                package.raw = raw;
                package
            })
            .collect();
        Lock {
            content_hash: None,
            packages,
        }
    }

    /// Write an installed.json with `(name, reference, dev, install_path)` rows.
    fn write_installed_json(vendor: &Path, entries: &[(&str, &str, bool, Option<&str>)]) {
        let packages: Vec<_> = entries
            .iter()
            .map(|(name, reference, _, path)| {
                json!({
                    "name": name,
                    "version": "1.0.0",
                    "dist": {"type": "zip", "url": "u", "reference": reference, "shasum": ""},
                    "install-path": path,
                })
            })
            .collect();
        let dev_names: Vec<_> = entries
            .iter()
            .filter(|(_, _, dev, _)| *dev)
            .map(|(name, ..)| *name)
            .collect();
        fs_err::create_dir_all(vendor.join("composer")).unwrap();
        fs_err::write(
            vendor.join("composer/installed.json"),
            serde_json::to_string_pretty(&json!({
                "packages": packages,
                "dev": true,
                "dev-package-names": dev_names,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    /// `write_installed_json` plus creating each entry's on-disk package dir,
    /// so existing "keep" fixtures don't trip the must-exist check a keep
    /// requires.
    fn installed(vendor: &Path, entries: &[(&str, &str, bool, Option<&str>)]) {
        write_installed_json(vendor, entries);
        for (_, _, _, path) in entries {
            if let Some(path) = path {
                // Same resolution `read_installed` uses, so the dir this
                // creates is the one `plan` will check for.
                fs_err::create_dir_all(normalise(&vendor.join("composer").join(path))).unwrap();
            }
        }
    }

    fn names(packages: &[Package]) -> Vec<&str> {
        packages.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn missing_installed_json_installs_everything() {
        let vendor = tempfile::tempdir().unwrap();
        let lock = lock(&[("a/a", "r1", false), ("b/b", "r2", true)]);
        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert_eq!(names(&plan.install), ["a/a", "b/b"]);
        assert!(plan.keep.is_empty());
        assert!(plan.remove.is_empty());
        assert!(!plan.is_noop());
    }

    #[test]
    fn identical_state_keeps_everything() {
        let vendor = tempfile::tempdir().unwrap();
        installed(
            vendor.path(),
            &[
                ("a/a", "r1", false, Some("../a/a")),
                ("b/b", "r2", true, Some("../b/b")),
            ],
        );
        let lock = lock(&[("a/a", "r1", false), ("b/b", "r2", true)]);
        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert_eq!(names(&plan.keep), ["a/a", "b/b"]);
        assert!(plan.install.is_empty());
        assert!(plan.remove.is_empty());
        assert!(plan.is_noop());
    }

    /// installed.json agreeing with the lock is not enough to keep a package
    /// whose own directory is gone (deleted by hand, or a crash mid-install):
    /// linking would then have nothing to hardlink from.
    #[test]
    fn keep_requires_the_install_dir_to_exist() {
        let vendor = tempfile::tempdir().unwrap();
        write_installed_json(vendor.path(), &[("a/a", "r1", false, Some("../a/a"))]);
        let lock = lock(&[("a/a", "r1", false)]);
        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert_eq!(names(&plan.install), ["a/a"]);
        assert!(plan.keep.is_empty());
        assert!(plan.remove.is_empty());
    }

    #[test]
    fn changed_reference_reinstalls() {
        let vendor = tempfile::tempdir().unwrap();
        installed(vendor.path(), &[("a/a", "old", false, Some("../a/a"))]);
        let lock = lock(&[("a/a", "new", false)]);
        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert_eq!(names(&plan.install), ["a/a"]);
        assert!(plan.keep.is_empty());
        assert!(plan.remove.is_empty());
    }

    #[test]
    fn orphan_is_removed() {
        let vendor = tempfile::tempdir().unwrap();
        installed(
            vendor.path(),
            &[
                ("a/a", "r1", false, Some("../a/a")),
                ("gone/gone", "r9", false, Some("../gone/gone")),
            ],
        );
        let lock = lock(&[("a/a", "r1", false)]);
        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert_eq!(names(&plan.keep), ["a/a"]);
        assert_eq!(
            plan.remove,
            [InstalledEntry {
                name: "gone/gone".into(),
                version: "1.0.0".into(),
                reference: Some("r9".into()),
                install_path: vendor.path().join("gone/gone"),
                abandoned: None,
            }]
        );
    }

    #[test]
    fn dev_package_removed_when_no_dev() {
        let vendor = tempfile::tempdir().unwrap();
        installed(
            vendor.path(),
            &[
                ("a/a", "r1", false, Some("../a/a")),
                ("b/b", "r2", true, Some("../b/b")),
            ],
        );
        let lock = lock(&[("a/a", "r1", false), ("b/b", "r2", true)]);
        let plan = plan(&lock, false, vendor.path()).unwrap();
        assert_eq!(names(&plan.keep), ["a/a"]);
        assert!(plan.install.is_empty());
        assert_eq!(plan.remove.len(), 1);
        assert_eq!(plan.remove[0].name, "b/b");
    }

    #[test]
    fn dev_flag_mismatch_reinstalls() {
        let vendor = tempfile::tempdir().unwrap();
        installed(vendor.path(), &[("a/a", "r1", true, Some("../a/a"))]);
        let lock = lock(&[("a/a", "r1", false)]);
        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert_eq!(names(&plan.install), ["a/a"]);
        assert!(plan.remove.is_empty(), "replaced in place, not removed");
    }

    #[test]
    fn null_install_path_is_ignored() {
        let vendor = tempfile::tempdir().unwrap();
        installed(vendor.path(), &[("meta/meta", "r1", false, None)]);
        let lock = lock(&[]);
        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert!(plan.is_noop());
        assert!(plan.remove.is_empty());
    }

    /// A package with no dist reference (e.g. a path repo) bumped to a new
    /// version must reinstall: comparing only `reference` would see two
    /// `None`s and wrongly call that a match.
    #[test]
    fn version_bump_with_unchanged_null_reference_reinstalls() {
        let vendor = tempfile::tempdir().unwrap();
        fs_err::create_dir_all(vendor.path().join("composer")).unwrap();
        fs_err::write(
            vendor.path().join("composer/installed.json"),
            serde_json::to_string_pretty(&json!({
                "packages": [{
                    "name": "a/a",
                    "version": "1.0.0",
                    "dist": {"type": "zip", "url": "u", "reference": null, "shasum": ""},
                    "install-path": "../a/a",
                }],
                "dev": true,
                "dev-package-names": [],
            }))
            .unwrap(),
        )
        .unwrap();

        let raw = json!({
            "name": "a/a",
            "version": "2.0.0",
            "dist": {"type": "zip", "url": "https://example.test/a.zip", "reference": null, "shasum": ""},
        });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        let lock = Lock {
            content_hash: None,
            packages: vec![package],
        };

        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert_eq!(names(&plan.install), ["a/a"]);
        assert!(plan.keep.is_empty());
    }

    #[test]
    fn install_path_escaping_vendor_errors() {
        let vendor = tempfile::tempdir().unwrap();
        installed(vendor.path(), &[("a/a", "r1", false, Some("../../../"))]);
        let lock = lock(&[("a/a", "r1", false)]);
        let err = plan(&lock, true, vendor.path()).unwrap_err().to_string();
        assert!(err.contains("a/a"), "error should name the entry: {err}");
    }

    #[test]
    fn installed_json_name_is_lowercased() {
        let vendor = tempfile::tempdir().unwrap();
        installed(vendor.path(), &[("A/A", "r1", false, Some("../a/a"))]);
        let lock = lock(&[("a/a", "r1", false)]);
        let plan = plan(&lock, true, vendor.path()).unwrap();
        assert_eq!(names(&plan.keep), ["a/a"]);
        assert!(plan.install.is_empty());
    }
}
