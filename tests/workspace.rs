//! `viv workspace list` (#276): discovers workspace members from
//! `extra.viv.workspace.members` and reports which other members each one
//! requires. `src/workspace.rs`'s own `tests` module covers every refusal
//! case directly against `discover`; this only exercises the CLI surface
//! against `tests/fixtures/workspace/basic`, a root with three members, one
//! requiring another.
//!
//! `viv workspace init`/`viv workspace add` (#315) are a different, plain
//! Composer mechanism (the aggregate root, `docs/research.md` chapter 3),
//! and get their own tests below: everything but
//! `init_matches_composer_install_byte_for_byte` is network-free, path
//! repositories only, matching the rest of this crate's `tests/path_*.rs`
//! style.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use common::TestContext;
use serde_json::Value;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/workspace/basic")
}

fn run() -> Output {
    Command::new(cargo_bin("viv"))
        .arg("workspace")
        .arg("list")
        .arg("-d")
        .arg(fixture())
        .output()
        .expect("failed to run viv")
}

#[test]
fn lists_every_member_and_the_inter_member_edge() {
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 3, "expected all three members: {stdout}");

    // Sorted by name: acme/bar, acme/baz, acme/foo.
    assert_eq!(lines[0], "acme/bar 1.2.0 packages/bar acme/foo");
    assert_eq!(lines[1], "acme/baz 0.9.0 packages/baz -");
    assert_eq!(lines[2], "acme/foo 1.0.0 packages/foo -");
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

/// `viv workspace init` with no patterns: a dry-run scan, no file written.
/// A `vendor/`/`node_modules/` composer.json is planted to prove the scan
/// skips both.
#[test]
fn init_with_no_patterns_prints_the_scan_and_writes_nothing() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::copy(
        fixture().join("composer.json"),
        project.join("composer.json"),
    )
    .unwrap();
    copy_tree(&fixture().join("packages"), &project.join("packages"));
    fs::create_dir_all(project.join("vendor/should-be-skipped")).unwrap();
    fs::write(project.join("vendor/should-be-skipped/composer.json"), "{}").unwrap();
    fs::create_dir_all(project.join("node_modules/should-be-skipped")).unwrap();
    fs::write(
        project.join("node_modules/should-be-skipped/composer.json"),
        "{}",
    )
    .unwrap();
    let before = fs::read(project.join("composer.json")).unwrap();

    let output = ctx
        .viv()
        .args(["workspace", "init"])
        .output()
        .expect("failed to run viv");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("packages (3)"), "{stdout}");
    assert!(stdout.contains("packages/bar"), "{stdout}");
    assert!(stdout.contains("packages/baz"), "{stdout}");
    assert!(stdout.contains("packages/foo"), "{stdout}");
    assert!(
        stdout.contains(". (1)"),
        "the project's own composer.json should be listed too: {stdout}"
    );
    assert!(
        !stdout.contains("vendor"),
        "vendor/ should be skipped: {stdout}"
    );
    assert!(
        !stdout.contains("node_modules"),
        "node_modules/ should be skipped: {stdout}"
    );

    assert_eq!(
        fs::read(project.join("composer.json")).unwrap(),
        before,
        "a no-args init must not write"
    );
}

/// `viv workspace init packages/*`: one `path` repository for the pattern,
/// one `require` line per member found, the two stability defaults, and a
/// resolve/install through the same `partial_update` `viv add` runs —
/// entirely offline, since a `"path"` repository is canonical by default
/// and answers for every name it provides before the implicit
/// `packagist.org` entry is ever queried.
#[test]
fn init_writes_the_aggregate_root_and_installs_offline() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_tree(&fixture().join("packages"), &project.join("packages"));

    ctx.viv()
        .args(["workspace", "init", "packages/*"])
        .assert()
        .success();

    let written = read_json(&project.join("composer.json"));
    assert!(
        written["name"].as_str().is_some_and(|n| n.contains('/')),
        "{written:#}"
    );
    assert_eq!(
        written["repositories"],
        serde_json::json!([{"type": "path", "url": "packages/*"}])
    );
    assert_eq!(
        written["require"],
        serde_json::json!({
            "acme/bar": "*",
            "acme/baz": "*",
            "acme/foo": "*",
        })
    );
    assert_eq!(written["minimum-stability"], "dev");
    assert_eq!(written["prefer-stable"], true);

    assert!(project.join("composer.lock").is_file());
    for pkg in ["bar", "baz", "foo"] {
        let dest = project.join(format!("vendor/acme/{pkg}"));
        assert!(
            fs::symlink_metadata(&dest).unwrap().is_symlink(),
            "vendor/acme/{pkg} should be a symlink"
        );
    }
}

