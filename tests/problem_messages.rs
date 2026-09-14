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
//!
//! #238's own golden below isn't a live-Composer capture the same way:
//! reproducing it against real Composer needs a repository that also
//! serves `security-advisories.api-url`, which this corpus's
//! `FixtureTransport` doesn't run a real `composer update` against. Its
//! `want` text is instead derived the same way the port itself was: the
//! sentence up to "affected by security advisories (...)" is
//! `Problem::getMissingPackageReason`'s own wording, verified against the
//! real capture `compat/results/v0.12.0.md` already has for that branch
//! (`reconnico/swark`'s `symfony/http-client` case). Everything after that
//! (the `--no-blocking`/`audit.ignore` remedy) is viv's own text, since viv
//! has no `policy.advisories.*` config for Composer's own closing sentence
//! to port.
//!
//! #152's root-conflict branch (`missing_package_suffix`'s
//! `root_conflict_suffix` call) has no golden here at all: it needs a
//! package name the root itself requires *and* a sibling that needs a
//! wider constraint on that same name, and viv's closure walk resolves a
//! root-level name's candidates from whatever constraints are known at
//! that name's own first fetch — a root require's own fetch always starts
//! before a transitive one can contribute a wider constraint for the same
//! name, so no fixture reaches a pool state root-conflict actually needs.
//! Same "solver explores fewer candidates than libsolv" gap #62 already
//! names for dedup, just for a different reason. Tested directly against
//! the function instead, in `src/solver/problem.rs`'s own `#[cfg(test)]`
//! module.

mod common;

use std::path::Path;
use std::time::Duration;

use common::FixtureTransport;
use serde_json::Value;
use vivace::repository::Repository;
use vivace::solver::pool_builder::AdvisoryFilter;
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

/// A fixture-served `security-advisories.api-url` response, the same
/// `AdvisoriesTransport` seam `tests/update.rs`'s own `AdvisoriesFixture`
/// uses; `tests/fixtures/solver-problems/packages.json` advertises exactly
/// this one endpoint.
struct AdvisoriesFixture(Value);

impl vivace::audit::AdvisoriesTransport for AdvisoriesFixture {
    #[allow(
        clippy::unused_async_trait_impl,
        reason = "the fixture reads an in-memory body synchronously; the trait is async for \
                  production (tests/update.rs's own AdvisoriesFixture does the same)"
    )]
    async fn post_advisories(
        &self,
        _url: &reqwest::Url,
        _packages: &[String],
    ) -> anyhow::Result<Value> {
        Ok(self.0.clone())
    }
}

fn advertised_endpoints() -> Vec<String> {
    vec!["https://packagist.org/api/security-advisories/".to_string()]
}

/// #238: every version matching a root require is advisory-blocked, so the
/// package disappears from the pool entirely — `Problem::pretty_string`'s
/// single-reason fast path used to read that as "never existed" and print
/// the typo suggestion this issue is about.
#[tokio::test]
async fn advisory_blocked_root_require_names_the_advisory_and_no_blocking() {
    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/solver-problems"),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let root: Value = serde_json::json!({
        "name": "vivace/fixture-advisory",
        "require": { "acme/vuln": "6.5.0" }
    });

    let advisories = AdvisoriesFixture(serde_json::json!({
        "advisories": {
            "acme/vuln": [{
                "advisoryId": "PKSA-test-0003",
                "packageName": "acme/vuln",
                "affectedVersions": ">=6.5.0,<6.5.1",
                "title": "fixture advisory covering acme/vuln 6.5.0",
                "cve": null,
                "link": null,
                "reportedAt": "2024-01-01 00:00:00",
            }],
        },
    }));
    let endpoints = advertised_endpoints();
    let filter = AdvisoryFilter {
        transport: &advisories,
        endpoints: &endpoints,
        audit: &vivace::lock::AuditConfig::default(),
        no_blocking: false,
        prefetched: None,
        cache_dir: None,
        metadata_ttl: Duration::ZERO,
    };

    let Err(err) = solver::solve_update_seeded(
        &repo,
        &root,
        false,
        false,
        &[],
        std::collections::HashMap::new(),
        Some(filter),
        None,
    )
    .await
    else {
        panic!("expected an advisory-blocked request to fail")
    };
    let solver_error = err
        .downcast_ref::<SolverError>()
        .expect("solve_update_seeded's error is a SolverError for an unsatisfiable request");

    let want = "Your requirements could not be resolved to an installable set of packages.\n\n  \
                Problem 1\n    - Root composer.json requires acme/vuln 6.5.0, found \
                acme/vuln[6.5.0] but these were not loaded, because they are affected by \
                security advisories (\"PKSA-test-0003\"). Go to \
                https://packagist.org/security-advisories/ to find advisory details. Require a \
                patched version to clear this, or override with --no-blocking (or add the \
                advisory to \"audit.ignore\") to install it anyway.\n";
    assert_eq!(format!("{solver_error}"), want);
}
