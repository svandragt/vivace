//! `install` resolving a conflicted `composer.lock` straight from git's
//! index stages (#299): a developer who merges two branches before ever
//! running an install has no merge driver wired yet (#298 fixes that for
//! every clone *after* the first), so git leaves plain conflict markers.
//! `install` reads the three stages `git show :1:`/`:2:`/`:3:` exposes and
//! runs the same record merge and escalated re-solve `viv lock merge`
//! would, instead of pointing at `viv lock merge <base> <ours> <theirs>`
//! — files this developer never had. Offline throughout: real "path"
//! packages (`tests/fixtures/path`) for the resolvable case, an inline
//! `"package"`-repository declaration (no dist, never fetched: the merge
//! fails before `install` gets that far) for the unsatisfiable one.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::TestContext;
use predicates::prelude::*;

fn path_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/path")
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

/// `git` in `dir`, with a fixed identity and signing disabled
/// (`tests/root_version.rs`'s own pattern), through the env-scrubbed
/// constructor so a `pre-commit` hook's inherited `GIT_DIR` can't point
/// this throwaway repo's commits at the real one.
fn git(dir: &Path, args: &[&str]) {
    let status = common::git_command()
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "vivace")
        .env("GIT_AUTHOR_EMAIL", "vivace@example.com")
        .env("GIT_COMMITTER_NAME", "vivace")
        .env("GIT_COMMITTER_EMAIL", "vivace@example.com")
        .status()
        .unwrap();
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn git_init(project: &Path) {
    git(project, &["init", "-q", "-b", "main"]);
    git(project, &["config", "commit.gpgsign", "false"]);
}

fn commit_all(project: &Path, message: &str) {
    git(project, &["add", "-A"]);
    git(project, &["commit", "-q", "-m", message]);
}

fn configured_driver(project: &Path) -> Option<String> {
    let output = common::git_command()
        .args(["config", "--local", "--get", "merge.viv.driver"])
        .current_dir(project)
        .output()
        .expect("git config --get");
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Base for the resolvable scenario: `acme/hello` required from the start
/// (a real "path" package, `tests/fixtures/path/packages/hello`) so the
/// two branches' additions land at the *ends* of an already non-empty
/// `require`/`packages` array (`docs/research.md` chapter 1's own
/// "positional structure" note) rather than both replacing an empty `{}`,
/// which git auto-merges cleanly, keeping `composer.json` conflict-free —
/// `composer.lock`'s own `content-hash` still collides, since both
/// branches touch that single line differently, which is enough on its
/// own to leave `composer.lock` mid-merge.
const RESOLVABLE_COMPOSER_JSON: &str = r#"{
    "name": "vivace/fixture-index-merge",
    "repositories": [
        {"packagist.org": false},
        {"type": "path", "url": "packages/hello", "options": {"versions": {"acme/hello": "1.0.0"}}},
        {"type": "path", "url": "packages/testkit", "options": {"versions": {"acme/testkit": "1.0.0"}}}
    ],
    "require": {
        "acme/hello": "1.0.0",
        "acme/testkit": "1.0.0"
    }
}
"#;

fn resolvable_lock(content_hash: &str, with_testkit: bool) -> String {
    let testkit_entry = r#",
        {
            "name": "acme/testkit",
            "version": "1.0.0",
            "dist": {
                "type": "path",
                "url": "packages/testkit",
                "reference": "78ff73f1cc37d92c843162614d0a6c8663398bcc"
            },
            "type": "library",
            "autoload": {
                "psr-4": {
                    "Acme\\Testkit\\": "src/"
                }
            },
            "transport-options": {
                "relative": true
            }
        }"#;
    format!(
        r#"{{
    "_readme": [
        "This file locks the dependencies of your project to a known state",
        "Read more about it at https://getcomposer.org/doc/01-basic-usage.md#installing-dependencies",
        "This file is @generated automatically"
    ],
    "content-hash": "{content_hash}",
    "packages": [
        {{
            "name": "acme/hello",
            "version": "1.0.0",
            "dist": {{
                "type": "path",
                "url": "packages/hello",
                "reference": "f25f249178304a977332e49488b49798b49c3092"
            }},
            "type": "library",
            "autoload": {{
                "psr-4": {{
                    "Acme\\Hello\\": "src/"
                }}
            }},
            "transport-options": {{
                "relative": true
            }}
        }}{}
    ],
    "packages-dev": [],
    "aliases": [],
    "minimum-stability": "stable",
    "stability-flags": {{}},
    "prefer-stable": false,
    "prefer-lowest": false,
    "platform": {{}},
    "platform-dev": {{}},
    "plugin-api-version": "2.9.0"
}}
"#,
        if with_testkit { testkit_entry } else { "" }
    )
}

