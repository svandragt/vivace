//! Path-repository and dist-less git-source lock entries (#13): packages
//! whose lock entry has `dist.type: path`, or no `dist` at all with a
//! `source.type` of `git`. Ports of just enough of Composer's
//! `PathDownloader` and `GitDownloader` to symlink/mirror a path package or
//! clone-and-checkout a git one into `vendor/`; neither ever touches the
//! content-addressed store (`store.rs`), which only ever holds zip/tar
//! archives.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use sha1::Digest as _;

use crate::autoload::generator::find_shortest_path;
use crate::lock::Package;
use crate::store::hex;

/// Materialise a `dist.type: path` package into `dest`: a symlink to
/// `dist.url` (resolved against `project_dir`) by default — relative unless
/// `transport-options.relative` is `false` — or a mirrored copy when
/// `transport-options.symlink` is `false`. Ports `PathDownloader::install`'s
/// default strategy; vivace has no Windows junction path, and never falls
/// back from symlink to mirror on a symlink error (Composer only does that
/// when `symlink` isn't pinned `true`, which vivace doesn't distinguish from
/// "unset" — see the module doc's ceiling note below).
pub fn install_path(project_dir: &Path, package: &Package, dest: &Path) -> Result<()> {
    let path_dist = package
        .dist
        .as_ref()
        .expect("caller checked dist.type == \"path\"");
    let target = project_dir.join(&path_dist.url);
    let target = fs_err::canonicalize(&target).with_context(|| {
        format!(
            "{}: path repository url \"{}\" does not exist",
            package.name, path_dist.url
        )
    })?;

    if let Some(parent) = dest.parent() {
        fs_err::create_dir_all(parent)?;
    }
    remove_existing(dest)?;

    if package.transport_options.symlink.unwrap_or(true) {
        let link_target = if package.transport_options.relative.unwrap_or(true) {
            // Canonicalise the parent dir (not `dest` itself, which doesn't
            // exist yet) so both sides of `find_shortest_path` agree on
            // symlinked ancestors, e.g. macOS's `/var` -> `/private/var`;
            // otherwise `target`'s canonical `/private/...` and a
            // non-canonical `/var/...` dest see different roots and the
            // "relative" path comes out absolute.
            let absolute_dest = if dest.is_absolute() {
                dest.to_path_buf()
            } else {
                std::env::current_dir()?.join(dest)
            };
            let absolute_dest = match absolute_dest.parent() {
                Some(parent) => fs_err::canonicalize(parent)?.join(
                    absolute_dest
                        .file_name()
                        .expect("dest has a file name; caller checked dist.type == \"path\""),
                ),
                None => absolute_dest,
            };
            // `directories: false`, matching `PathDownloader::install`'s own
            // call: `dest` is the symlink being created, a file-like entry
            // in its parent dir, not itself the base to measure depth from.
            PathBuf::from(find_shortest_path(
                &absolute_dest.to_string_lossy(),
                &target.to_string_lossy(),
                false,
            ))
        } else {
            target.clone()
        };
        fs_err::os::unix::fs::symlink(&link_target, dest).with_context(|| {
            format!(
                "{}: symlinking {} -> {}",
                package.name,
                dest.display(),
                link_target.display()
            )
        })?;
    } else {
        copy_dir(&target, dest)
            .with_context(|| format!("{}: mirroring {}", package.name, target.display()))?;
    }
    Ok(())
}

