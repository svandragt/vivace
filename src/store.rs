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

use std::collections::HashSet;
use std::io::Cursor;
use std::os::unix::fs::PermissionsExt;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime};

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

    fn dists_dir(&self) -> PathBuf {
        self.root.join(DISTS_BUCKET)
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
        // Path packages (#13) are symlinked/mirrored straight from
        // `dist.url` and never fetched, so they never enter the store.
        if dist.r#type == "path" {
            return Ok(None);
        }
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
    ///
    /// A hit touches the pointer's own mtime (#20), so `prune --older-than`
    /// can tell a pointer still in active use from one no lock has named in
    /// months; best-effort, since a warm install failing over a read-only
    /// cache or a permissions quirk shouldn't fail the whole install.
    pub fn lookup(&self, pkg: &Package) -> Option<PathBuf> {
        let pointer = self.pointer(pkg).ok().flatten()?;
        let target = fs_err::read_link(&pointer).ok()?;
        let dir = self.archive_dir().join(target.file_name()?);
        if !(dir.is_dir() && archive_marker(&dir).is_file()) {
            return None;
        }
        touch_pointer(&pointer);
        Some(dir)
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
    ///
    /// Small (the common case): the whole archive already lives in memory,
    /// so it's hashed and extracted straight from there.
    pub fn add_archive(&self, pkg: &Package, archive_bytes: &[u8]) -> Result<PathBuf> {
        let dist_type = self.dist_type(pkg)?;
        let hash_started = std::time::Instant::now();
        let id = hex(Sha256::digest(archive_bytes));
        tracing::debug!(
            package = %pkg.name,
            elapsed_ms = hash_started.elapsed().as_millis(),
            "hashed dist"
        );
        self.store_extracted(pkg, &id, |dest| {
            extract_archive(
                &pkg.name,
                dist_type,
                Cursor::new(archive_bytes),
                archive_bytes.len() as u64,
                dest,
            )
        })
    }

    /// Same as [`Store::add_archive`], but for a download already spilled to
    /// `path` (#21: a large download streamed to a temp file rather than a
    /// `Vec<u8>`). Hashes and extracts straight from the file so the archive
    /// never has to be loaded into memory whole, which would reintroduce the
    /// peak-RSS problem streaming the download was meant to avoid.
    pub fn add_archive_from_file(&self, pkg: &Package, path: &Path) -> Result<PathBuf> {
        let dist_type = self.dist_type(pkg)?;
        let archive_len = fs_err::metadata(path)?.len();
        let hash_started = std::time::Instant::now();
        let id = hex(sha256_of_file(path)?);
        tracing::debug!(
            package = %pkg.name,
            elapsed_ms = hash_started.elapsed().as_millis(),
            "hashed dist"
        );
        self.store_extracted(pkg, &id, |dest| {
            let file = fs_err::File::open(path)?;
            extract_archive(&pkg.name, dist_type, file, archive_len, dest)
        })
    }

    /// Shared tail of `add_archive`/`add_archive_from_file`: given the
    /// archive's content-addressed `id`, run `extract` into the archive
    /// bucket (unless an identical archive is already there) and point
    /// `pkg`'s dist pointer at it.
    fn store_extracted(
        &self,
        pkg: &Package,
        id: &str,
        extract: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<PathBuf> {
        let Some(pointer) = self.pointer(pkg)? else {
            bail!("{}: no dist entry, or a path dist, never stored", pkg.name);
        };
        let archive_dir = self.archive_dir();
        let dest = archive_dir.join(id);
        let marker = archive_marker(&dest);

        if !dest.is_dir() || !marker.is_file() {
            fs_err::create_dir_all(&archive_dir)?;
            let temp = tempfile::tempdir_in(&archive_dir)?;
            let extract_started = std::time::Instant::now();
            extract(temp.path()).with_context(|| format!("extracting {} ({id})", pkg.name))?;
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
            .join(id);
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

    /// The store's temp area: the same directory `add_archive` extracts
    /// into, so a large download (#21) spilled to a temp file here shares a
    /// filesystem with its eventual archive dir (no cross-device rename) even
    /// when `$TMPDIR` points elsewhere.
    pub fn temp_dir(&self) -> Result<PathBuf> {
        let dir = self.archive_dir();
        fs_err::create_dir_all(&dir)?;
        Ok(dir)
    }

    /// `pkg.dist.type`, using the same "no dist, or a path dist" check
    /// [`Store::pointer`] makes, so `add_archive`/`add_archive_from_file`
    /// fail with the same message `store_extracted` would have hit anyway.
    fn dist_type<'p>(&self, pkg: &'p Package) -> Result<&'p str> {
        self.pointer(pkg)?.ok_or_else(|| {
            anyhow::anyhow!("{}: no dist entry, or a path dist, never stored", pkg.name)
        })?;
        Ok(&pkg
            .dist
            .as_ref()
            .expect("checked above via pointer()")
            .r#type)
    }

    /// Remove every top-level entry that is not a current bucket or `.lock`,
    /// every archive no dist pointer references any more, and (when
    /// `older_than` is set) every dist pointer whose own mtime is older than
    /// that (#20) — [`Store::lookup`] touches a pointer's mtime on every hit,
    /// so "older than N days" means "not installed from in N days".
    pub fn prune(&self, older_than: Option<Duration>) -> Result<PruneReport> {
        self.lock
            .file()
            .lock()
            .with_context(|| format!("locking store {} exclusively", self.root.display()))?;
        let result = self.prune_locked(older_than);
        self.lock.file().lock_shared()?;
        result
    }

    fn prune_locked(&self, older_than: Option<Duration>) -> Result<PruneReport> {
        let mut report = PruneReport::default();
        for entry in fs_err::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            if [ARCHIVE_BUCKET, DISTS_BUCKET, LOCK_FILE]
                .iter()
                .any(|keep| name == *keep)
            {
                continue;
            }
            report.add(&entry.path())?;
            if entry.file_type()?.is_dir() {
                fs_err::remove_dir_all(entry.path())?;
            } else {
                fs_err::remove_file(entry.path())?;
            }
        }

        if let Some(older_than) = older_than {
            self.remove_stale_pointers(older_than, &mut report)?;
        }

        // Stray `.tmp*`/`.stale-*` dirs from an add_zip that never reached
        // its final rename (crash, or a losing race with another process on
        // the same archive), `.ok` markers whose dir is gone (removed by
        // hand, or a prune that got as far as the dir but not the marker;
        // a marker with no dir would wrongly claim completeness if a dir of
        // the same id ever reappeared without going through `add_archive`),
        // and any archive dir no dist pointer references any more.
        let referenced = self.referenced_archive_ids()?;
        let archive_dir = self.archive_dir();
        if archive_dir.is_dir() {
            for entry in fs_err::read_dir(&archive_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if name.starts_with(".tmp") || name.starts_with(".stale-") {
                    report.add(&entry.path())?;
                    fs_err::remove_dir_all(entry.path())?;
                } else if let Some(id) = name.strip_suffix(".ok") {
                    if !archive_dir.join(id).is_dir() {
                        report.add(&entry.path())?;
                        fs_err::remove_file(entry.path())?;
                    }
                } else if entry.file_type()?.is_dir() && !referenced.contains(&name) {
                    report.add(&entry.path())?;
                    fs_err::remove_dir_all(entry.path())?;
                    let marker = archive_marker(&entry.path());
                    if marker.is_file() {
                        report.add(&marker)?;
                        fs_err::remove_file(&marker)?;
                    }
                }
            }
        }
        Ok(report)
    }

    /// Remove every dist pointer under `dists-v0` whose own mtime (touched by
    /// [`Store::lookup`] on every hit) is older than `older_than`, then any
    /// directory left empty by that removal.
    fn remove_stale_pointers(&self, older_than: Duration, report: &mut PruneReport) -> Result<()> {
        let dists_dir = self.dists_dir();
        if !dists_dir.is_dir() {
            return Ok(());
        }
        let cutoff = SystemTime::now().checked_sub(older_than);
        remove_stale_pointers_in(&dists_dir, cutoff, report)
    }

    /// Every archive id currently referenced by a dist pointer, so
    /// `prune_locked` can tell an archive nothing points at any more from
    /// one still in use.
    fn referenced_archive_ids(&self) -> Result<HashSet<String>> {
        let mut ids = HashSet::new();
        collect_referenced_ids(&self.dists_dir(), &mut ids)?;
        Ok(ids)
    }

    /// Archive and dist-pointer counts and total bytes (#20's `viv cache
    /// size`).
    pub fn size(&self) -> Result<CacheSize> {
        let mut archives = 0u64;
        let mut archive_bytes = 0u64;
        let archive_dir = self.archive_dir();
        if archive_dir.is_dir() {
            for entry in fs_err::read_dir(&archive_dir)? {
                let entry = entry?;
                let name = entry.file_name().to_string_lossy().into_owned();
                if entry.file_type()?.is_dir()
                    && !name.starts_with(".tmp")
                    && !name.starts_with(".stale-")
                {
                    archives += 1;
                    archive_bytes += count_tree(&entry.path())?.1;
                }
            }
        }
        Ok(CacheSize {
            archives,
            archive_bytes,
            pointers: count_pointers(&self.dists_dir())?,
        })
    }

    /// `viv cache clean`'s safety check, run before opening (and so before
    /// touching) `dir` at all: a missing or empty directory is fine to
    /// remove outright, but a directory holding anything other than our own
    /// bucket names is refused, in case `--cache-dir`/`$XDG_CACHE_HOME`
    /// points somewhere that isn't actually a vivace cache.
    pub fn looks_like_cache(dir: &Path) -> Result<bool> {
        if !dir.is_dir() {
            return Ok(true);
        }
        for entry in fs_err::read_dir(dir)? {
            let name = entry?.file_name();
            if ![ARCHIVE_BUCKET, DISTS_BUCKET, LOCK_FILE]
                .iter()
                .any(|keep| name == *keep)
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// Remove everything under the store root, then the root directory
    /// itself. Exclusive-locked like `prune`, but (there being nothing left
    /// to open it against afterward) doesn't restore the shared lock.
    pub fn clean(&self) -> Result<()> {
        self.lock
            .file()
            .lock()
            .with_context(|| format!("locking store {} exclusively", self.root.display()))?;
        for entry in fs_err::read_dir(&self.root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                fs_err::remove_dir_all(entry.path())?;
            } else {
                fs_err::remove_file(entry.path())?;
            }
        }
        fs_err::remove_dir(&self.root)?;
        Ok(())
    }
}

/// What [`Store::prune`] removed, for the caller's own report line
/// (`viv cache prune`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PruneReport {
    pub entries: u64,
    pub bytes: u64,
}

impl PruneReport {
    /// Count `path` (file or dir, walked recursively for its byte total)
    /// before it's removed, as one entry.
    fn add(&mut self, path: &Path) -> Result<()> {
        let bytes = if path.is_dir() {
            count_tree(path)?.1
        } else {
            fs_err::metadata(path)?.len()
        };
        self.entries += 1;
        self.bytes += bytes;
        Ok(())
    }

    /// Like `add`, but for a dist pointer, always a symlink: count its own
    /// (tiny) size rather than following it into the archive it points at —
    /// that archive is a shared resource, counted (and freed, if this was
    /// its last pointer) by the orphan-archive sweep instead.
    fn add_pointer(&mut self, path: &Path) -> Result<()> {
        self.entries += 1;
        self.bytes += fs_err::symlink_metadata(path)?.len();
        Ok(())
    }
}

/// Bytes and counts making up the store (#20's `viv cache size`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CacheSize {
    pub archives: u64,
    pub archive_bytes: u64,
    pub pointers: u64,
}