/// A fresh clone that never wired the driver, whose `git merge` left plain
/// markers in `composer.lock` (one branch added `acme/testkit`, the other
/// changed nothing else): `install` resolves it from the index stages,
/// installs both packages, and wires the clone so the next merge doesn't
/// repeat the trip.
#[test]
fn git_merge_resolves_a_conflicted_composer_lock() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    copy_tree(
        &path_fixture().join("packages/hello"),
        &project.join("packages/hello"),
    );
    copy_tree(
        &path_fixture().join("packages/testkit"),
        &project.join("packages/testkit"),
    );
    fs::write(project.join("composer.json"), RESOLVABLE_COMPOSER_JSON).unwrap();
    fs::write(
        project.join("composer.lock"),
        resolvable_lock("base-hash", false),
    )
    .unwrap();
    git_init(project);
    commit_all(project, "base");

    git(project, &["checkout", "-q", "-b", "with-testkit"]);
    fs::write(
        project.join("composer.lock"),
        resolvable_lock("with-testkit-hash", true),
    )
    .unwrap();
    commit_all(project, "add testkit");

    git(project, &["checkout", "-q", "main"]);
    fs::write(
        project.join("composer.lock"),
        resolvable_lock("main-hash", false),
    )
    .unwrap();
    commit_all(project, "touch the lock without adding anything");

    let merge_status = common::git_command()
        .args(["merge", "with-testkit"])
        .current_dir(project)
        .status()
        .unwrap();
    assert!(
        !merge_status.success(),
        "the merge must leave conflict markers"
    );
    assert!(
        fs::read_to_string(project.join("composer.lock"))
            .unwrap()
            .contains("<<<<<<<"),
        "composer.lock must be left mid-merge"
    );

    ctx.viv()
        .arg("install")
        .assert()
        .success()
        .stderr(predicates::str::contains(
            "viv: set merge.viv.driver in this clone",
        ));

    let lock = fs::read_to_string(project.join("composer.lock")).unwrap();
    assert!(
        !lock.contains("<<<<<<<"),
        "the lock must be resolved, not left with markers"
    );
    assert!(project.join("vendor/acme/hello").is_dir());
    assert!(project.join("vendor/acme/testkit").is_dir());
    assert_eq!(
        configured_driver(project).as_deref(),
        Some("viv lock merge %O %A %B")
    );
    assert_eq!(
        fs::read_to_string(project.join(".gitattributes")).unwrap(),
        "composer.lock merge=viv\n"
    );
}

const UNSATISFIABLE_COMPOSER_JSON_BASE: &str = r#"{
    "name": "vivace/fixture-index-merge-unsat",
    "repositories": [
        {"packagist.org": false},
        {"type": "package", "package": [
            {"name": "acme/anchor", "version": "1.0.0"},
            {"name": "acme/hello", "version": "1.0.0"}
        ]}
    ],
    "require": {
        "acme/anchor": "1.0.0",
        "acme/hello": "1.0.0"
    }
}
"#;

/// Only `acme/hello`'s own requirement changes, to a constraint the inline
/// `"package"` repository (fixed at `1.0.0`, no second version declared)
/// can never satisfy — `acme/anchor`'s line is untouched, so `composer.json`
/// still merges without a single marker.
const UNSATISFIABLE_COMPOSER_JSON_BRANCH: &str = r#"{
    "name": "vivace/fixture-index-merge-unsat",
    "repositories": [
        {"packagist.org": false},
        {"type": "package", "package": [
            {"name": "acme/anchor", "version": "1.0.0"},
            {"name": "acme/hello", "version": "1.0.0"}
        ]}
    ],
    "require": {
        "acme/anchor": "1.0.0",
        "acme/hello": "^2.0"
    }
}
"#;

/// `acme/anchor` never changes across the three commits — it's the
/// `packages` array's clean anchor `lock_merge::splice_array` needs to
/// find its `"packages": [` line by (a lock where *every* record is
/// divergent has nothing to splice around, a documented ceiling, not
/// this test's own concern). `acme/hello`'s `version` is the one line
/// both branches change, to different values: divergent by
/// `lock_merge::merge`'s own three-way identity comparison, regardless of
/// what either value means.
fn unsatisfiable_lock(content_hash: &str, hello_version: &str) -> String {
    format!(
        r#"{{
    "_readme": [
        "This file locks the dependencies of your project to a known state",
        "Read more about it at https://getcomposer.org/doc/01-basic-usage.md#installing-dependencies",
        "This file is @generated automatically"
    ],
    "content-hash": "{content_hash}",
    "packages": [
        {{
            "name": "acme/anchor",
            "version": "1.0.0",
            "type": "library"
        }},
        {{
            "name": "acme/hello",
            "version": "{hello_version}",
            "type": "library"
        }}
    ],
    "packages-dev": [],
    "aliases": [],
    "minimum-stability": "stable",
    "stability-flags": {{}},
    "prefer-stable": false,
    "prefer-lowest": false,
    "platform": {{}},
    "platform-dev": {{}},
    "plugin-api-version": "2.9.0"
}}
"#
    )
}

