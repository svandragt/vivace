//! `composer` shim: maps supported subcommands to `viv`, execs the real
//! Composer for everything else. Uses fake `composer`/`viv` scripts on a
//! temp `PATH` that just echo their argv, so we can assert on which one ran
//! and with what.

use std::fs;
use std::io::Write as _;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::Path;

use assert_cmd::Command;
use tempfile::tempdir;
use vivace::lock::Package;
use vivace::store::Store;

/// Serializes each test's fake-`composer`-copy + spawn against the others.
/// `fs::copy` briefly holds its destination open for write; a concurrent
/// thread's `Command::spawn` forks (inheriting that fd process-wide) and,
/// if it hasn't exec'd yet when this thread execs its own freshly-copied
/// binary, the kernel sees a write-open fd on that inode and returns
/// `ETXTBSY` ("text file busy"). Holding this lock for the whole test body
/// keeps the copy-then-exec sequences from interleaving.
static SHIM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// A shell script on `PATH` that prints its own name and argv, one per line.
fn fake_bin(dir: &Path, name: &str) {
    let path = dir.join(name);
    fs::write(
        &path,
        format!("#!/bin/sh\necho {name}\nfor a in \"$@\"; do echo \"$a\"; done\n"),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// The shim looks for `viv` next to its own executable before falling back
/// to `PATH`, and target/debug always has a real `viv` sitting there. Copy
/// the built shim into the fake-bin dir so that same-directory lookup finds
/// our fake `viv` script instead.
fn shim_in(dir: &Path) -> Command {
    let shim_src = assert_cmd::cargo::cargo_bin("composer");
    let shim_dst = dir.join("composer-shim");
    fs::copy(&shim_src, &shim_dst).unwrap();
    fs::set_permissions(&shim_dst, fs::Permissions::from_mode(0o755)).unwrap();

    let mut cmd = Command::new(shim_dst);
    cmd.env("PATH", dir);
    cmd.env_remove("VIV_COMPOSER_PATH");
    cmd
}

#[test]
fn install_with_supported_flags_calls_viv() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["install", "--no-dev", "--no-progress"])
        .assert()
        .success()
        .stdout("viv\ninstall\n--no-dev\n--no-progress\n");
}

/// `--working-dir=<dir>` (the `=` form) through `translate`: same in-place
/// rewrite `translate_with_packages` does for `update` above. viv rejects a
/// bare `--working-dir` outright, so a regression here would turn a working
/// `composer install --working-dir=x` into a hard failure.
#[test]
fn install_working_dir_equals_form_rewrites_to_dash_d() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["install", "--working-dir=/some/dir"])
        .assert()
        .success()
        .stdout("viv\ninstall\n-d=/some/dir\n");
}

/// `--working-dir <dir>` (space-separated) through `translate`: unlike
/// `translate_with_packages`, `translate` has no `!arg.starts_with('-')`
/// positional passthrough (`install` takes no package positionals), so the
/// bare `/some/dir` that follows hits `classify` on its own and comes back
/// `Unknown`, falling the whole invocation through to the real Composer.
/// Pinned here so that asymmetry with the `update`/`add`/`rm` path (which
/// does keep it as `-d`'s value) doesn't silently change.
#[test]
fn install_working_dir_space_form_delegates_to_real_composer() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["install", "--working-dir", "/some/dir"])
        .assert()
        .success()
        .stdout("composer\ninstall\n--working-dir\n/some/dir\n");
}

#[test]
fn dump_autoload_with_supported_flag_calls_viv() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["dump-autoload", "-o"])
        .assert()
        .success()
        .stdout("viv\ndump-autoload\n-o\n");
}

#[test]
fn normalize_calls_viv() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .arg("normalize")
        .assert()
        .success()
        .stdout("viv\nnormalize\n");
}

#[test]
fn create_project_with_unknown_flag_delegates_to_real_composer() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["create-project", "acme/app", "--stability", "dev"])
        .assert()
        .success()
        .stdout("composer\ncreate-project\nacme/app\n--stability\ndev\n");
}

#[test]
fn update_calls_viv() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .arg("update")
        .assert()
        .success()
        .stdout("viv\nupdate\n");
}

/// Partial-update package positionals and `-w`/`-W` (#186's own examples)
/// pass straight through to `viv update`.
#[test]
fn update_with_packages_and_dependency_flags_calls_viv() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["update", "acme/pkg", "-w"])
        .assert()
        .success()
        .stdout("viv\nupdate\nacme/pkg\n-w\n");
}

/// `--working-dir=<dir>` (the `=` form): `translate_with_packages` rewrites
/// the whole arg in place, same as `translate` does for `install` below.
#[test]
fn update_working_dir_equals_form_rewrites_to_dash_d() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["update", "--working-dir=/some/dir"])
        .assert()
        .success()
        .stdout("viv\nupdate\n-d=/some/dir\n");
}

/// `--working-dir <dir>` (space-separated): the flag becomes `-d`, and the
/// following bare `/some/dir` must still arrive as `-d`'s value rather than
/// take the `!arg.starts_with('-')` early-continue meant for package
/// positionals.
#[test]
fn update_working_dir_space_form_keeps_the_path_as_its_value() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["update", "--working-dir", "/some/dir"])
        .assert()
        .success()
        .stdout("viv\nupdate\n-d\n/some/dir\n");
}

