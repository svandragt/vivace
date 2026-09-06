//! Global content-addressed store of extracted dist archives.
//!
//! Layout under the store root (borrowed from uv's versioned cache buckets):
//!
//! - `archive-v0/<sha256 of the archive bytes>/` holds an extracted tree, and
//!   `archive-v0/<sha256>.ok` (a sibling of the dir, not inside it, so `link`
//!   never has to skip it) is written only once extraction finishes: a dir
//!   with no marker is incomplete (crash mid-extraction, or mid-rename) and
//!   `lookup`/`add_archive` treat it as if the dir were missing.
//! - `dists-v0/<vendor>/<name>/<reference>` is a relative symlink to one of
//!   those trees.
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
    ///
    /// One `Store` per process is expected: the shared lock is per-`File`
    /// handle, not per-process, so a second `Store` opened on the same root
    /// in this process would also hold the shared lock and block `prune`'s
    /// exclusive one forever.
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
    ///
    /// `Ok(None)` means the package has no dist entry; an `Err` means the
    /// name or reference is not safe to join into a store path.
    fn pointer(&self, pkg: &Package) -> Result<Option<PathBuf>> {
        let Some(dist) = pkg.dist.as_ref() else {
            return Ok(None);
        };
        // ponytail: two names differing only by case collide on a
        // case-insensitive filesystem (macOS default, Windows); Composer
        // hits the same wall, fold and disambiguate if it ever bites here.
        sanitise_path_component("package name", &pkg.name)?;
        let reference = match dist.reference.as_deref().filter(|r| !r.is_empty()) {
            Some(reference) => reference.to_owned(),
            None => hex(sha1::Sha1::digest(dist.url.as_bytes())),
        };
        sanitise_path_component("dist reference", &reference)?;
        Ok(Some(
            self.root.join(DISTS_BUCKET).join(&pkg.name).join(reference),
        ))
    }

    /// The archive dir a package's dist pointer resolves to, if any. `None`
    /// when the dir is missing *or* incomplete (no `.ok` marker next to it).
    pub fn lookup(&self, pkg: &Package) -> Option<PathBuf> {
        let pointer = self.pointer(pkg).ok().flatten()?;
        let target = fs_err::read_link(pointer).ok()?;
        let dir = self.archive_dir().join(target.file_name()?);
        (dir.is_dir() && archive_marker(&dir).is_file()).then_some(dir)
    }

    /// Thin wrapper kept for callers written before tar dists ([#8]); dispatch
    /// on `pkg.dist.type` lives in [`Store::add_archive`].
    pub fn add_zip(&self, pkg: &Package, zip_bytes: &[u8]) -> Result<PathBuf> {
        self.add_archive(pkg, zip_bytes)
    }

    /// Extract `archive_bytes` into the archive bucket (unless an identical
    /// archive is already there) and point `pkg`'s dist pointer at it. The
    /// archive format is `pkg.dist.type`: `zip`, or `tar` (covering `.tar`,
    /// `.tar.gz`/`.tgz` and `.tar.bz2`, detected from the archive bytes).
    pub fn add_archive(&self, pkg: &Package, archive_bytes: &[u8]) -> Result<PathBuf> {
        let Some(pointer) = self.pointer(pkg)? else {
            bail!("{}: no dist entry", pkg.name);
        };
        let dist_type = &pkg
            .dist
            .as_ref()
            .expect("pointer() returned Some, so dist is Some")
            .r#type;
        let hash_started = std::time::Instant::now();
        let id = hex(Sha256::digest(archive_bytes));
        tracing::debug!(
            package = %pkg.name,
            elapsed_ms = hash_started.elapsed().as_millis(),
            "hashed dist"
        );
        let archive_dir = self.archive_dir();
        let dest = archive_dir.join(&id);
        let marker = archive_marker(&dest);

        if !dest.is_dir() || !marker.is_file() {
            fs_err::create_dir_all(&archive_dir)?;
            let temp = tempfile::tempdir_in(&archive_dir)?;
            let extract_started = std::time::Instant::now();
            extract_archive(dist_type, archive_bytes, temp.path())
                .with_context(|| format!("extracting {} ({})", pkg.name, id))?;
            tracing::debug!(
                package = %pkg.name,
                elapsed_ms = extract_started.elapsed().as_millis(),
                "extracted dist"
            );
            let manifest = archive_manifest(temp.path())?;
            let temp = temp.keep();

            // A dir already at `dest` here has no marker: it's incomplete
            // (an earlier extraction crashed, or lost a race, between its
            // own rename and marker write). Swap it aside first so the
            // rename below lands on an empty target — the same crash-safety
            // shape `link::link_tree` uses for `vendor/`.
            let aside = if dest.is_dir() {
                let slot = archive_dir.join(format!(".stale-{id}"));
                fs_err::rename(&dest, &slot)?;
                Some(slot)
            } else {
                None
            };
            if let Err(err) = fs_err::rename(&temp, &dest) {
                if dest.is_dir() {
                    // Another process finished the same archive first:
                    // theirs is byte-identical, so drop ours.
                    fs_err::remove_dir_all(&temp)?;
                } else {
                    let _ = fs_err::remove_dir_all(&temp);
                    return Err(err.into());
                }
            }
            if let Some(aside) = aside {
                fs_err::remove_dir_all(aside)?;
            }
            write_marker(&marker, &manifest)?;
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
        // ponytail: a random suffix per call rather than a per-process one so
        // concurrent add_zip calls on the same process never share a path
        // (tempfile's Builder retries on a name collision).
        tempfile::Builder::new()
            .prefix(".tmp-")
            .make_in(parent, |p| fs_err::os::unix::fs::symlink(&target, p))?
            .into_temp_path()
            .persist(&pointer)?;
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
        // Stray `.tmp*`/`.stale-*` dirs from an add_zip that never reached
        // its final rename (crash, or a losing race with another process on
        // the same archive), and `.ok` markers whose dir is gone (removed by
        // hand, or a prune that got as far as the dir but not the marker):
        // a marker with no dir would wrongly claim completeness if a dir of
        // the same id ever reappeared without going through `add_archive`.
        let archive_dir = self.archive_dir();
        if archive_dir.is_dir() {
            for entry in fs_err::read_dir(&archive_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".tmp") || name.starts_with(".stale-") {
                    fs_err::remove_dir_all(entry.path())?;
                } else if let Some(id) = name.strip_suffix(".ok")
                    && !archive_dir.join(id).is_dir()
                {
                    fs_err::remove_file(entry.path())?;
                }
            }
        }
        Ok(())
    }
}

