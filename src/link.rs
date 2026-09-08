//! Materialise a store tree into `vendor/` by hardlink, falling back to copy.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};

/// uv's `warn_user_once!`: a large install falls back to copy on the first
/// file and stays there, but that's one `link_tree` call per package, so
/// without this the same warning prints once per package.
static WARNED: AtomicBool = AtomicBool::new(false);

/// Same shape as `WARNED`, for the separate clone -> hardlink transition
/// (#19): a run that never asked for `--link-mode clone` must not have its
/// first-ever hardlink fallback swallowed by this one having already fired.
static WARNED_CLONE: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
static WARN_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[cfg(test)]
static WARN_COUNT_CLONE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How store files reach `vendor/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum LinkMode {
    /// One inode shared with the store; vendor files stay read-only.
    #[default]
    Hardlink,
    /// Independent copies, for projects that patch `vendor/` or whose vendor
    /// dir sits on another filesystem.
    Copy,
    /// A reflink (Linux `FICLONE` on btrfs/XFS, macOS `clonefile` on APFS):
    /// a copy-on-write clone sharing the store's extents, so it costs about
    /// as much to make as a hardlink but, unlike a hardlink, is never made
    /// read-only — patching `vendor/` works without `--link-mode copy`'s
    /// extra copy. Falls back to hardlink, then copy, the first time a file
    /// doesn't support it (probed once per run, same as the hardlink ->
    /// copy fallback below).
    Clone,
}

/// Best-effort reflink of a whole file: `Ok` only if the destination now
/// shares the source's extents, `Err` (any reason — unsupported filesystem,
/// cross-device, anything else) leaves no partial file behind and tells the
/// caller to fall back.
#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "FICLONE has no safe wrapper without adding a libc/rustix/nix \
              dependency for one syscall; both fds below are owned Files this \
              function opened, and the call takes no pointer arguments"
)]
fn reflink(src: &Path, dest: &Path) -> std::io::Result<()> {
    use std::os::fd::AsRawFd as _;

    // `linux/fs.h`: `FICLONE` is `_IOW(0x94, 9, int)`, cloning the whole
    // destination file from the source fd passed as the ioctl argument.
    const FICLONE: std::os::raw::c_ulong = 0x4004_9409;

    unsafe extern "C" {
        // The real declaration is variadic (`...`); ioctl's calling
        // convention doesn't change for a fixed 3-argument call, and a tiny
        // extern "C" declaration here avoids pulling in a libc dependency
        // for one syscall (no libc/rustix/nix crate is in Cargo.toml today).
        fn ioctl(
            fd: std::os::raw::c_int,
            request: std::os::raw::c_ulong,
            arg: std::os::raw::c_int,
        ) -> std::os::raw::c_int;
    }

    let src_file = fs_err::File::open(src)?;
    let dest_file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(dest)?;
    // SAFETY: both fds are owned Files opened just above (dest_file for
    // writing, src_file for reading), and the call passes no pointers.
    let ret = unsafe { ioctl(dest_file.as_raw_fd(), FICLONE, src_file.as_raw_fd()) };
    if ret == -1 {
        let err = std::io::Error::last_os_error();
        drop(dest_file);
        // Don't leave the empty file FICLONE needed an fd for lying around
        // for the hardlink/copy fallback to trip over.
        let _ = std::fs::remove_file(dest);
        return Err(err);
    }
    Ok(())
}

/// macOS's `clonefile(2)` creates `dest` itself (it must not exist yet), and
/// leaves nothing behind on failure — no cleanup needed, unlike Linux's
/// ioctl on an fd it had to open first.
#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "clonefile(2) has no safe wrapper without adding a libc/rustix/nix \
              dependency for one syscall; both CStrings below outlive the call \
              and are nul-terminated by CString::new"
)]
fn reflink(src: &Path, dest: &Path) -> std::io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt as _;

    unsafe extern "C" {
        fn clonefile(
            src: *const std::os::raw::c_char,
            dst: *const std::os::raw::c_char,
            flags: u32,
        ) -> std::os::raw::c_int;
    }

    let src = CString::new(src.as_os_str().as_bytes())
        .map_err(|err| std::io::Error::new(ErrorKind::InvalidInput, err))?;
    let dest = CString::new(dest.as_os_str().as_bytes())
        .map_err(|err| std::io::Error::new(ErrorKind::InvalidInput, err))?;
    // SAFETY: `src`/`dest` are valid, nul-terminated C strings owned by this
    // function and kept alive across the call.
    let ret = unsafe { clonefile(src.as_ptr(), dest.as_ptr(), 0) };
    if ret == -1 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// No reflink primitive on any other target (Windows already isn't
/// supported — see `README.md`'s Stopping section): always fall back.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn reflink(_src: &Path, _dest: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(ErrorKind::Unsupported))
}

/// Build `dest` from `src` atomically: link or copy into a temp sibling, then
/// rename over any existing `dest`. Returns the mode actually used, which
/// differs from `mode` only when the first hardlink failed and the whole tree
/// fell back to copying.
pub fn link_tree(src: &Path, dest: &Path, mode: LinkMode) -> Result<LinkMode> {
    let parent = dest
        .parent()
        .with_context(|| format!("{} has no parent directory", dest.display()))?;
    fs_err::create_dir_all(parent)?;
    let temp = tempfile::tempdir_in(parent)?;
    let mut linker = Linker {
        mode,
        linked_any: false,
    };
    linker.walk(src, temp.path())?;

    // Swap the old tree aside before the new one takes its place, so a
    // crash between the two renames still leaves one of them recoverable
    // instead of deleting `dest` before its replacement is ready.
    let old = if dest.symlink_metadata().is_ok() {
        let slot = tempfile::Builder::new()
            .prefix(".old-")
            .tempdir_in(parent)?;
        let slot = slot.keep();
        fs_err::remove_dir(&slot)?;
        fs_err::rename(dest, &slot)?;
        Some(slot)
    } else {
        None
    };
    fs_err::rename(temp.keep(), dest)?;
    if let Some(old) = old {
        fs_err::remove_dir_all(old)?;
    }
    Ok(linker.mode)
}

/// uv's fallback state machine: the first file may switch the tree from
/// hardlink to copy (with one warning); after any file has linked, a failure
/// is an error rather than a silently mixed tree.
struct Linker {
    mode: LinkMode,
    linked_any: bool,
}

impl Linker {
    /// `dest` must already exist (the caller creates each directory once, as
    /// it's first visited, instead of every entry re-asking via
    /// `create_dir_all` — see #78).
    fn walk(&mut self, src: &Path, dest: &Path) -> Result<()> {
        fs_err::set_permissions(dest, PermissionsExt::from_mode(0o755))?;
        for entry in fs_err::read_dir(src)? {
            let entry = entry?;
            let target = dest.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                fs_err::create_dir(&target)?;
                self.walk(&entry.path(), &target)?;
            } else {
                self.file(&entry.path(), &target)?;
            }
        }
        Ok(())
    }

    fn file(&mut self, src: &Path, dest: &Path) -> Result<()> {
        if self.mode == LinkMode::Clone {
            match reflink(src, dest) {
                Ok(()) => {
                    self.linked_any = true;
                    // A reflink's whole point is a writable copy, so unlike
                    // a hardlink it must not stay read-only.
                    return make_writable(dest);
                }
                Err(err) if !self.linked_any => {
                    if !WARNED_CLONE.swap(true, Ordering::Relaxed) {
                        #[cfg(test)]
                        WARN_COUNT_CLONE.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            "cloning into vendor failed ({err}); hardlinking instead for this \
                             and later packages. Pass --link-mode hardlink to silence this \
                             warning."
                        );
                    }
                    self.mode = LinkMode::Hardlink;
                }
                Err(err) => return Err(err.into()),
            }
        }
        if self.mode == LinkMode::Hardlink {
            match fs_err::hard_link(src, dest) {
                Ok(()) => {
                    self.linked_any = true;
                    return Ok(());
                }
                // ext4 caps a file at 65000 links; a popular polyfill shared by
                // enough projects on one machine gets there. Copy this one file
                // and carry on linking the rest.
                Err(err) if err.kind() == ErrorKind::TooManyLinks => {}
                Err(err) if !self.linked_any => {
                    if !WARNED.swap(true, Ordering::Relaxed) {
                        #[cfg(test)]
                        WARN_COUNT.fetch_add(1, Ordering::Relaxed);
                        tracing::warn!(
                            "hardlinking into vendor failed ({err}); copying instead for this \
                             and later packages. Pass --link-mode copy to silence this warning."
                        );
                    }
                    self.mode = LinkMode::Copy;
                }
                Err(err) => return Err(err.into()),
            }
        }
        fs_err::copy(src, dest)?;
        if self.mode == LinkMode::Copy {
            make_writable(dest)?;
        }
        Ok(())
    }
}

/// A hardlinked file shares an inode with the (read-only) store, so its
/// mode stays whatever the store used and must not be touched here. A copy
/// or clone owns its bytes outright, so add the owner write bit back
/// (0444 -> 0644, 0555 -> 0755): for a copy that undoes the store's
/// read-only bit, for a clone it's the entire reason to prefer it over a
/// hardlink.
fn make_writable(dest: &Path) -> Result<()> {
    let mode = fs_err::metadata(dest)?.permissions().mode() & 0o777;
    fs_err::set_permissions(dest, PermissionsExt::from_mode(mode | 0o200))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    use super::*;

    fn source() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs_err::create_dir(dir.path().join("src")).unwrap();
        fs_err::write(dir.path().join("src/A.php"), "<?php").unwrap();
        fs_err::write(dir.path().join("composer.json"), "{}").unwrap();
        fs_err::set_permissions(
            dir.path().join("composer.json"),
            PermissionsExt::from_mode(0o444),
        )
        .unwrap();
        dir
    }

    fn ino(path: &Path) -> u64 {
        fs_err::metadata(path).unwrap().ino()
    }

    fn mode(path: &Path) -> u32 {
        fs_err::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn hardlink_shares_inodes() {
        let src = source();
        let vendor = tempfile::tempdir().unwrap();
        let dest = vendor.path().join("acme/pkg");
        let used = link_tree(src.path(), &dest, LinkMode::Hardlink).unwrap();
        assert_eq!(used, LinkMode::Hardlink);
        assert_eq!(
            ino(&dest.join("src/A.php")),
            ino(&src.path().join("src/A.php"))
        );
        assert_eq!(mode(&dest.join("src")), 0o755);
    }

    #[test]
    fn copy_adds_owner_write_bit_so_vendor_is_patchable() {
        let src = source();
        let vendor = tempfile::tempdir().unwrap();
        let dest = vendor.path().join("acme/pkg");
        let used = link_tree(src.path(), &dest, LinkMode::Copy).unwrap();
        assert_eq!(used, LinkMode::Copy);
        let a = dest.join("src/A.php");
        assert_ne!(ino(&a), ino(&src.path().join("src/A.php")));
        assert_eq!(fs_err::read_to_string(&a).unwrap(), "<?php");
        assert_eq!(mode(&dest.join("composer.json")), 0o644);
    }

    /// Whether `vendor.path()`'s filesystem can reflink at all — probed with
    /// the same `reflink()` production uses, not a separate guess, since a
    /// filesystem/kernel combination that answers differently to this than
    /// to a real call would make the test lie about what it covers.
    fn reflink_supported(source_file: &Path, vendor: &Path) -> bool {
        let probe_dest = vendor.join(".reflink-probe");
        let supported = reflink(source_file, &probe_dest).is_ok();
        let _ = fs_err::remove_file(&probe_dest);
        supported
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn clone_shares_extents_and_stays_writable_where_supported() {
        let src = source();
        let vendor = tempfile::tempdir().unwrap();
        if !reflink_supported(&src.path().join("composer.json"), vendor.path()) {
            eprintln!(
                "skipping clone_shares_extents_and_stays_writable_where_supported: \
                 {} has no reflink support",
                vendor.path().display()
            );
            return;
        }

        let dest = vendor.path().join("acme/pkg");
        let used = link_tree(src.path(), &dest, LinkMode::Clone).unwrap();
        assert_eq!(used, LinkMode::Clone);
        let a = dest.join("src/A.php");
        assert_ne!(
            ino(&a),
            ino(&src.path().join("src/A.php")),
            "a clone is its own inode, not the store's"
        );
        assert_eq!(fs_err::read_to_string(&a).unwrap(), "<?php");
        assert_eq!(
            mode(&dest.join("composer.json")),
            0o644,
            "a clone must not stay read-only like a hardlink"
        );
    }

    /// uv/pnpm's probe-once-and-latch: the first file that can't reflink
    /// downgrades the whole tree to hardlink instead, with one warning, the
    /// same shape as the hardlink -> copy fallback above.
    #[test]
    #[allow(clippy::print_stderr)]
    fn clone_falls_back_to_hardlink_when_unsupported() {
        let src = source();
        let vendor = tempfile::tempdir().unwrap();
        if reflink_supported(&src.path().join("composer.json"), vendor.path()) {
            eprintln!(
                "skipping clone_falls_back_to_hardlink_when_unsupported: {} supports reflink",
                vendor.path().display()
            );
            return;
        }

        WARN_COUNT_CLONE.store(0, Ordering::Relaxed);
        WARNED_CLONE.store(false, Ordering::Relaxed);

        let dest = vendor.path().join("acme/pkg");
        let used = link_tree(src.path(), &dest, LinkMode::Clone).unwrap();
        assert_eq!(
            used,
            LinkMode::Hardlink,
            "must fall back rather than silently keep failing per file"
        );
        assert_eq!(
            ino(&dest.join("src/A.php")),
            ino(&src.path().join("src/A.php"))
        );
        assert_eq!(
            WARN_COUNT_CLONE.load(Ordering::Relaxed),
            1,
            "one warning for the whole tree, not one per file"
        );
    }

    #[test]
    fn existing_dest_is_replaced() {
        let src = source();
        let vendor = tempfile::tempdir().unwrap();
        let dest = vendor.path().join("acme/pkg");
        fs_err::create_dir_all(dest.join("old")).unwrap();
        fs_err::write(dest.join("old/stale.php"), "x").unwrap();
        link_tree(src.path(), &dest, LinkMode::Hardlink).unwrap();
        assert!(!dest.join("old").exists());
        assert!(dest.join("src/A.php").exists());
        let siblings: Vec<_> = fs_err::read_dir(vendor.path().join("acme"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(siblings, ["pkg"], "no .old-*/.tmp* sibling left behind");
    }

    #[test]
    fn read_only_source_stays_read_only() {
        let src = source();
        let vendor = tempfile::tempdir().unwrap();
        let dest = vendor.path().join("acme/pkg");
        link_tree(src.path(), &dest, LinkMode::Hardlink).unwrap();
        assert_eq!(mode(&dest.join("composer.json")), 0o444);
    }

    /// uv's `warn_user_once!`: a 101-package install shouldn't print 101
    /// identical "falling back to copy" warnings, one per call to
    /// `link_tree`.
    #[test]
    #[allow(clippy::print_stderr)]
    fn cross_filesystem_fallback_warns_once_across_calls() {
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap());
        let probe_src = tempfile::NamedTempFile::new().unwrap();
        let probe_dest = home.join(format!(".vivace-link-test-{}", std::process::id()));
        let cross_fs = fs_err::hard_link(probe_src.path(), &probe_dest).is_err();
        let _ = fs_err::remove_file(&probe_dest);
        if !cross_fs {
            eprintln!(
                "skipping cross_filesystem_fallback_warns_once_across_calls: \
                 /tmp and $HOME are on the same filesystem here"
            );
            return;
        }

        WARN_COUNT.store(0, std::sync::atomic::Ordering::Relaxed);
        WARNED.store(false, std::sync::atomic::Ordering::Relaxed);

        let src = source();
        let vendor = tempfile::TempDir::new_in(&home).unwrap();
        link_tree(
            src.path(),
            &vendor.path().join("acme/pkg1"),
            LinkMode::Hardlink,
        )
        .unwrap();
        link_tree(
            src.path(),
            &vendor.path().join("acme/pkg2"),
            LinkMode::Hardlink,
        )
        .unwrap();

        assert_eq!(
            WARN_COUNT.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "warning should fire once per process, not once per link_tree call"
        );
    }
}
