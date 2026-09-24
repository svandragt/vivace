//! #300: `viv install` refuses — before any fetch/plan/link work, exit `2`,
//! matching `Installer::ERROR_DEPENDENCY_RESOLUTION_FAILED` — when the lock
//! needs a `php`/`ext-*` requirement this host (or `config.platform`) can't
//! satisfy. Offline path-repository fixtures, `tests/platform_ignore.rs`'s
//! own style.
//!
//! `expected/stderr.txt` (`tests/fixtures/platform-check/`) is scoped to the
//! `SolverProblemsException`-shaped header and `Problem N` lines a real
//! `devbox run -- composer install --no-interaction` prints for this exact
//! fixture (captured verbatim, Composer 2.10.2) — not Composer's surrounding
//! CLI banner (`Installing dependencies from lock file...`, `Verifying lock
//! file contents...`) or its host-specific `.ini` file hint block
//! (`SolverProblemsException::createExtensionHint`, never ported —
//! `src/solver/problem.rs`'s own module doc names what stage 5 covers), same
//! scoping `tests/support/mod.rs`'s own doc comment already applies to
//! `--EXPECT-OUTPUT--`.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/platform-check")
}

fn php_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/platform-check-php")
}

fn copy_sources(from: &Path, project: &Path) {
    for name in ["composer.json", "composer.lock"] {
        let src = from.join(name);
        if src.is_file() {
            fs::copy(src, project.join(name)).unwrap();
        }
    }
    let packages = from.join("packages");
    if packages.is_dir() {
        copy_tree(&packages, &project.join("packages"));
    }
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

/// A root require and a locked package's own require, each naming an
/// extension no host has: both surface as their own numbered `Problem`,
/// vendor/ never gets written.
#[test]
fn install_refuses_a_missing_extension_root_and_locked_package() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_sources(&fixture(), project);

    let expected = fs::read(fixture().join("expected/stderr.txt")).unwrap();
    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .code(2)
        .stderr(expected);

    assert!(
        !project.join("vendor").exists(),
        "a refused install must not write vendor/ at all"
    );
}

/// `--ignore-platform-req` (with a `*` glob) drops the matching targets from
/// the check entirely, root and locked-package alike, so the same lock
/// installs cleanly.
#[test]
fn ignore_platform_req_wildcard_installs_despite_the_missing_extensions() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_sources(&fixture(), project);

    ctx.viv()
        .args(["install", "--ignore-platform-req=ext-viv-*"])
        .assert()
        .success();

    assert!(
        project.join("vendor/acme/hello").exists(),
        "ignoring both missing extensions must let the install proceed"
    );
}

/// `config.platform` is honoured, not just the live host: a `php` pinned
/// below the root require's own constraint fails the same way a real
/// mismatch would (the "php version case" from the four wordings #300
/// confirmed against `Problem.php`).
///
/// Real Composer's own message for an *overridden* `php` also names the
/// override (`"...(7.0.0; overridden via config.platform, actual: 8.4.24)
/// does not satisfy..."`, `Problem::getPlatformPackageVersion`) — this port
/// doesn't track "was this detected or overridden, and what was the raw
/// host value" through `cached_platform_packages`, so it prints the plainer
/// `"...(7.0.0) does not satisfy..."` #300's brief itself asked for; only
/// the parenthetical detail is missing, not the refusal.
#[test]
fn install_refuses_a_php_version_mismatch_from_config_platform() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_sources(&php_fixture(), project);

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains(
            "Root composer.json requires php >=8.0 but your php version (7.0.0) does not \
             satisfy that requirement.",
        ));

    assert!(
        !project.join("vendor").exists(),
        "a refused install must not write vendor/ at all"
    );
}

/// #300 follow-up: no `php` on `PATH` at all must not turn into a false
/// refusal — `cached_platform_packages`'s own "assume php 8.3.0, no
/// extensions" fallback is exactly the wrong assumption for a lock that
/// needs a real extension (`acme/hello` here), and `viv install` worked
/// without `php` installed before this check existed (a container build
/// stage producing `vendor/` ahead of its `php-fpm` layer is a real case).
/// `PATH` set to an empty temp dir, same as `tests/update.rs`'s own
/// `update_warns_once_when_no_php_binary_is_on_path` (unset PATH could still
/// resolve to a shell builtin/hash lookup on some platforms).
#[test]
fn install_without_php_on_path_still_installs() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_sources(&fixture(), project);
    let empty_path = tempfile::tempdir().unwrap();

    ctx.viv()
        .arg("install")
        .env("PATH", empty_path.path())
        .assert()
        .success();

    assert!(
        project.join("vendor/acme/hello").exists(),
        "no php on PATH must skip the check, not refuse the install"
    );
}