/// `viv workspace add`: append one more member's `require` line to an
/// existing aggregate root and resolve again, changing `composer.lock`
/// only by that package. `packages/qux` is created only after `init` has
/// already run, so `init`'s own require list never saw it — `add` still
/// resolves it because the `path:packages/*` repository re-globs live.
#[test]
fn add_appends_one_member_and_the_lock_changes_only_by_it() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_tree(&fixture().join("packages"), &project.join("packages"));

    ctx.viv()
        .args(["workspace", "init", "packages/*"])
        .assert()
        .success();
    let before = read_json(&project.join("composer.lock"));

    fs::create_dir_all(project.join("packages/qux")).unwrap();
    fs::write(
        project.join("packages/qux/composer.json"),
        serde_json::to_vec(&serde_json::json!({"name": "acme/qux", "version": "1.0.0"})).unwrap(),
    )
    .unwrap();

    ctx.viv()
        .args(["workspace", "add", "packages/qux"])
        .assert()
        .success();

    let written = read_json(&project.join("composer.json"));
    assert_eq!(written["require"]["acme/qux"], "*");

    let after = read_json(&project.join("composer.lock"));
    let by_name = |lock: &Value| -> BTreeMap<String, Value> {
        lock["packages"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| (p["name"].as_str().unwrap().to_string(), p.clone()))
            .collect()
    };
    let before = by_name(&before);
    let after = by_name(&after);
    for (name, entry) in &before {
        assert_eq!(after.get(name), Some(entry), "{name} should be unchanged");
    }
    assert_eq!(
        after.len(),
        before.len() + 1,
        "exactly one package should have been added"
    );
    assert!(after.contains_key("acme/qux"));

    assert!(
        fs::symlink_metadata(project.join("vendor/acme/qux"))
            .unwrap()
            .is_symlink()
    );
}

/// #315's "done when": the file `viv workspace init` writes is ordinary
/// Composer input, and installing it with `viv` and with real Composer
/// produces the same `vendor/`, byte for byte, each package symlinked in.
/// Skips without `composer` on `PATH` (run through `devbox run --`).
#[test]
fn init_matches_composer_install_byte_for_byte() {
    if common::scrubbed_command("composer")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipping init_matches_composer_install_byte_for_byte: composer is not on PATH");
        return;
    }

    let scratch = tempfile::tempdir().unwrap();
    let seed = scratch.path().join("seed");
    copy_tree(&fixture().join("packages"), &seed.join("packages"));
    let seed_cache = scratch.path().join("seed-cache");
    Command::new(cargo_bin("viv"))
        .args(["workspace", "init", "packages/*", "--no-install"])
        .arg("--cache-dir")
        .arg(&seed_cache)
        .current_dir(&seed)
        .assert()
        .success();

    let composer_dir = scratch.path().join("composer");
    let viv_dir = scratch.path().join("viv");
    copy_tree(&seed, &composer_dir);
    copy_tree(&seed, &viv_dir);

    let composer_home = tempfile::tempdir().unwrap();
    let status = common::scrubbed_command("composer")
        .args([
            "install",
            "--no-interaction",
            "--no-scripts",
            "--no-plugins",
        ])
        .current_dir(&composer_dir)
        .env("COMPOSER_HOME", composer_home.path())
        .env("COMPOSER_CACHE_DIR", composer_home.path().join("cache"))
        .status()
        .unwrap();
    assert!(status.success(), "composer install failed");

    Command::new(cargo_bin("viv"))
        .arg("install")
        .arg("--cache-dir")
        .arg(scratch.path().join("viv-cache"))
        .current_dir(&viv_dir)
        .assert()
        .success();

    assert_vendor_matches(&composer_dir.join("vendor"), &viv_dir.join("vendor"));
    for pkg in ["acme/bar", "acme/baz", "acme/foo"] {
        for vendor in [composer_dir.join("vendor"), viv_dir.join("vendor")] {
            assert!(
                fs::symlink_metadata(vendor.join(pkg)).unwrap().is_symlink(),
                "{}/{pkg} should be a symlink",
                vendor.display()
            );
        }
    }
}

/// Every entry under `dir`, `/`-separated relative to `dir` itself, paired
/// with whether it's a symlink. A symlink is never descended into: on
/// either side here it points back into the shared `packages/` copy this
/// test duplicated verbatim, so its contents are identical by
/// construction, and a mismatched `bool` between the two trees already
/// catches a symlink-vs-copy regression without walking any further.
fn walk(root: &Path, dir: &Path, on_entry: &mut impl FnMut(PathBuf, bool)) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(Result::unwrap).collect();
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let relative = path.strip_prefix(root).unwrap().to_path_buf();
        let file_type = entry.file_type().unwrap();
        // viv's own marker (`src/install.rs`), no Composer equivalent;
        // `compat/run.sh`'s own byte-diff excludes it the same way.
        if relative == Path::new("composer/.vivace-state") {
            continue;
        }
        if file_type.is_symlink() {
            on_entry(relative, true);
        } else if file_type.is_dir() {
            walk(root, &path, on_entry);
        } else {
            on_entry(relative, false);
        }
    }
}

fn assert_vendor_matches(composer_vendor: &Path, viv_vendor: &Path) {
    let mut composer_entries = Vec::new();
    walk(composer_vendor, composer_vendor, &mut |path, is_symlink| {
        composer_entries.push((path, is_symlink));
    });
    let mut viv_entries = Vec::new();
    walk(viv_vendor, viv_vendor, &mut |path, is_symlink| {
        viv_entries.push((path, is_symlink));
    });
    composer_entries.sort();
    viv_entries.sort();
    assert_eq!(
        composer_entries, viv_entries,
        "vendor/ entries (and which are symlinks) differ"
    );

    let mut mismatches = Vec::new();
    for (relative, is_symlink) in &composer_entries {
        // composer.phar writes a LICENSE with two extra blank lines that a
        // from-source Composer does not; viv matches the source one (tests/new.rs).
        if *is_symlink || relative == Path::new("composer/LICENSE") {
            continue;
        }
        let want = fs::read(composer_vendor.join(relative)).unwrap();
        let got = fs::read(viv_vendor.join(relative)).unwrap_or_default();
        if want != got {
            mismatches.push(relative.clone());
        }
    }
    assert!(
        mismatches.is_empty(),
        "files differing from composer's own output: {mismatches:?}"
    );
}
