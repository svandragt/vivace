//! Global content-addressed store of extracted dist archives.
//!
//! Layout under the store root (borrowed from uv's versioned cache buckets):
//!
//! - `archive-v0/<sha256 of the zip bytes>/` holds an extracted tree.
//! - `dists-v0/<vendor>/<name>/<reference>` is a relative symlink to one of
//!   those trees. A pointer that resolves means "extracted"; there are no
//!   marker files.
//! - `.lock` carries a process-lifetime shared advisory lock so `prune` (which
//!   takes it exclusively) never deletes under a running install.
//!
//! Bump a bucket suffix when its format changes; `prune` removes everything
//! that is not a current bucket.

use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};

use crate::lock::Package;

const ARCHIVE_BUCKET: &str = "archive-v0";
const DISTS_BUCKET: &str = "dists-v0";
const LOCK_FILE: &str = ".lock";

/// An opened store. Dropping it releases the shared lock.
#[derive(Debug)]
pub struct Store {
    root: PathBuf,
    lock: fs_err::File,
}

impl Store {
    /// Create `root` if needed and take the shared lock.
    pub fn open(root: &Path) -> Result<Store> {
        fs_err::create_dir_all(root)?;
        let lock = fs_err::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(root.join(LOCK_FILE))?;
        lock.file()
            .lock_shared()
            .with_context(|| format!("locking store {}", root.display()))?;
        Ok(Store {
            root: root.to_path_buf(),
            lock,
        })
    }

    fn archive_dir(&self) -> PathBuf {
        self.root.join(ARCHIVE_BUCKET)
    }

    /// `dists-v0/<vendor>/<name>/<reference>`. Falls back to Composer's own
    /// cache key (sha1 of the dist URL) when the lock has no reference.
    fn pointer(&self, pkg: &Package) -> Option<PathBuf> {
        let dist = pkg.dist.as_ref()?;
        let reference = match dist.reference.as_deref().filter(|r| !r.is_empty()) {
            Some(reference) => reference.to_owned(),
            None => hex(sha1::Sha1::digest(dist.url.as_bytes())),
        };
        Some(self.root.join(DISTS_BUCKET).join(&pkg.name).join(reference))
    }

    /// The archive dir a package's dist pointer resolves to, if any.
    pub fn lookup(&self, pkg: &Package) -> Option<PathBuf> {
        let target = fs_err::read_link(self.pointer(pkg)?).ok()?;
        let dir = self.archive_dir().join(target.file_name()?);
        dir.is_dir().then_some(dir)
    }

    /// Extract `zip_bytes` into the archive bucket (unless an identical archive
    /// is already there) and point `pkg`'s dist pointer at it.
    pub fn add_zip(&self, pkg: &Package, zip_bytes: &[u8]) -> Result<PathBuf> {
        let Some(pointer) = self.pointer(pkg) else {
            bail!("{}: no dist entry", pkg.name);
        };
        let id = hex(Sha256::digest(zip_bytes));
        let archive_dir = self.archive_dir();
        let dest = archive_dir.join(&id);

        if !dest.is_dir() {
            fs_err::create_dir_all(&archive_dir)?;
            let temp = tempfile::tempdir_in(&archive_dir)?;
            extract_zip(zip_bytes, temp.path())
                .with_context(|| format!("extracting {} ({})", pkg.name, id))?;
            let temp = temp.keep();
            if let Err(err) = fs_err::rename(&temp, &dest) {
                // Another process finished the same archive first: theirs is
                // byte-identical, so drop ours.
                if !dest.is_dir() {
                    return Err(err.into());
                }
                fs_err::remove_dir_all(&temp)?;
            }
        }

        let parent = pointer
            .parent()
            .expect("pointer is nested under the dists bucket");
        fs_err::create_dir_all(parent)?;
        let depth = parent
            .strip_prefix(&self.root)
            .expect("pointer is under the store root")
            .components()
            .count();
        let target = PathBuf::from("../".repeat(depth))
            .join(ARCHIVE_BUCKET)
            .join(&id);
        let temp_link = parent.join(format!(".tmp-{}", std::process::id()));
        fs_err::os::unix::fs::symlink(&target, &temp_link)?;
        fs_err::rename(&temp_link, &pointer)?;
        Ok(dest)
    }