/// Reject a package name or dist reference that would escape the store when
/// joined into a pointer path: only Composer's own charset, no leading `/`,
/// no `..` component.
fn sanitise_path_component(kind: &str, value: &str) -> Result<()> {
    if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "._-/".contains(c))
    {
        bail!("{kind} {value:?} contains characters other than [A-Za-z0-9._-/]");
    }
    if value.starts_with('/') {
        bail!("{kind} {value:?} is an absolute path");
    }
    if Path::new(value)
        .components()
        .any(|c| c == Component::ParentDir)
    {
        bail!("{kind} {value:?} contains a `..` component");
    }
    Ok(())
}

/// The `.ok` marker path for an archive dir: a sibling file, not an entry
/// inside the dir, so `link::link_tree` never has to know it exists.
fn archive_marker(dir: &Path) -> PathBuf {
    dir.with_extension("ok")
}

/// Write `manifest` into `marker` via a temp file in the same directory, then
/// rename it into place, so a reader never observes a partially written
/// marker.
fn write_marker(marker: &Path, manifest: &str) -> Result<()> {
    use std::io::Write;
    let parent = marker
        .parent()
        .expect("marker is nested under the archive dir");
    let mut temp = tempfile::Builder::new()
        .prefix(".tmp-ok-")
        .tempfile_in(parent)?;
    temp.write_all(manifest.as_bytes())?;
    temp.persist(marker)?;
    Ok(())
}

