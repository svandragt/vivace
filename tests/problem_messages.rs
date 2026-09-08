//! Resolver stage 5 follow-up (#62): `Problem.php`'s reason sort
//! (`getRulePriority`/`getSortableString`) and `formatDeduplicatedRules`
//! (version-range collapsing across near-identical `requires`/`conflicts`
//! lines), ported in `src/solver/problem.rs`. Two goldens, each captured
//! verbatim from a real `composer update --no-ansi` (Composer 2.10.2)
//! against a small hand-authored `composer`-type repository
//! (`tests/fixtures/solver-problems/`) and replayed offline, the same
//! `FixtureTransport`/recorded-metadata pattern `tests/solver.rs`'s
//! `unsatisfiable_root_require_names_the_package` and `tests/update.rs`'s
//! `unsatisfiable_root_require_matches_composers_message` already use.
//! `condenseVersionList`'s own `"..."` collapsing is exercised directly in
//! `src/solver/problem.rs`'s `#[cfg(test)]` module instead of here — see
//! `conflict_dedup_matches_composer`'s doc comment for why a live solve
//! rarely reaches it.

mod common;

use std::path::Path;

use common::FixtureTransport;
use serde_json::Value;
use vivace::repository::Repository;
use vivace::solver::{self, problem::SolverError};

#[tokio::test]
async fn dedup_requires_matches_composer() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/solver-problems"),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    // Both `acme/needy` versions require the same nonexistent package at
    // the same constraint, so `formatDeduplicatedRules` groups them into
    // one `acme/needy[1.0.0, 2.0.0] require ...` line (the "requires" ->
    // "require" depluralisation) — the `PackageRequires` counterpart of
    // `conflict_dedup_matches_composer`'s `PackageConflict` merge, and,
    // since the target "could not be found", the "Potential causes" hint
    // block too.
    let root: Value = serde_json::json!({
        "name": "vivace/fixture-dedup2",
        "require": {
            "acme/needy": "^1.0 || ^2.0"
        }
    });

    let Err(err) = solver::solve_update(&repo, &root, false, false).await else {
        panic!("expected an unsatisfiable request to fail")
    };
    let solver_error = err
        .downcast_ref::<SolverError>()
        .expect("solve_update's error is a SolverError for an unsatisfiable request");

    // Captured verbatim (`--no-ansi`) from `composer update` against a
    // local `composer`-type repository serving
    // `tests/fixtures/solver-problems`'s `packages.json`/`p2` tree,
    // Composer 2.10.2.
    let want = "Your requirements could not be resolved to an installable set of packages.\n\n  \
                Problem 1\n    - Root composer.json requires acme/needy ^1.0 || ^2.0 -> \
                satisfiable by acme/needy[1.0.0, 2.0.0].\n    - acme/needy[1.0.0, 2.0.0] \
                require acme/missing-thing ^1.0 -> could not be found in any version, there \
                may be a typo in the package name.\n\nPotential causes:\n - A typo in the \
                package name\n - The package is not available in a stable-enough version \
                according to your minimum-stability setting\n   see \
                <https://getcomposer.org/doc/04-schema.md#minimum-stability> for more details.\n \
                - It's a private package and you forgot to add a custom repository to find \
                it\n\nRead <https://getcomposer.org/doc/articles/troubleshooting.md> for further \
                common problems.\n";
    assert_eq!(format!("{solver_error}"), want);
}

#[tokio::test]
async fn conflict_dedup_matches_composer() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/solver-problems"),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    // Both `acme/core` versions conflict with the same `acme/plugin`
    // version, so `formatDeduplicatedRules` groups them into one
    // `acme/core[1.0.0, 2.0.0] conflict with ...` line — the "conflicts"
    // -> "conflict" depluralisation, and `RootRequire`'s priority-2 lines
    // sorting ahead of the priority-1 `PackageConflict` line.
    // `condenseVersionList`'s own `"..."` collapse (both here in a dedup
    // group's version list, and in `package_list`'s own `satisfiable
    // by`/`found` lists) is exercised directly against `Problem.php`'s
    // algorithm by `src/solver/problem.rs`'s own `#[cfg(test)]` module
    // instead of here: vivace's `DefaultPolicy` already proves a whole
    // major-version bucket unsatisfiable from one representative version
    // (a real efficiency win a CDCL solver can take that libsolv's own
    // search order here does not), so a bucket big enough to collapse
    // rarely survives into the *rendered* problem with every member
    // still present to golden against a live `composer update`.
    let root: Value = serde_json::json!({
        "name": "vivace/fixture-conflict",
        "require": {
            "acme/core": "^1.0 || ^2.0",
            "acme/plugin": "1.0.0"
        }
    });

    let Err(err) = solver::solve_update(&repo, &root, false, false).await else {
        panic!("expected an unsatisfiable request to fail")
    };
    let solver_error = err
        .downcast_ref::<SolverError>()
        .expect("solve_update's error is a SolverError for an unsatisfiable request");

    // Captured verbatim (`--no-ansi`) from `composer update` against a
    // local `composer`-type repository serving
    // `tests/fixtures/solver-problems`'s `packages.json`/`p2` tree,
    // Composer 2.10.2.
    let want = "Your requirements could not be resolved to an installable set of packages.\n\n  \
                Problem 1\n    - Root composer.json requires acme/core ^1.0 || ^2.0 -> \
                satisfiable by acme/core[1.0.0, 2.0.0].\n    - Root composer.json requires \
                acme/plugin 1.0.0 -> satisfiable by acme/plugin[1.0.0].\n    - \
                acme/core[1.0.0, 2.0.0] conflict with acme/plugin 1.0.0.\n";
    assert_eq!(format!("{solver_error}"), want);
}