/// Recursively remove every dist-pointer symlink under `dir` whose own mtime
/// predates `cutoff` (`None` skips the whole pass, e.g. if the clock ever
/// looks implausible), then any directory `dir` itself left empty by that.
fn remove_stale_pointers_in(
    dir: &Path,
    cutoff: Option<SystemTime>,
    report: &mut PruneReport,
) -> Result<()> {
    let Some(cutoff) = cutoff else {
        return Ok(());
    };
    for entry in fs_err::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_symlink() {
            let mtime = fs_err::symlink_metadata(&path)?.modified()?;
            if mtime < cutoff {
                report.add_pointer(&path)?;
                fs_err::remove_file(&path)?;
            }
        } else if entry.file_type()?.is_dir() {
            remove_stale_pointers_in(&path, Some(cutoff), report)?;
            if fs_err::read_dir(&path)?.next().is_none() {
                fs_err::remove_dir(&path)?;
            }
        }
    }
    Ok(())
}

/// Recursively collect every archive id a dist-pointer symlink under `dir`
/// resolves to (its target's file name), for `Store::referenced_archive_ids`.
fn collect_referenced_ids(dir: &Path, ids: &mut HashSet<String>) -> Result<()> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs_err::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_symlink() {
            if let Ok(target) = fs_err::read_link(&path)
                && let Some(id) = target.file_name()
            {
                ids.insert(id.to_string_lossy().into_owned());
            }
        } else if entry.file_type()?.is_dir() {
            collect_referenced_ids(&path, ids)?;
        }
    }
    Ok(())
}