/// A merge that resolves `composer.json` cleanly but leaves a merged
/// manifest the divergent name can never satisfy: `install` exits 1 with
/// markers for that one name, and — unlike #274's plain message —
/// without the `viv lock merge <base> <ours> <theirs>` line, since this
/// developer has none of those files. No driver gets wired on a failed
/// resolve: nothing here should change until the developer fixes the
/// manifest.
#[test]
fn git_merge_with_an_unsatisfiable_manifest_leaves_markers_and_no_file_advice() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::write(
        project.join("composer.json"),
        UNSATISFIABLE_COMPOSER_JSON_BASE,
    )
    .unwrap();
    fs::write(
        project.join("composer.lock"),
        unsatisfiable_lock("base-hash", "1.0.0"),
    )
    .unwrap();
    git_init(project);
    commit_all(project, "base");

    git(project, &["checkout", "-q", "-b", "bump-hello"]);
    fs::write(
        project.join("composer.json"),
        UNSATISFIABLE_COMPOSER_JSON_BRANCH,
    )
    .unwrap();
    fs::write(
        project.join("composer.lock"),
        unsatisfiable_lock("branch-hash", "2.0.0"),
    )
    .unwrap();
    commit_all(project, "bump hello's constraint");

    git(project, &["checkout", "-q", "main"]);
    fs::write(
        project.join("composer.lock"),
        unsatisfiable_lock("main-hash", "1.5.0"),
    )
    .unwrap();
    commit_all(project, "touch the lock without touching the manifest");

    let merge_status = common::git_command()
        .args(["merge", "bump-hello"])
        .current_dir(project)
        .status()
        .unwrap();
    assert!(
        !merge_status.success(),
        "the merge must leave conflict markers"
    );
    assert!(
        !fs::read_to_string(project.join("composer.json"))
            .unwrap()
            .contains("<<<<<<<"),
        "composer.json must merge clean"
    );

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains("packages: acme/hello"))
        .stderr(predicates::str::contains(
            "resolve the markers by hand, then retry",
        ))
        .stderr(predicates::str::contains("viv lock merge <base> <ours> <theirs>").not());

    assert!(
        fs::read_to_string(project.join("composer.lock"))
            .unwrap()
            .contains("<<<<<<< ours"),
        "the lock is left holding markers for the name that couldn't resolve"
    );
    assert!(
        !project.join(".gitattributes").exists(),
        "a failed resolve must not wire the driver"
    );
    assert_eq!(configured_driver(project), None);
}

/// #274's own behaviour, unchanged, when there is no git repository at
/// all to read index stages from — `install`'s only clue that a parse
/// failure might be a conflicted merge is `project_dir` being a git
/// checkout, so a marker pasted into a plain directory (no `git init`,
/// `tests/install_markers.rs`'s own way of manufacturing one) keeps
/// naming `viv lock merge <base> <ours> <theirs>`.
#[test]
fn markers_with_no_git_repository_keep_274s_advice() {
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs::write(project.join("composer.json"), RESOLVABLE_COMPOSER_JSON).unwrap();
    // The marker itself must start a fresh line at column 1, the way git's
    // own output always does (`tests/install_markers.rs`'s own splice):
    // the opening brace's newline is part of the search text so it isn't
    // left indenting `<<<<<<<` itself.
    let lock = resolvable_lock("base-hash", false).replacen(
        "        {\n            \"name\": \"acme/hello\",\n            \"version\": \"1.0.0\",\n",
        "        {\n<<<<<<< ours\n            \"name\": \"acme/hello\",\n            \"version\": \"1.0.0\",\n=======\n            \"name\": \"acme/hello\",\n            \"version\": \"2.0.0\",\n>>>>>>> theirs\n",
        1,
    );
    fs::write(project.join("composer.lock"), lock).unwrap();

    ctx.viv()
        .arg("install")
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "run `viv lock merge <base> <ours> <theirs>` or resolve the markers by hand, then retry",
        ));

    assert!(!project.join("vendor").exists());
}
