//! CLI-surface snapshot tests: exit codes and stdout/stderr shape, not the
//! byte-exact `vendor/` contents (that is `install_e2e.rs`'s job).

#[macro_use]
mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog")
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

/// Copy just the source inputs a fresh `composer install` would see: no
/// `vendor/`, so the plan is always a clean install of every lock package.
fn copy_monolog_sources(project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture().join(name), project.join(name)).unwrap();
    }
    for dir in ["src", "lib"] {
        copy_tree(&fixture().join(dir), &project.join(dir));
    }
}

#[test]
fn update_without_a_composer_json_fails() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    cmd.arg("update");
    viv_snapshot!(ctx, cmd);
}

#[test]
fn install_without_a_lock_fails() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    cmd.arg("install");
    viv_snapshot!(ctx, cmd);
}

#[test]
fn install_dry_run_lists_the_plan() {
    let ctx = TestContext::new();
    copy_monolog_sources(ctx.project.path());
    let mut cmd = ctx.viv();
    cmd.args(["install", "--dry-run"]);
    viv_snapshot!(ctx, cmd);
}

/// #33: a stale `content-hash` is a warning, not a hard error — the lock
/// still installs (or, here, still prints a dry-run plan).
#[test]
fn install_warns_when_the_lock_is_stale() {
    let ctx = TestContext::new();
    copy_monolog_sources(ctx.project.path());
    // Adding a requirement without touching composer.lock changes
    // composer.json's content-hash without changing what's locked.
    let composer_json = ctx.project.path().join("composer.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&composer_json).unwrap()).unwrap();
    json["require"]["psr/log"] = "^3.0 || ^2.0".into();
    fs::write(&composer_json, serde_json::to_vec(&json).unwrap()).unwrap();

    let mut cmd = ctx.viv();
    cmd.args(["install", "--dry-run"]);
    viv_snapshot!(ctx, cmd);
}

/// #33: a requirement entirely absent from the lock is fatal
/// (`ERROR_LOCK_FILE_INVALID` in Composer), unlike a stale hash.
#[test]
fn install_fails_when_a_requirement_is_missing_from_the_lock() {
    let ctx = TestContext::new();
    copy_monolog_sources(ctx.project.path());
    let composer_json = ctx.project.path().join("composer.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&composer_json).unwrap()).unwrap();
    json["require"]["acme/not-locked"] = "^1.0".into();
    fs::write(&composer_json, serde_json::to_vec(&json).unwrap()).unwrap();

    let mut cmd = ctx.viv();
    cmd.args(["install", "--dry-run"]);
    viv_snapshot!(ctx, cmd);
}

/// A hand-made lock with one locked `composer-plugin` package that is
/// neither a native adapter (`composer/installers`,
/// `*-wordpress-core-installer`) nor known-inert, enabled via
/// `config.allow-plugins`.
fn write_unknown_plugin_lock(project: &Path) {
    fs::write(
        project.join("composer.json"),
        r#"{
            "name": "vivace/fixture-unknown-plugin",
            "config": {
                "allow-plugins": { "acme/mystery-plugin": true }
            }
        }"#,
    )
    .unwrap();
    fs::write(
        project.join("composer.lock"),
        r#"{
            "packages": [
                {
                    "name": "acme/mystery-plugin",
                    "version": "1.0.0",
                    "type": "composer-plugin",
                    "dist": { "type": "zip", "url": "https://example.test/a.zip", "reference": "abc", "shasum": "" }
                }
            ],
            "packages-dev": []
        }"#,
    )
    .unwrap();
}

/// #51 rule 3: viv refuses to run a Composer plugin it has no native adapter
/// or known-inert entry for, naming it and pointing at
/// `docs/plugin-strategy.md`.
#[test]
fn install_refuses_an_unknown_enabled_composer_plugin() {
    let ctx = TestContext::new();
    write_unknown_plugin_lock(ctx.project.path());
    let mut cmd = ctx.viv();
    cmd.args(["install", "--dry-run"]);
    viv_snapshot!(ctx, cmd);
}