/// Recursively count dist-pointer symlinks under `dir`, for `Store::size`.
fn count_pointers(dir: &Path) -> Result<u64> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0u64;
    for entry in fs_err::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_symlink() {
            count += 1;
        } else if entry.file_type()?.is_dir() {
            count += count_pointers(&entry.path())?;
        }
    }
    Ok(count)
}

/// Best-effort: update a dist pointer's own mtime (not the archive it
/// symlinks to) to now, so `prune --older-than` can tell it apart from a
/// pointer no lock has named in months. A failure here (read-only cache,
/// permissions) is silently ignored rather than failing the install that
/// triggered it.
fn touch_pointer(pointer: &Path) {
    let now = filetime::FileTime::now();
    let _ = filetime::set_symlink_file_times(pointer, now, now);
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

/// Entries per archive before extraction refuses to continue (#21): well
/// above any real Composer package, comfortably below what a crafted archive
/// needs to make `read_dir`/inode allocation the bottleneck instead of a
/// deliberate limit.
const MAX_ENTRIES: u64 = 100_000;

/// The inflated-size cap for one archive: `max(64 MiB, 100 * archive_len)`,
/// so a legitimately large package (100 MiB of assets in a 2 MiB zip is
/// unusual, but not a bomb) isn't punished for compressing well, while a
/// crafted archive that inflates far past its own size still trips the cap
/// before it fills the disk. `VIV_MAX_INFLATED_BYTES` overrides the computed
/// value outright, so a test can craft a small bomb without a 64 MiB payload.
fn max_inflated_bytes(archive_len: u64) -> u64 {
    if let Ok(over) = std::env::var("VIV_MAX_INFLATED_BYTES")
        && let Ok(over) = over.parse::<u64>()
    {
        return over;
    }
    (64 * 1024 * 1024).max(archive_len.saturating_mul(100))
}

/// Running totals for one archive's extraction, checked as entries and bytes
/// are produced (not after the fact) so a zip/tar bomb is caught mid-copy
/// instead of after it has already filled the disk.
struct ExtractLimits<'a> {
    package: &'a str,
    max_bytes: u64,
    entries: u64,
    written: u64,
}

