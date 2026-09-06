//! Diff the lock against `vendor/composer/installed.json` to decide what to
//! keep, install, and remove.

use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::lock::{Lock, Package};

/// One `packages[]` entry of `installed.json`, as much of it as planning needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledEntry {
    pub name: String,
    pub reference: Option<String>,
    /// Absolute path of the package dir.
    pub install_path: PathBuf,
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
/// name, `dist.reference` and dev flag; otherwise it is (re)installed.
/// Installed entries without a locked counterpart are removed. No
/// `installed.json` means a fresh install of everything.
pub fn plan(lock: &Lock, dev: bool, vendor_dir: &Path) -> Result<Plan> {
    let mut installed = read_installed(&vendor_dir.join("composer/installed.json"))?;
    let mut plan = Plan::default();
    for package in lock.packages(dev) {
        let reference = package.dist.as_ref().and_then(|d| d.reference.clone());
        match installed.remove(&package.name) {
            Some((entry, was_dev)) if entry.reference == reference && was_dev == package.dev => {
                plan.keep.push(package.clone());
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
fn read_installed(path: &Path) -> Result<HashMap<String, (InstalledEntry, bool)>> {
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
        let dev = file.dev_package_names.contains(&entry.name);
        installed.insert(
            entry.name.clone(),
            (
                InstalledEntry {
                    name: entry.name,
                    reference: entry.dist.and_then(|d| d.reference),
                    install_path: normalise(&composer_dir.join(install_path)),
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
    dist: Option<InstalledDist>,
    #[serde(rename = "install-path")]
    install_path: Option<String>,
}

#[derive(Deserialize)]
struct InstalledDist {
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
    fn installed(vendor: &Path, entries: &[(&str, &str, bool, Option<&str>)]) {
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
                reference: Some("r9".into()),
                install_path: vendor.path().join("gone/gone"),
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
}