/// A one-line `files=<count> bytes=<total>` summary of everything under
/// `dir`, written into the `.ok` marker: a truncated extraction (crash
/// mid-copy) leaves a dir with no marker at all, rather than a marker that
/// might itself be checked against the wrong count.
fn archive_manifest(dir: &Path) -> Result<String> {
    let (files, bytes) = count_tree(dir)?;
    Ok(format!("files={files} bytes={bytes}\n"))
}

fn count_tree(dir: &Path) -> Result<(u64, u64)> {
    let mut files = 0u64;
    let mut bytes = 0u64;
    for entry in fs_err::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            let (nested_files, nested_bytes) = count_tree(&entry.path())?;
            files += nested_files;
            bytes += nested_bytes;
        } else {
            files += 1;
            bytes += entry.metadata()?.len();
        }
    }
    Ok((files, bytes))
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

/// Turn an archive entry name (zip or tar) into a path safely nested under
/// the extraction root. Follows uv's `SanitizedArchivePath`: components are
/// walked so `..` pops rather than escapes, and absolute or
/// control-character names are refused.
fn sanitise(name: &str) -> Result<PathBuf> {
    if name.chars().any(char::is_control) {
        bail!("archive entry {name:?} contains control characters");
    }
    let mut path = PathBuf::new();
    for component in Path::new(name).components() {
        match component {
            Component::Normal(part) => path.push(part),
            Component::CurDir => {}
            Component::ParentDir => {
                if !path.pop() {
                    bail!("archive entry {name:?} escapes the archive root");
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!("archive entry {name:?} is an absolute path");
            }
        }
    }
    if path.as_os_str().is_empty() {
        bail!("archive entry {name:?} resolves to an empty path");
    }
    Ok(path)
}

/// Extract `bytes` (in `dist_type`'s format: `zip` or `tar`) into `dest`.
fn extract_archive(dist_type: &str, bytes: &[u8], dest: &Path) -> Result<()> {
    match dist_type {
        "zip" => extract_zip(bytes, dest),
        "tar" => extract_tar(bytes, dest),
        other => bail!("dist type \"{other}\" is not supported in vivace v0.1 (zip and tar only)"),
    }
}

/// Extract a tar archive into `dest`, applying Composer's single-top-directory
/// rule. Files become 0444, or 0555 when the entry carried any exec bit; the
/// rest of the tar mode is ignored. Symlink and hardlink entries are skipped.
///
/// Composer's `dist.type` is `"tar"` for `.tar`, `.tar.gz`/`.tgz` and
/// `.tar.bz2` alike (`TarDownloader` hands all three to `PharData`, which
/// tells them apart by content); sniff the same way here.
fn extract_tar(bytes: &[u8], dest: &Path) -> Result<()> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        extract_tar_entries(flate2::read::GzDecoder::new(bytes), dest)
    } else if bytes.starts_with(b"BZh") {
        // ponytail: no bzip2 decoder wired in (Cargo.toml only adds `tar` and
        // `flate2`); add the `bzip2` crate here if a tar.bz2 dist shows up.
        bail!("tar.bz2 dists are not supported in vivace v0.1 (no bzip2 decoder wired in)");
    } else {
        extract_tar_entries(bytes, dest)
    }
}