impl<'a> ExtractLimits<'a> {
    fn new(package: &'a str, archive_len: u64) -> Self {
        ExtractLimits {
            package,
            max_bytes: max_inflated_bytes(archive_len),
            entries: 0,
            written: 0,
        }
    }

    fn count_entry(&mut self) -> Result<()> {
        self.entries += 1;
        if self.entries > MAX_ENTRIES {
            bail!(
                "{}: archive has more than {MAX_ENTRIES} entries, refusing to extract further \
                 (zip/tar bomb protection)",
                self.package
            );
        }
        Ok(())
    }

    /// Wrap `writer` so every byte written through it counts against this
    /// archive's inflated-size cap, erroring mid-write (not after) once the
    /// cap is exceeded.
    fn counted<'w, W: std::io::Write>(&'w mut self, writer: W) -> CountingWriter<'w, 'a, W> {
        CountingWriter {
            limits: self,
            inner: writer,
        }
    }
}

struct CountingWriter<'a, 'b, W> {
    limits: &'a mut ExtractLimits<'b>,
    inner: W,
}

impl<W: std::io::Write> std::io::Write for CountingWriter<'_, '_, W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.limits.written += buf.len() as u64;
        if self.limits.written > self.limits.max_bytes {
            return Err(std::io::Error::other(format!(
                "{}: inflated size exceeds the {} byte cap, refusing to extract further (zip/tar \
                 bomb protection)",
                self.limits.package, self.limits.max_bytes
            )));
        }
        self.inner.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Hash a file's contents without loading it whole into memory: read in