fn remove_existing(dest: &Path) -> Result<()> {
    match fs_err::symlink_metadata(dest) {
        Ok(meta) if meta.is_dir() => fs_err::remove_dir_all(dest)?,
        Ok(_) => fs_err::remove_file(dest)?,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    fs_err::create_dir_all(dest)?;
    for entry in fs_err::read_dir(src)? {
        let entry = entry?;
        let to = dest.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs_err::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}

/// Clone-and-checkout a dist-less, `source.type: git` package into `dest`.
///
/// Ports just the shape of `GitDownloader::doInstall`, not its from-cache
/// dance: a bare mirror lives under `<cache_dir>/git-v0/<sha1 of the source
/// url>/`, cloned once and `fetch`ed again only when `reference` isn't
/// already in it (an unbounded mirror never pruned by `viv cache prune`,
/// unlike the archive store — this is a global git object cache, not a
/// per-reference pointer bucket; add its own prune sweep if that
/// unboundedness ever bites). A full clone from that mirror then checks out
/// `reference`, leaving a real `.git` history in `vendor/`, matching
/// Composer's `installation-source: source`.
pub fn checkout_git(cache_dir: &Path, package: &Package, dest: &Path) -> Result<()> {
    let source = package
        .source
        .as_ref()
        .expect("caller checked source.type == \"git\"");
    let reference = source
        .reference
        .as_deref()
        .with_context(|| format!("{}: git source has no reference", package.name))?;

    let mirror = cache_dir
        .join("git-v0")
        .join(hex(sha1::Sha1::digest(source.url.as_bytes())));
    sync_mirror(&source.url, &mirror, reference)
        .with_context(|| format!("{}: syncing git mirror for {}", package.name, source.url))?;

    let parent = dest
        .parent()
        .with_context(|| format!("{} has no parent directory", dest.display()))?;
    fs_err::create_dir_all(parent)?;
    let temp = tempfile::tempdir_in(parent)?;
    let checkout = temp.path().join("checkout");

    run_git(
        None,
        [
            "clone",
            "--no-checkout",
            "--",
            path_str(&mirror)?,
            path_str(&checkout)?,
        ],
    )
    .with_context(|| format!("{}: cloning from the local mirror", package.name))?;
    run_git(Some(&checkout), ["checkout", "--detach", reference, "--"])
        .with_context(|| format!("{}: checking out {reference}", package.name))?;
    run_git(
        Some(&checkout),
        ["remote", "set-url", "origin", "--", &source.url],
    )
    .with_context(|| format!("{}: pointing origin back at {}", package.name, source.url))?;

    // Same crash-safe swap `link::link_tree` uses for `vendor/`: rename the
    // old dir aside before the new one takes its place, so a crash between
    // the two renames still leaves one of them recoverable.
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
    fs_err::rename(&checkout, dest)?;
    if let Some(old) = old {
        fs_err::remove_dir_all(old)?;
    }
    Ok(())
}

fn sync_mirror(url: &str, mirror: &Path, reference: &str) -> Result<()> {
    if mirror.join("HEAD").is_file() {
        if !has_commit(mirror, reference)? {
            run_git(Some(mirror), ["fetch", "--tags", "origin"]).with_context(|| {
                format!("fetching {url} into the mirror at {}", mirror.display())
            })?;
        }
        return Ok(());
    }
    if let Some(parent) = mirror.parent() {
        fs_err::create_dir_all(parent)?;
    }
    run_git(None, ["clone", "--mirror", "--", url, path_str(mirror)?])
        .with_context(|| format!("mirroring {url} into {}", mirror.display()))
}

fn has_commit(repo: &Path, reference: &str) -> Result<bool> {
    Ok(Command::new("git")
        .args(["-C", path_str(repo)?, "cat-file", "-e"])
        .arg(format!("{reference}^{{commit}}"))
        .status()
        .context("running git cat-file")?
        .success())
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))
}