/// `--no-plugins` downgrades the same refusal to a warning and installs
/// under `vendor/` anyway, as Composer does with the same flag.
#[test]
fn install_no_plugins_downgrades_the_refusal_to_a_warning() {
    let ctx = TestContext::new();
    write_unknown_plugin_lock(ctx.project.path());
    let mut cmd = ctx.viv();
    cmd.args(["install", "--dry-run", "--no-plugins"]);
    viv_snapshot!(ctx, cmd);
}

/// #23: `--offline` with nothing in the store fails fast, naming every
/// missing package in one message rather than the first one a fetch
/// attempt happens to reach.
#[test]
fn install_offline_reports_every_missing_package_in_one_message() {
    let ctx = TestContext::new();
    copy_monolog_sources(ctx.project.path());
    let mut cmd = ctx.viv();
    cmd.args(["install", "--offline"]);
    viv_snapshot!(ctx, cmd);
}

/// `COMPOSER_DISABLE_NETWORK=1` is Composer's own spelling of the same
/// thing, and must behave identically to `--offline`.
#[test]
fn install_composer_disable_network_env_behaves_like_offline() {
    let ctx = TestContext::new();
    copy_monolog_sources(ctx.project.path());
    let mut cmd = ctx.viv();
    cmd.env("COMPOSER_DISABLE_NETWORK", "1");
    cmd.arg("install");
    viv_snapshot!(ctx, cmd);
}

#[test]
fn dump_autoload_without_a_lock_fails() {
    let ctx = TestContext::new();
    let mut cmd = ctx.viv();
    cmd.arg("dump-autoload");
    viv_snapshot!(ctx, cmd);
}

/// #36: `viv cache prune` removes a stale bucket and reports what it freed.
#[test]
fn cache_prune_reports_what_it_removed() {
    let ctx = TestContext::new();
    fs::create_dir_all(ctx.cache.path().join("archive-v0")).unwrap();
    fs::create_dir_all(ctx.cache.path().join("dists-v0")).unwrap();
    fs::create_dir_all(ctx.cache.path().join("old-bucket-v0")).unwrap();
    fs::write(ctx.cache.path().join("old-bucket-v0/stray"), "12345").unwrap();

    let mut cmd = ctx.viv();
    cmd.args(["cache", "prune"]);
    viv_snapshot!(ctx, cmd);
    assert!(!ctx.cache.path().join("old-bucket-v0").exists());
}

/// #36: `viv cache clean` removes the whole cache dir outright.
#[test]
fn cache_clean_removes_the_cache_dir() {
    let ctx = TestContext::new();
    fs::create_dir_all(ctx.cache.path().join("archive-v0")).unwrap();

    let mut cmd = ctx.viv();
    cmd.args(["cache", "clean"]);
    viv_snapshot!(ctx, cmd);
    assert!(!ctx.cache.path().exists());
}

/// #36: a directory that isn't a vivace cache is left alone.
#[test]
fn cache_clean_refuses_a_directory_with_foreign_content() {
    let ctx = TestContext::new();
    fs::write(ctx.cache.path().join("not-ours.txt"), "keep me").unwrap();

    let mut cmd = ctx.viv();
    cmd.args(["cache", "clean"]);
    viv_snapshot!(ctx, cmd);
    assert!(ctx.cache.path().join("not-ours.txt").exists());
}

/// Lay down one archive (`content`) and a dist pointer symlinking to it,
/// mirroring `Store`'s own layout, without needing a network fetch.
fn seed_archive(cache: &Path, content: &[u8]) -> PathBuf {
    let id = "deadbeefcafe";
    let archive_dir = cache.join("archive-v0").join(id);
    fs::create_dir_all(&archive_dir).unwrap();
    fs::write(archive_dir.join("file"), content).unwrap();
    fs::write(cache.join("archive-v0").join(format!("{id}.ok")), "files=1").unwrap();
    let pointer_dir = cache.join("dists-v0/acme/pkg");
    fs::create_dir_all(&pointer_dir).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&archive_dir, pointer_dir.join("ref")).unwrap();
    archive_dir
}