/// fixed-size chunks, same as the streaming download that produced it.
fn sha256_of_file(path: &Path) -> Result<impl AsRef<[u8]>> {
    let mut file = fs_err::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8 * 1024];
    loop {
        let n = std::io::Read::read(&mut file, &mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize())
}

/// Extract an archive (in `dist_type`'s format: `zip` or `tar`) from
/// `reader` into `dest`. `archive_len` is the archive's own compressed size,
/// used to scale the inflated-size cap (#21).
fn extract_archive<R: std::io::Read + std::io::Seek>(
    package: &str,
    dist_type: &str,
    reader: R,
    archive_len: u64,
    dest: &Path,
) -> Result<()> {
    match dist_type {
        "zip" => extract_zip(package, reader, archive_len, dest),
        "tar" => extract_tar(package, reader, archive_len, dest),
        other => {
            bail!(
                "{package}: dist type \"{other}\" is not supported in vivace v0.1 (zip and tar only)"
            )
        }
    }
}

/// Extract a tar archive into `dest`, applying Composer's single-top-directory
/// rule. Files become 0444, or 0555 when the entry carried any exec bit; the
/// rest of the tar mode is ignored. Symlink and hardlink entries are skipped.
///
/// Composer's `dist.type` is `"tar"` for `.tar`, `.tar.gz`/`.tgz` and
/// `.tar.bz2` alike (`TarDownloader` hands all three to `PharData`, which
/// tells them apart by content); sniff the same way here.
fn extract_tar<R: std::io::Read + std::io::Seek>(
    package: &str,
    mut reader: R,
    archive_len: u64,
    dest: &Path,
) -> Result<()> {
    use std::io::SeekFrom;
    let mut limits = ExtractLimits::new(package, archive_len);
    // Peek the first few bytes to sniff gzip/bzip2 magic, then rewind: works
    // for both an in-memory `Cursor` and an on-disk `File`, unlike the old
    // `bytes.starts_with(...)` check a `Read`-only stream can't do twice.
    let mut magic = [0u8; 3];
    let mut filled = 0usize;
    while filled < magic.len() {
        let n = reader.read(&mut magic[filled..])?;
        if n == 0 {
            break;
        }
        filled += n;
    }
    reader.seek(SeekFrom::Start(0))?;
    if filled >= 2 && magic[..2] == [0x1f, 0x8b] {
        extract_tar_entries(flate2::read::GzDecoder::new(reader), dest, &mut limits)
    } else if filled >= 3 && &magic[..3] == b"BZh" {
        extract_tar_entries(bzip2_rs::DecoderReader::new(reader), dest, &mut limits)
    } else {
        extract_tar_entries(reader, dest, &mut limits)
    }
}

