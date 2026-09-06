//! Materialise a store tree into `vendor/` by hardlink, falling back to copy.

use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use anyhow::{Context, Result};

/// How store files reach `vendor/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum LinkMode {
    /// One inode shared with the store; vendor files stay read-only.
    #[default]
    Hardlink,
    /// Independent copies, for projects that patch `vendor/` or whose vendor
    /// dir sits on another filesystem.
    Copy,
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
    fn walk(&mut self, src: &Path, dest: &Path) -> Result<()> {
        fs_err::create_dir_all(dest)?;
        fs_err::set_permissions(dest, PermissionsExt::from_mode(0o755))?;
        for entry in fs_err::read_dir(src)? {
            let entry = entry?;
            let target = dest.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                self.walk(&entry.path(), &target)?;
            } else {
                self.file(&entry.path(), &target)?;
            }
        }
        Ok(())
    }

    fn file(&mut self, src: &Path, dest: &Path) -> Result<()> {
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
                    tracing::warn!(
                        "hardlinking {} failed ({err}); falling back to copying for this \
                         package. Pass --link-mode copy to silence this warning.",
                        src.display()
                    );
                    self.mode = LinkMode::Copy;
                }
                Err(err) => return Err(err.into()),
            }
        }
        fs_err::copy(src, dest)?;
        if self.mode == LinkMode::Copy {
            // A hardlinked file stays read-only so an edit can't corrupt the
            // store; a copy shares no inode, so add the owner write bit back
            // (0444 -> 0644, 0555 -> 0755) so patching vendor works.
            let mode = fs_err::metadata(dest)?.permissions().mode() & 0o777;
            fs_err::set_permissions(dest, PermissionsExt::from_mode(mode | 0o200))?;
        }
        Ok(())
    }
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
}