    /// Remove every top-level entry that is not a current bucket or `.lock`.
    pub fn prune(&self) -> Result<()> {
        self.lock
            .file()
            .lock()
            .with_context(|| format!("locking store {} exclusively", self.root.display()))?;
        let result = self.prune_locked();
        self.lock.file().lock_shared()?;
        result
    }

    fn prune_locked(&self) -> Result<()> {
        for entry in fs_err::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            if [ARCHIVE_BUCKET, DISTS_BUCKET, LOCK_FILE]
                .iter()
                .any(|keep| name == *keep)
            {
                continue;
            }
            if entry.file_type()?.is_dir() {
                fs_err::remove_dir_all(entry.path())?;
            } else {
                fs_err::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

/// Lowercase hex of a digest.
pub(crate) fn hex(digest: impl AsRef<[u8]>) -> String {
    use std::fmt::Write;
    digest.as_ref().iter().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn mkdir_755(path: &Path) -> Result<()> {
    fs_err::create_dir_all(path)?;
    fs_err::set_permissions(path, PermissionsExt::from_mode(0o755))?;
    Ok(())
}

/// Turn a zip entry name into a path safely nested under the extraction root.
/// Follows uv's `SanitizedArchivePath`: components are walked so `..` pops
/// rather than escapes, and absolute or control-character names are refused.
fn sanitise(name: &str) -> Result<PathBuf> {
    if name.chars().any(char::is_control) {
        bail!("zip entry {name:?} contains control characters");
    }
    let mut path = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !path.pop() {
                    bail!("zip entry {name:?} escapes the archive root");
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("zip entry {name:?} is an absolute path");
            }
        }
    }
    if path.as_os_str().is_empty() {
        bail!("zip entry {name:?} resolves to an empty path");
    }
    Ok(path)
}

/// Extract `bytes` into `dest`, applying Composer's single-top-directory
/// rule. Files become 0444, or 0555 when the entry carried any exec bit; the
/// rest of the zip mode is ignored. Symlink entries are skipped.
fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = std::str::from_utf8(entry.name_raw())
            .map_err(|_| anyhow::anyhow!("zip entry {index} is not valid UTF-8"))?
            .to_owned();
        let path = dest.join(sanitise(&name)?);
        if entry.is_dir() {
            mkdir_755(&path)?;
            continue;
        }
        if entry.is_symlink() {
            tracing::warn!("skipping symlink entry {name} in archive");
            continue;
        }
        if let Some(parent) = path.parent() {
            mkdir_755(parent)?;
        }
        let mut file = fs_err::File::create(&path)?;
        std::io::copy(&mut entry, &mut file)
            .with_context(|| format!("writing zip entry {name}"))?;
        let executable = entry.unix_mode().is_some_and(|mode| mode & 0o111 != 0);
        file.set_permissions(PermissionsExt::from_mode(if executable {
            0o555
        } else {
            0o444
        }))?;
    }
    strip_single_top_dir(dest)
}

