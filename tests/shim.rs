//! `composer` shim: maps supported subcommands to `viv`, execs the real
//! Composer for everything else. Uses fake `composer`/`viv` scripts on a
//! temp `PATH` that just echo their argv, so we can assert on which one ran
//! and with what.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use assert_cmd::Command;
use tempfile::tempdir;

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
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .args(["install", "--no-dev"])
        .assert()
        .success()
        .stdout("viv\ninstall\n--no-dev\n");
}

#[test]
fn update_delegates_to_real_composer() {
    let dir = tempdir().unwrap();
    fake_bin(dir.path(), "composer");
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .arg("update")
        .assert()
        .success()
        .stdout("composer\nupdate\n");
}

#[test]
fn install_with_unknown_flag_delegates_to_real_composer() {
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
    let dir = tempdir().unwrap();
    // No fake `composer` on PATH: only `viv` is present, so `update` (which
    // the shim never handles) has nowhere to delegate to.
    fake_bin(dir.path(), "viv");

    shim_in(dir.path())
        .arg("update")
        .assert()
        .failure()
        .code(1)
        .stderr(predicates::str::contains("no real Composer found"));
}