/// Run `git <args>` (optionally with a working dir), erroring with its
/// stderr on a non-zero exit.
fn run_git<'a>(dir: Option<&Path>, args: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let output = command.output().context("running git")?;
    if !output.status.success() {
        bail!(
            "git exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use crate::lock::{Dist, Package, Source, TransportOptions};

    use super::*;

    fn package(name: &str, dist: Option<Dist>, source: Option<Source>) -> Package {
        Package {
            name: name.into(),
            version: "1.0.0".into(),
            dist,
            source,
            transport_options: TransportOptions::default(),
            autoload: None,
            require: serde_json::Map::new(),
            provide: serde_json::Map::new(),
            replace: serde_json::Map::new(),
            r#type: "library".into(),
            target_dir: None,
            bin: Vec::new(),
            include_path: Vec::new(),
            dev: false,
            raw: serde_json::Value::Null,
            install_dir: None,
        }
    }

    fn path_package(url: &str, transport_options: TransportOptions) -> Package {
        let mut pkg = package(
            "acme/hello",
            Some(Dist {
                r#type: "path".into(),
                url: url.into(),
                reference: Some("ref".into()),
                shasum: None,
            }),
            None,
        );
        pkg.transport_options = transport_options;
        pkg
    }

    /// Real source for the target dir, so a package dir with just a plain
    /// file is enough to tell symlink from mirror apart.
    fn real_source_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join("marker.txt"), "hello").unwrap();
        dir
    }

    #[test]
    fn install_path_defaults_to_a_relative_symlink() {
        let project = tempfile::tempdir().unwrap();
        let src = real_source_dir();
        fs_err::create_dir_all(project.path().join("packages")).unwrap();
        let link_name = project.path().join("packages/hello");
        // A relative `dist.url`, resolved against `project_dir`, mirroring
        // how Composer's own lock records a path repository's url.
        std::os::unix::fs::symlink(src.path(), &link_name).unwrap();

        let dest = project.path().join("vendor/acme/hello");
        let package = path_package("packages/hello", TransportOptions::default());
        install_path(project.path(), &package, &dest).unwrap();

        assert!(fs_err::symlink_metadata(&dest).unwrap().is_symlink());
        let target = fs_err::read_link(&dest).unwrap();
        assert!(target.is_relative(), "{}", target.display());
        assert_eq!(
            fs_err::read_to_string(dest.join("marker.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn install_path_relative_symlink_survives_a_symlinked_ancestor() {
        // Simulates macOS, where the system temp dir is under `/var`, itself
        // a symlink to `/private/var`: put the whole project under an
        // `alias -> real` symlink so `target`'s canonicalised path and
        // `dest`'s non-canonical one disagree on the root unless `dest`'s
        // ancestor is canonicalised too.
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs_err::create_dir_all(&real).unwrap();
        let alias = tmp.path().join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();

        let project = alias.join("project");
        let src_dir = project.join("packages/hello");
        fs_err::create_dir_all(&src_dir).unwrap();
        fs_err::write(src_dir.join("marker.txt"), "hello").unwrap();

        let dest = project.join("vendor/acme/hello");
        let package = path_package("packages/hello", TransportOptions::default());
        install_path(&project, &package, &dest).unwrap();

        assert!(fs_err::symlink_metadata(&dest).unwrap().is_symlink());
        let target = fs_err::read_link(&dest).unwrap();
        assert!(target.is_relative(), "{}", target.display());
        assert_eq!(
            fs_err::read_to_string(dest.join("marker.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn install_path_mirrors_when_symlink_is_false() {
        let project = tempfile::tempdir().unwrap();
        let src = real_source_dir();

        let dest = project.path().join("vendor/acme/hello");
        let package = path_package(
            src.path().to_str().unwrap(),
            TransportOptions {
                symlink: Some(false),
                relative: None,
            },
        );
        install_path(project.path(), &package, &dest).unwrap();

        assert!(!fs_err::symlink_metadata(&dest).unwrap().is_symlink());
        assert_eq!(
            fs_err::read_to_string(dest.join("marker.txt")).unwrap(),
            "hello"
        );
    }

    #[test]
    fn install_path_absolute_symlink_when_relative_is_false() {
        let project = tempfile::tempdir().unwrap();
        let src = real_source_dir();

        let dest = project.path().join("vendor/acme/hello");
        let package = path_package(
            src.path().to_str().unwrap(),
            TransportOptions {
                symlink: None,
                relative: Some(false),
            },
        );
        install_path(project.path(), &package, &dest).unwrap();

        let target = fs_err::read_link(&dest).unwrap();
        assert!(target.is_absolute(), "{}", target.display());
        assert_eq!(target, fs_err::canonicalize(src.path()).unwrap());
    }

    #[test]
    fn install_path_replaces_an_existing_dest() {
        let project = tempfile::tempdir().unwrap();
        let src = real_source_dir();

        let dest = project.path().join("vendor/acme/hello");
        fs_err::create_dir_all(&dest).unwrap();
        fs_err::write(dest.join("stale.txt"), "old").unwrap();

        let package = path_package(src.path().to_str().unwrap(), TransportOptions::default());
        install_path(project.path(), &package, &dest).unwrap();

        assert!(fs_err::symlink_metadata(&dest).unwrap().is_symlink());
        assert!(!dest.join("stale.txt").exists());
    }

    fn git_available() -> bool {
        Command::new("git").arg("--version").output().is_ok()
    }

    /// A local, hookless git repo: the global `core.hooksPath` a developer
    /// machine may have configured (it rewrites commit messages here) must
    /// not leak into a test fixture repo.
    fn init_repo(dir: &Path) {
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .args(["-c", "core.hooksPath=/dev/null"])
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "vivace test")
                .env("GIT_AUTHOR_EMAIL", "test@example.test")
                .env("GIT_COMMITTER_NAME", "vivace test")
                .env("GIT_COMMITTER_EMAIL", "test@example.test")
                .status()
                .unwrap();
            assert!(status.success(), "git {args:?}");
        };
        run(&["init", "-q", "-b", "main"]);
        fs_err::write(dir.join("Thing.php"), "<?php\nclass Thing {}\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-q", "-m", "init"]);
    }

    fn head(dir: &Path) -> String {
        String::from_utf8(
            Command::new("git")
                .args(["-C"])
                .arg(dir)
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
    }

    #[test]
    #[allow(clippy::print_stderr)]
    fn checkout_git_clones_and_checks_out_the_reference() {
        if !git_available() {
            eprintln!("skipping checkout_git_clones_and_checks_out_the_reference: git not on PATH");
            return;
        }
        let upstream = tempfile::tempdir().unwrap();
        init_repo(upstream.path());
        let reference = head(upstream.path());

        let cache = tempfile::tempdir().unwrap();
        let vendor = tempfile::tempdir().unwrap();
        let dest = vendor.path().join("acme/vcslib");

        let package = package(
            "acme/vcslib",
            None,
            Some(Source {
                r#type: "git".into(),
                url: upstream.path().to_str().unwrap().into(),
                reference: Some(reference.clone()),
            }),
        );
        checkout_git(cache.path(), &package, &dest).unwrap();

        assert!(dest.join(".git").is_dir());
        assert!(dest.join("Thing.php").is_file());
        assert_eq!(head(&dest), reference);
        assert!(
            cache.path().join("git-v0").is_dir(),
            "the mirror should be cached under git-v0"
        );

        // Re-run against the same reference: the mirror is not re-fetched
        // (nothing to fetch, the reference is already there), and the
        // checkout is replaced cleanly.
        checkout_git(cache.path(), &package, &dest).unwrap();
        assert_eq!(head(&dest), reference);
    }
}