/// If `dest` holds exactly one entry (ignoring `.DS_Store`) and it is a
/// directory, hoist its contents into `dest`.
fn strip_single_top_dir(dest: &Path) -> Result<()> {
    let mut entries = Vec::new();
    for entry in fs_err::read_dir(dest)? {
        let entry = entry?;
        if entry.file_name() != ".DS_Store" {
            entries.push(entry.path());
        }
    }
    let [top] = entries.as_slice() else {
        return Ok(());
    };
    if !top.is_dir() {
        return Ok(());
    }
    // Rename first so a child sharing the top dir's name cannot collide.
    let staging = dest.join(".viv-top");
    fs_err::rename(top, &staging)?;
    for child in fs_err::read_dir(&staging)? {
        let child = child?;
        fs_err::rename(child.path(), dest.join(child.file_name()))?;
    }
    fs_err::remove_dir(&staging)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::os::unix::fs::PermissionsExt;

    use serde_json::json;
    use zip::write::SimpleFileOptions;

    use super::*;

    fn package(name: &str, reference: &str) -> Package {
        let mut package: Package = serde_json::from_value(json!({
            "name": name,
            "version": "1.0.0",
            "dist": {"type": "zip", "url": "https://example.test/a.zip", "reference": reference, "shasum": ""},
        }))
        .unwrap();
        package.raw = json!({});
        package
    }

    /// Build a zip in memory. Names ending in `/` become directories; a
    /// `Some(mode)` sets unix permissions.
    fn zip_of(entries: &[(&str, &[u8], Option<u32>)]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        for (name, content, mode) in entries {
            let mut options = SimpleFileOptions::default();
            if let Some(mode) = mode {
                options = options.unix_permissions(*mode);
            }
            if name.ends_with('/') {
                writer.add_directory(*name, options).unwrap();
            } else {
                writer.start_file(*name, options).unwrap();
                writer.write_all(content).unwrap();
            }
        }
        writer.finish().unwrap().into_inner()
    }

    fn mode_of(path: &Path) -> u32 {
        fs_err::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn strips_single_top_directory() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[
            ("pkg-abc/", b"", None),
            ("pkg-abc/composer.json", b"{}", None),
            ("pkg-abc/src/", b"", None),
            ("pkg-abc/src/A.php", b"<?php", None),
        ]);
        let dir = store.add_zip(&package("acme/pkg", "abc"), &zip).unwrap();
        assert_eq!(
            fs_err::read_to_string(dir.join("composer.json")).unwrap(),
            "{}"
        );
        assert_eq!(
            fs_err::read_to_string(dir.join("src/A.php")).unwrap(),
            "<?php"
        );
        assert!(!dir.join("pkg-abc").exists());
    }

    #[test]
    fn keeps_multiple_top_entries() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[("a/", b"", None), ("a/x", b"1", None), ("b", b"2", None)]);
        let dir = store.add_zip(&package("acme/pkg", "abc"), &zip).unwrap();
        assert_eq!(fs_err::read_to_string(dir.join("a/x")).unwrap(), "1");
        assert_eq!(fs_err::read_to_string(dir.join("b")).unwrap(), "2");
    }

    #[test]
    fn rejects_parent_dir_escape() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[("ok", b"1", None), ("../evil", b"2", None)]);
        let err = format!(
            "{:#}",
            store
                .add_zip(&package("acme/pkg", "abc"), &zip)
                .unwrap_err()
        );
        assert!(
            err.contains("../evil"),
            "error should name the entry: {err}"
        );
        assert!(!root.path().parent().unwrap().join("evil").exists());
    }

    #[test]
    fn honours_only_the_exec_bit() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[
            ("bin/", b"", None),
            ("bin/tool", b"#!/bin/sh", Some(0o764)),
            ("plain", b"x", Some(0o666)),
        ]);
        let dir = store.add_zip(&package("acme/pkg", "abc"), &zip).unwrap();
        assert_eq!(mode_of(&dir.join("bin/tool")), 0o555);
        assert_eq!(mode_of(&dir.join("plain")), 0o444);
        assert_eq!(mode_of(&dir.join("bin")), 0o755);
    }

    #[test]
    fn identical_bytes_share_one_archive() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[("f", b"1", None)]);
        let first = store.add_zip(&package("acme/pkg", "ref1"), &zip).unwrap();
        let second = store.add_zip(&package("acme/pkg", "ref2"), &zip).unwrap();
        assert_eq!(first, second);
        assert_eq!(
            fs_err::read_dir(root.path().join("archive-v0"))
                .unwrap()
                .count(),
            1
        );
        assert!(root.path().join("dists-v0/acme/pkg/ref1").exists());
        assert!(root.path().join("dists-v0/acme/pkg/ref2").exists());
    }

    #[test]
    fn lookup_after_add_zip() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let pkg = package("acme/pkg", "abc");
        assert_eq!(store.lookup(&pkg), None);
        let dir = store.add_zip(&pkg, &zip_of(&[("f", b"1", None)])).unwrap();
        assert_eq!(store.lookup(&pkg), Some(dir));
    }

    #[test]
    fn prune_removes_stale_buckets_only() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        store
            .add_zip(&package("acme/pkg", "abc"), &zip_of(&[("f", b"1", None)]))
            .unwrap();
        fs_err::create_dir(root.path().join("old-bucket-v0")).unwrap();
        fs_err::write(root.path().join("stray.txt"), "x").unwrap();
        store.prune().unwrap();
        assert!(!root.path().join("old-bucket-v0").exists());
        assert!(!root.path().join("stray.txt").exists());
        assert!(root.path().join("archive-v0").is_dir());
        assert!(root.path().join("dists-v0").is_dir());
        assert!(root.path().join(".lock").is_file());
    }
}