fn extract_tar_entries<R: std::io::Read>(
    reader: R,
    dest: &Path,
    limits: &mut ExtractLimits<'_>,
) -> Result<()> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries()? {
        limits.count_entry()?;
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
        let mut file = limits.counted(fs_err::File::create(&path)?);
        std::io::copy(&mut entry, &mut file)
            .with_context(|| format!("writing tar entry {name}"))?;
        file.inner
            .set_permissions(PermissionsExt::from_mode(if executable {
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
fn extract_zip<R: std::io::Read + std::io::Seek>(
    package: &str,
    reader: R,
    archive_len: u64,
    dest: &Path,
) -> Result<()> {
    let mut limits = ExtractLimits::new(package, archive_len);
    let mut archive = zip::ZipArchive::new(reader)?;
    for index in 0..archive.len() {
        limits.count_entry()?;
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
        let mut file = limits.counted(fs_err::File::create(&path)?);
        std::io::copy(&mut entry, &mut file)
            .with_context(|| format!("writing zip entry {name}"))?;
        let executable = entry.unix_mode().is_some_and(|mode| mode & 0o111 != 0);
        file.inner
            .set_permissions(PermissionsExt::from_mode(if executable {
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

    /// `bzip2-rs` (#27's decoder) only decodes, so unlike `tar_gz_of` this
    /// can't build a tar.bz2 in memory: `tests/fixtures/tar-bz2/sample.tar.bz2`
    /// is a `pkg-abc/{composer.json,src/A.php}` tree, committed as made by
    /// `devbox run -- tar -cjf`.
    fn tar_bz2_fixture() -> Vec<u8> {
        fs_err::read(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tar-bz2/sample.tar.bz2"),
        )
        .unwrap()
    }

    /// Every regular file under `dir`, relative path to contents, walked
    /// recursively: `tar_bz2_matches_zip_of_the_same_content` byte-diffs this
    /// against a zip extraction of identical content.
    fn read_tree(dir: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
        fn walk(root: &Path, dir: &Path, files: &mut std::collections::BTreeMap<PathBuf, Vec<u8>>) {
            for entry in fs_err::read_dir(dir).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                if entry.file_type().unwrap().is_dir() {
                    walk(root, &path, files);
                } else {
                    files.insert(
                        path.strip_prefix(root).unwrap().to_path_buf(),
                        fs_err::read(&path).unwrap(),
                    );
                }
            }
        }
        let mut files = std::collections::BTreeMap::new();
        walk(dir, dir, &mut files);
        files
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
        let first = zip_of(&[("A.php", b"first", None)]);
        extract_zip(
            "acme/pkg",
            Cursor::new(&first),
            first.len() as u64,
            dest.path(),
        )
        .unwrap();
        let second = zip_of(&[("A.php", b"second", None)]);
        extract_zip(
            "acme/pkg",
            Cursor::new(&second),
            second.len() as u64,
            dest.path(),
        )
        .unwrap();
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
        let report = store.prune(None).unwrap();
        assert_eq!(report.entries, 1);
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
        let report = store.prune(None).unwrap();
        assert_eq!(report.entries, 1);
        assert_eq!(report.bytes, "files=0 bytes=0\n".len() as u64);
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
        let report = store.prune(None).unwrap();
        assert_eq!(report.entries, 2, "old-bucket-v0 and stray.txt");
        assert_eq!(
            report.bytes, 1,
            "stray.txt's one byte; old-bucket-v0 is empty"
        );
        assert!(!root.path().join("old-bucket-v0").exists());
        assert!(!root.path().join("stray.txt").exists());
        assert!(root.path().join("archive-v0").is_dir());
        assert!(root.path().join("dists-v0").is_dir());
        assert!(root.path().join(".lock").is_file());
    }

    #[test]
    fn looks_like_cache_accepts_missing_and_empty_dirs() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("does-not-exist");
        assert!(Store::looks_like_cache(&missing).unwrap());

        let empty = root.path().join("empty");
        fs_err::create_dir(&empty).unwrap();
        assert!(Store::looks_like_cache(&empty).unwrap());
    }

    #[test]
    fn looks_like_cache_accepts_our_own_buckets_and_rejects_foreign_content() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        store
            .add_zip(&package("acme/pkg", "abc"), &zip_of(&[("f", b"1", None)]))
            .unwrap();
        assert!(Store::looks_like_cache(root.path()).unwrap());

        fs_err::write(root.path().join("Documents.docx"), "not ours").unwrap();
        assert!(!Store::looks_like_cache(root.path()).unwrap());
    }

    #[test]
    fn clean_removes_the_whole_cache_dir() {
        let root = tempfile::tempdir().unwrap();
        let cache_dir = root.path().join("cache");
        let store = Store::open(&cache_dir).unwrap();
        store
            .add_zip(&package("acme/pkg", "abc"), &zip_of(&[("f", b"1", None)]))
            .unwrap();

        store.clean().unwrap();
        assert!(!cache_dir.exists(), "the cache dir itself should be gone");
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

    /// #21: a download already spilled to a file (the streaming path)
    /// extracts identically to the in-memory path.
    #[test]
    fn add_archive_from_file_matches_add_archive() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[
            ("pkg-abc/", b"", None),
            ("pkg-abc/composer.json", b"{}", None),
        ]);
        let temp_dir = tempfile::tempdir().unwrap();
        let archive_path = temp_dir.path().join("downloaded.zip");
        fs_err::write(&archive_path, &zip).unwrap();

        let dir = store
            .add_archive_from_file(&package("acme/pkg", "abc"), &archive_path)
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
    fn tar_bz2_extracts() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let dir = store
            .add_archive(&tar_package("acme/pkg", "abc"), &tar_bz2_fixture())
            .unwrap();
        assert_eq!(
            fs_err::read_to_string(dir.join("composer.json")).unwrap(),
            "{\"name\":\"acme/pkg\"}"
        );
        assert_eq!(
            fs_err::read_to_string(dir.join("src/A.php")).unwrap(),
            "<?php\nclass A {}\n"
        );
    }

    /// #27: a tar.bz2 dist extracts to exactly the same tree as a zip dist
    /// of the same content, byte for byte.
    #[test]
    fn tar_bz2_matches_zip_of_the_same_content() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let zip = zip_of(&[
            ("pkg-abc/", b"", None),
            ("pkg-abc/composer.json", b"{\"name\":\"acme/pkg\"}", None),
            ("pkg-abc/src/", b"", None),
            ("pkg-abc/src/A.php", b"<?php\nclass A {}\n", None),
        ]);

        let tar_bz2_dir = store
            .add_archive(&tar_package("acme/tarbz2", "a"), &tar_bz2_fixture())
            .unwrap();
        let zip_dir = store.add_zip(&package("acme/zip", "b"), &zip).unwrap();

        assert_eq!(read_tree(&tar_bz2_dir), read_tree(&zip_dir));
    }

    /// #21: a highly compressible entry (all zeroes) inflates far past the
    /// archive's own size; extraction refuses to keep writing once it trips
    /// the byte cap, rather than filling the disk.
    #[test]
    #[allow(
        unsafe_code,
        reason = "nextest gives this test its own process; no other thread touches env vars"
    )]
    fn zip_bomb_trips_the_inflated_size_cap() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        // ponytail: env var only touched by this one test (nextest gives it
        // its own process), so no guard against concurrent mutation needed.
        // SAFETY: single-threaded within this test process at this point.
        unsafe {
            std::env::set_var("VIV_MAX_INFLATED_BYTES", "1024");
        }
        let payload = vec![0u8; 1_000_000];
        let zip = zip_of(&[("bomb", &payload, None)]);
        let err = format!(
            "{:#}",
            store
                .add_zip(&package("acme/pkg", "abc"), &zip)
                .unwrap_err()
        );
        // SAFETY: see above.
        unsafe {
            std::env::remove_var("VIV_MAX_INFLATED_BYTES");
        }
        assert!(
            err.contains("acme/pkg"),
            "error should name the package: {err}"
        );
        assert!(err.contains("1024"), "error should name the limit: {err}");
    }

    /// #21: an archive with more entries than the cap is refused, even when
    /// every entry is empty (so the byte cap alone wouldn't catch it).
    #[test]
    fn too_many_entries_trips_the_entry_count_cap() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let names: Vec<String> = (0..=MAX_ENTRIES).map(|i| format!("f{i}")).collect();
        let entries: Vec<(&str, &[u8], Option<u32>)> =
            names.iter().map(|n| (n.as_str(), &b""[..], None)).collect();
        let zip = zip_of(&entries);
        let err = format!(
            "{:#}",
            store
                .add_zip(&package("acme/pkg", "abc"), &zip)
                .unwrap_err()
        );
        assert!(
            err.contains("acme/pkg"),
            "error should name the package: {err}"
        );
        assert!(
            err.contains(&MAX_ENTRIES.to_string()),
            "error should name the limit: {err}"
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