fn extract_tar_entries<R: std::io::Read>(reader: R, dest: &Path) -> Result<()> {
    // ponytail: no cap on inflated size (a tar bomb fills the disk), same
    // gap the zip extractor already carries.
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let name = entry
            .path()?
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("tar entry is not valid UTF-8"))?
            .to_owned();
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            tracing::warn!("skipping symlink entry {name} in archive");
            continue;
        }
        let path = dest.join(sanitise(&name)?);
        if entry_type.is_dir() {
            mkdir_755(&path)?;
            continue;
        }
        if let Some(parent) = path.parent() {
            mkdir_755(parent)?;
        }
        // A repeated entry name would otherwise hit EACCES: the first pass
        // already chmod'd the file read-only.
        if path.is_file() {
            fs_err::remove_file(&path)?;
        }
        let executable = entry.header().mode().unwrap_or(0) & 0o111 != 0;
        let mut file = fs_err::File::create(&path)?;
        std::io::copy(&mut entry, &mut file)
            .with_context(|| format!("writing tar entry {name}"))?;
        file.set_permissions(PermissionsExt::from_mode(if executable {
            0o555
        } else {
            0o444
        }))?;
    }
    strip_single_top_dir(dest)
}

/// Extract `bytes` into `dest`, applying Composer's single-top-directory
/// rule. Files become 0444, or 0555 when the entry carried any exec bit; the
/// rest of the zip mode is ignored. Symlink entries are skipped.
fn extract_zip(bytes: &[u8], dest: &Path) -> Result<()> {
    // ponytail: no cap on inflated size (a zip bomb fills the disk), add a
    // running total checked against a limit if that ever shows up for real.
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
        // A repeated entry name would otherwise hit EACCES: the first pass
        // already chmod'd the file read-only.
        if path.is_file() {
            fs_err::remove_file(&path)?;
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

    fn tar_package(name: &str, reference: &str) -> Package {
        let mut package: Package = serde_json::from_value(json!({
            "name": name,
            "version": "1.0.0",
            "dist": {"type": "tar", "url": "https://example.test/a.tar", "reference": reference, "shasum": ""},
        }))
        .unwrap();
        package.raw = json!({});
        package
    }

    /// Build an uncompressed tar in memory. Names ending in `/` become
    /// directories; a `Some(mode)` sets the unix mode bits.
    fn tar_of(entries: &[(&str, &[u8], Option<u32>)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, content, mode) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_path(name).unwrap();
            if name.ends_with('/') {
                header.set_entry_type(tar::EntryType::Directory);
                header.set_size(0);
                header.set_mode(mode.unwrap_or(0o755));
                header.set_cksum();
                builder.append(&header, std::io::empty()).unwrap();
            } else {
                header.set_size(content.len() as u64);
                header.set_mode(mode.unwrap_or(0o644));
                header.set_cksum();
                builder.append(&header, *content).unwrap();
            }
        }
        builder.into_inner().unwrap()
    }

    fn tar_gz_of(entries: &[(&str, &[u8], Option<u32>)]) -> Vec<u8> {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_of(entries)).unwrap();
        encoder.finish().unwrap()
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
        // One archive dir plus its `.ok` marker, not two archives.
        assert_eq!(
            fs_err::read_dir(root.path().join("archive-v0"))
                .unwrap()
                .count(),
            2
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
    fn lookup_without_marker_is_none() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let pkg = package("acme/pkg", "abc");
        let dir = store.add_zip(&pkg, &zip_of(&[("f", b"1", None)])).unwrap();
        fs_err::remove_file(archive_marker(&dir)).unwrap();
        assert_eq!(store.lookup(&pkg), None);
        assert!(dir.is_dir(), "the dir itself is untouched, only unmarked");
    }

    #[test]
    fn add_archive_reextracts_a_stale_dir_missing_its_marker() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let pkg = package("acme/pkg", "abc");
        let zip = zip_of(&[("f", b"1", None)]);
        let dir = store.add_zip(&pkg, &zip).unwrap();
        // Simulate a truncated extraction: extra junk in the dir, no marker.
        fs_err::write(dir.join("stray"), "junk").unwrap();
        fs_err::remove_file(archive_marker(&dir)).unwrap();

        let dir2 = store.add_zip(&pkg, &zip).unwrap();
        assert_eq!(dir, dir2);
        assert!(
            !dir2.join("stray").exists(),
            "a stale dir should be replaced by re-extraction, not reused"
        );
        assert!(archive_marker(&dir2).is_file());
    }

    #[test]
    fn rejects_reference_that_escapes_the_store() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[("f", b"1", None)]);
        let err = format!(
            "{:#}",
            store
                .add_zip(&package("acme/pkg", "../../evil"), &zip)
                .unwrap_err()
        );
        assert!(
            err.contains("../../evil"),
            "error should name the reference: {err}"
        );
        assert!(!root.path().parent().unwrap().join("evil").exists());
    }

    #[test]
    fn rejects_absolute_reference() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[("f", b"1", None)]);
        let err = format!(
            "{:#}",
            store.add_zip(&package("acme/pkg", "/x"), &zip).unwrap_err()
        );
        assert!(err.contains("/x"), "error should name the reference: {err}");
        assert!(!Path::new("/x").exists());
    }

    #[test]
    fn duplicate_entry_name_uses_the_last_entry() {
        // The `zip` writer refuses to build an archive with a repeated name,
        // so exercise the exact code path a duplicate entry would hit
        // instead: extracting into a dest where the file already exists and
        // is already chmod'd 0444 by the earlier pass.
        let dest = tempfile::tempdir().unwrap();
        extract_zip(&zip_of(&[("A.php", b"first", None)]), dest.path()).unwrap();
        extract_zip(&zip_of(&[("A.php", b"second", None)]), dest.path()).unwrap();
        assert_eq!(
            fs_err::read_to_string(dest.path().join("A.php")).unwrap(),
            "second"
        );
    }

    #[test]
    fn keeps_top_level_ds_store() {
        // Composer only ignores `.DS_Store` when deciding whether the
        // archive has a single top-level directory; it does not delete it.
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[
            ("pkg-abc/", b"", None),
            ("pkg-abc/composer.json", b"{}", None),
            (".DS_Store", b"junk", None),
        ]);
        let dir = store.add_zip(&package("acme/pkg", "abc"), &zip).unwrap();
        assert!(dir.join(".DS_Store").is_file());
    }

    #[test]
    fn prune_removes_stray_temp_archives() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        store
            .add_zip(&package("acme/pkg", "abc"), &zip_of(&[("f", b"1", None)]))
            .unwrap();
        fs_err::create_dir(root.path().join("archive-v0/.tmpstray")).unwrap();
        store.prune().unwrap();
        assert!(!root.path().join("archive-v0/.tmpstray").exists());
        // The one real archive dir plus its `.ok` marker survive.
        assert_eq!(
            fs_err::read_dir(root.path().join("archive-v0"))
                .unwrap()
                .count(),
            2
        );
    }

    #[test]
    fn prune_removes_orphan_markers() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let dir = store
            .add_zip(&package("acme/pkg", "abc"), &zip_of(&[("f", b"1", None)]))
            .unwrap();
        let orphan = root.path().join("archive-v0/deadbeef.ok");
        fs_err::write(&orphan, "files=0 bytes=0\n").unwrap();
        store.prune().unwrap();
        assert!(!orphan.exists());
        assert!(archive_marker(&dir).is_file(), "the real marker survives");
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

    #[test]
    fn tar_strips_single_top_directory() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let tar = tar_of(&[
            ("pkg-abc/", b"", None),
            ("pkg-abc/composer.json", b"{}", None),
            ("pkg-abc/src/", b"", None),
            ("pkg-abc/src/A.php", b"<?php", None),
        ]);
        let dir = store
            .add_archive(&tar_package("acme/pkg", "abc"), &tar)
            .unwrap();
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
    fn tar_rejects_parent_dir_escape() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        // `Header::set_path` refuses a `..` component itself, so poke the raw
        // name bytes directly to build the malicious entry `set_path` exists
        // to prevent in the first place.
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_gnu();
        header.set_size(1);
        header.set_mode(0o644);
        let name = b"../evil";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_cksum();
        builder.append(&header, &b"2"[..]).unwrap();
        let tar = builder.into_inner().unwrap();
        let err = format!(
            "{:#}",
            store
                .add_archive(&tar_package("acme/pkg", "abc"), &tar)
                .unwrap_err()
        );
        assert!(
            err.contains("../evil"),
            "error should name the entry: {err}"
        );
        assert!(!root.path().parent().unwrap().join("evil").exists());
    }

    #[test]
    fn tar_honours_only_the_exec_bit() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let tar = tar_of(&[
            ("bin/", b"", None),
            ("bin/tool", b"#!/bin/sh", Some(0o764)),
            ("plain", b"x", Some(0o666)),
        ]);
        let dir = store
            .add_archive(&tar_package("acme/pkg", "abc"), &tar)
            .unwrap();
        assert_eq!(mode_of(&dir.join("bin/tool")), 0o555);
        assert_eq!(mode_of(&dir.join("plain")), 0o444);
        assert_eq!(mode_of(&dir.join("bin")), 0o755);
    }

    #[test]
    fn tar_gz_extracts() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let tar_gz = tar_gz_of(&[
            ("pkg-abc/", b"", None),
            ("pkg-abc/composer.json", b"{}", None),
        ]);
        let dir = store
            .add_archive(&tar_package("acme/pkg", "abc"), &tar_gz)
            .unwrap();
        assert_eq!(
            fs_err::read_to_string(dir.join("composer.json")).unwrap(),
            "{}"
        );
    }

    #[test]
    fn tar_skips_symlink_entries() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let mut builder = tar::Builder::new(Vec::new());
        let mut file_header = tar::Header::new_gnu();
        file_header.set_path("real").unwrap();
        file_header.set_size(1);
        file_header.set_mode(0o644);
        file_header.set_cksum();
        builder.append(&file_header, &b"1"[..]).unwrap();
        let mut link_header = tar::Header::new_gnu();
        link_header.set_path("link").unwrap();
        link_header.set_entry_type(tar::EntryType::Symlink);
        link_header.set_size(0);
        link_header.set_mode(0o777);
        link_header.set_cksum();
        builder
            .append_link(&mut link_header, "link", "real")
            .unwrap();
        let tar = builder.into_inner().unwrap();

        let dir = store
            .add_archive(&tar_package("acme/pkg", "abc"), &tar)
            .unwrap();
        assert!(dir.join("real").is_file());
        assert!(!dir.join("link").exists());
    }

    #[test]
    fn tar_bz2_errors_clearly() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let mut bytes = b"BZh".to_vec();
        bytes.extend_from_slice(b"91AY&SY");
        let err = format!(
            "{:#}",
            store
                .add_archive(&tar_package("acme/pkg", "abc"), &bytes)
                .unwrap_err()
        );
        assert!(
            err.contains("acme/pkg"),
            "error should name the package: {err}"
        );
        assert!(
            err.contains("tar.bz2"),
            "error should name the format: {err}"
        );
    }

    #[test]
    fn unsupported_dist_type_names_package_and_type() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let mut package: Package = serde_json::from_value(json!({
            "name": "acme/pkg",
            "version": "1.0.0",
            "dist": {"type": "rar", "url": "https://example.test/a.rar", "reference": "abc", "shasum": ""},
        }))
        .unwrap();
        package.raw = json!({});
        let err = format!(
            "{:#}",
            store.add_archive(&package, b"whatever").unwrap_err()
        );
        assert!(
            err.contains("acme/pkg"),
            "error should name the package: {err}"
        );
        assert!(err.contains("rar"), "error should name the type: {err}");
    }
}