/// #20: `viv cache size` reports archive bytes, and archive/pointer counts.
#[test]
fn cache_size_reports_archives_and_pointers() {
    let ctx = TestContext::new();
    seed_archive(ctx.cache.path(), b"12345");

    let mut cmd = ctx.viv();
    cmd.args(["cache", "size"]);
    viv_snapshot!(ctx, cmd);
}

/// #20: `viv cache prune --older-than 0` removes every dist pointer (none
/// can be less than zero days old) and, with no pointer left referencing it,
/// the archive it pointed to.
#[test]
fn cache_prune_older_than_removes_the_pointer_and_its_orphaned_archive() {
    let ctx = TestContext::new();
    let archive_dir = seed_archive(ctx.cache.path(), b"12345");

    let mut cmd = ctx.viv();
    cmd.args(["cache", "prune", "--older-than", "0"]);
    cmd.assert().success();

    assert!(!ctx.cache.path().join("dists-v0/acme/pkg/ref").exists());
    assert!(!archive_dir.exists());
}

/// The fixture's `vendor/` is gitignored and only populated locally by
/// `make fixtures` (needs devbox composer), so skip instead of panicking
/// when it's missing, e.g. on a fresh CI checkout.
#[allow(clippy::print_stderr, reason = "test skip notice, not app logging")]
fn fixture_vendor_missing() -> bool {
    if fixture()
        .join("vendor/composer/installed.json")
        .try_exists()
        .unwrap_or(false)
    {
        return false;
    }
    eprintln!(
        "skipping: run `make fixtures` (needs devbox composer) to populate tests/fixtures/monolog/vendor"
    );
    true
}

/// Byte-compare every file under `expected` against the same relative path
/// under `actual`, collecting mismatches instead of failing on the first.
fn compare_tree(actual: &Path, expected: &Path, rel: &Path, mismatches: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(expected.join(rel)).unwrap() {
        let entry = entry.unwrap();
        let rel = rel.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            compare_tree(actual, expected, &rel, mismatches);
        } else {
            let want = fs::read(expected.join(&rel)).unwrap();
            let got = fs::read(actual.join(&rel)).unwrap_or_default();
            if want != got {
                mismatches.push(rel);
            }
        }
    }
}

#[test]
fn dump_autoload_matches_composer() {
    if fixture_vendor_missing() {
        return;
    }
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_monolog_sources(project);
    copy_tree(&fixture().join("vendor"), &project.join("vendor"));
    // Wipe what the fixture's own `composer install` produced, so a pass
    // proves `dump-autoload` regenerated it, not that it was already there.
    fs::remove_dir_all(project.join("vendor/composer")).unwrap();
    fs::remove_file(project.join("vendor/autoload.php")).unwrap();

    let mut cmd = ctx.viv();
    cmd.arg("dump-autoload");
    let output = cmd.output().expect("failed to run viv");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Generated autoload files\n"
    );
    let mut mismatches = Vec::new();
    compare_tree(
        &project.join("vendor"),
        &fixture().join("expected/dev"),
        Path::new(""),
        &mut mismatches,
    );
    assert!(
        mismatches.is_empty(),
        "files differing from composer (dev): {mismatches:?}"
    );

    let mut cmd = ctx.viv();
    cmd.args(["dump-autoload", "--no-dev"]);
    let output = cmd.output().expect("failed to run viv");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut mismatches = Vec::new();
    compare_tree(
        &project.join("vendor"),
        &fixture().join("expected/no-dev"),
        Path::new(""),
        &mut mismatches,
    );
    assert!(
        mismatches.is_empty(),
        "files differing from composer (no-dev): {mismatches:?}"
    );
}

/// `viv update-lock` (#86) delegates to the same function as `viv update
/// --lock`: both round-trip the monolog fixture's already-canonical lock
/// byte-identically.
#[test]
fn update_lock_matches_update_lock_flag() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    for name in ["composer.json", "composer.lock"] {
        fs::copy(fixture().join(name), project.join(name)).unwrap();
    }

    ctx.viv().arg("update-lock").assert().success();

    let want = fs::read_to_string(fixture().join("composer.lock")).unwrap();
    let got = fs::read_to_string(project.join("composer.lock")).unwrap();
    assert_eq!(got, want);
}