/// `-vv`/`-vvv` both collapse to `-v`, viv's only verbosity level.
#[test]
fn update_extra_verbose_flags_collapse_to_single_v() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["update", "-vv"])
        .assert()
        .success()
        .stdout("viv\nupdate\n-v\n");

    shim_in(dir.path())
        .args(["update", "-vvv"])
        .assert()
        .success()
        .stdout("viv\nupdate\n-v\n");
}

#[test]
fn update_with_unknown_flag_delegates_to_real_composer() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["update", "--root-reqs"])
        .assert()
        .success()
        .stdout("composer\nupdate\n--root-reqs\n");
}

#[test]
fn require_calls_viv() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["require", "acme/pkg", "--dev"])
        .assert()
        .success()
        .stdout("viv\nadd\nacme/pkg\n--dev\n");
}

#[test]
fn require_with_unknown_flag_delegates_to_real_composer() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["require", "acme/pkg", "--fixed"])
        .assert()
        .success()
        .stdout("composer\nrequire\nacme/pkg\n--fixed\n");
}

#[test]
fn remove_calls_viv() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["remove", "acme/pkg"])
        .assert()
        .success()
        .stdout("viv\nrm\nacme/pkg\n");
}

#[test]
fn remove_with_unknown_flag_delegates_to_real_composer() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["remove", "acme/pkg", "--unused"])
        .assert()
        .success()
        .stdout("composer\nremove\nacme/pkg\n--unused\n");
}

#[test]
fn install_with_unknown_flag_delegates_to_real_composer() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["install", "--unknown-flag"])
        .assert()
        .success()
        .stdout("composer\ninstall\n--unknown-flag\n");
}

#[test]
fn missing_real_composer_exits_with_message() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    // No fake `composer` on PATH: only `viv` is present, so `self-update`
    // (which the shim never handles) has nowhere to delegate to.
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .arg("self-update")
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("no real Composer found"));
}

/// A lock package matching what `Store::add_zip` needs, pre-populating the
/// store offline the way `tests/install_e2e.rs`'s `store_package` does.
fn store_package(name: &str) -> Package {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "version": "1.0.0",
        "dist": {
            "type": "zip",
            "url": "https://example.invalid/pkg.zip",
            "reference": "deadbeef",
            "shasum": "",
        },
    }))
    .unwrap()
}

fn zip_of_one_file(name: &str, content: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(content).unwrap();
    writer.finish().unwrap().into_inner()
}

/// #123: `composer install` through the shim on a vendor/ Composer wrote
/// must adopt it (relink from the store) without prompting when stdin isn't
/// a terminal (`Command::output`'s default, same as every other test here),
/// even though the shim sets `VIV_VIA_SHIM` — only an interactive TTY would
/// see the confirmation.
#[test]
fn install_through_shim_adopts_a_composer_written_vendor_without_prompting() {
    let _guard = SHIM_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    // The real `viv`, not the echo stub other tests use here: this test
    // needs an actual install to run against a real store and vendor/.
    let viv_src = assert_cmd::cargo::cargo_bin("viv");
    let viv_dst = dir.path().join("viv");
    fs::copy(&viv_src, &viv_dst).unwrap();
    fs::set_permissions(&viv_dst, fs::Permissions::from_mode(0o755)).unwrap();

    let project = tempdir().unwrap();
    let cache = tempdir().unwrap();
    fs::write(
        project.path().join("composer.json"),
        r#"{"name": "acme/app", "require": {"acme/pkg": "^1.0"}}"#,
    )
    .unwrap();
    fs::write(
        project.path().join("composer.lock"),
        r#"{
            "packages": [
                {
                    "name": "acme/pkg",
                    "version": "1.0.0",
                    "dist": {
                        "type": "zip",
                        "url": "https://example.invalid/pkg.zip",
                        "reference": "deadbeef",
                        "shasum": ""
                    }
                }
            ],
            "packages-dev": []
        }"#,
    )
    .unwrap();

    let store = Store::open(&cache.path().join("vivace")).unwrap();
    store
        .add_zip(
            &store_package("acme/pkg"),
            &zip_of_one_file("pkg.txt", b"hi"),
        )
        .unwrap();
    drop(store);

    // A prior real install, straight through `viv` (not the shim): leaves a
    // store-linked, viv-written vendor/.
    Command::new(&viv_dst)
        .current_dir(project.path())
        .env("XDG_CACHE_HOME", cache.path())
        .args(["install", "--offline"])
        .assert()
        .success();

    let target = project.path().join("vendor/acme/pkg/pkg.txt");
    let store_ino = fs::metadata(&target).unwrap().ino();

    // Simulate a tree Composer (or a pre-adopt viv) overwrote: drop the
    // state marker and replace the hardlink with a plain writable copy.
    fs::remove_file(project.path().join("vendor/composer/.vivace-state")).unwrap();
    let content = fs::read(&target).unwrap();
    fs::remove_file(&target).unwrap();
    fs::write(&target, &content).unwrap();
    assert_ne!(fs::metadata(&target).unwrap().ino(), store_ino);

    shim_in(dir.path())
        .current_dir(project.path())
        .env("XDG_CACHE_HOME", cache.path())
        .arg("install")
        .assert()
        .success();

    let relinked = fs::metadata(&target).unwrap();
    assert_eq!(
        relinked.ino(),
        store_ino,
        "the shim's install should adopt: relink from the store"
    );
    assert!(relinked.nlink() > 1, "adopt should hardlink, not copy");
}
