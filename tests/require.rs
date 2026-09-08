//! Resolver stage 5 (`docs/resolver-design.md`, composer/composer#42):
//! `viv require`'s constraint synthesis and `composer.json` edit,
//! byte-diffed against a real Composer run (through `viv normalize`, since
//! #145: `add`/`rm` always normalize now).
//!
//! `tests/fixtures/require-psr-container/composer.lock` and
//! `composer.json.after` were recorded by running real Composer 2.10.2
//! against `tests/fixtures/packagist/repo.packagist.org` as a local
//! `file://` `composer`-type repository (`composer.json.before`'s
//! `repositories` block), not live Packagist: this keeps the recording
//! reproducible from the same fixtures `tests/update.rs` already commits,
//! with no extra network fixtures to maintain and no drift risk. The
//! command was:
//!
//! ```sh
//! composer -d <scratch> require psr/container --no-interaction --no-plugins --no-scripts --no-install
//! ```
//!
//! The hermetic tests below never invoke Composer or the network at all:
//! they drive `vivace::solver`/`vivace::lock_writer` directly (same shape
//! as `tests/update.rs`'s `update_lock`), replaying the same recorded
//! Packagist metadata through `FixtureTransport`.
#![allow(
    clippy::print_stderr,
    reason = "skip messages are the point of this test, not a lint violation"
)]

mod common;

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use common::{FixtureTransport, TestContext, fixtures_root};
use serde_json::{Map, Value};
use vivace::repository::Repository;
use vivace::solver::{self, pool_builder::UpdateAllowMode};

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/require-psr-container")
}

/// `composer.lock`'s `content-hash` is `content_hash` of `composer.json`
/// (`src/lock.rs`), which includes the `require`/`require-dev` objects'
/// *own* key order. #145: `add`/`rm` now always normalize `composer.json`
/// (sorting those sections), so the on-disk file — and its content-hash —
/// no longer matches Composer's own unnormalized `JsonManipulator` edit
/// that `composer.lock` fixtures here were recorded against. Every other
/// field is still expected to match byte for byte.
fn lock_without_content_hash(path: &Path) -> Value {
    let mut lock: Value = serde_json::from_slice(&fs_err::read(path).unwrap()).unwrap();
    if let Some(obj) = lock.as_object_mut() {
        obj.remove("content-hash");
    }
    lock
}

fn assert_matches_expected(got: &str, expected_path: &Path) {
    let want = fs_err::read_to_string(expected_path).unwrap();
    assert_eq!(
        got,
        want,
        "{} does not match viv require's output",
        expected_path.display()
    );
}

/// `viv require psr/container` on the monolog fixture (moved out of
/// `require-dev`, since the fixture already declares it there, matching
/// real Composer's own "present in the other key" move): the partial
/// update this stage adds must reproduce Composer's real `composer.lock`
/// byte for byte, given the already-edited `composer.json`.
#[tokio::test]
async fn require_reproduces_composers_lock() {
    let fixture = fixture_dir();
    let composer_json = fs_err::read(fixture.join("composer.json.after")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let locked_by_name = common::locked_by_name(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
    );

    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let result = solver::solve_partial_update(
        &repo,
        &root,
        false,
        false,
        &locked_by_name,
        &["psr/container".to_string()],
        UpdateAllowMode::OnlyListed,
    )
    .await
    .unwrap();

    let options = vivace::lock_writer::LockOptions {
        minimum_stability: result.minimum_stability,
        stability_flags: &result.stability_flags,
        prefer_stable: result.prefer_stable,
        prefer_lowest: result.prefer_lowest,
        platform_reqs: &result.platform_reqs,
        platform_dev_reqs: &result.platform_dev_reqs,
        platform_overrides: &result.platform_overrides,
        aliases: &result.aliases,
    };
    let got =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// `viv remove psr/container --dev` on the monolog fixture: recorded the
/// same way as `require_reproduces_composers_lock`
/// (`tests/fixtures/remove-psr-container`), `composer remove ... --dev`
/// against the local `file://` mirror.
#[tokio::test]
async fn remove_reproduces_composers_lock() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/remove-psr-container");
    let composer_json = fs_err::read(fixture.join("composer.json.after")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let locked_by_name = common::locked_by_name(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
    );

    let cache = tempfile::tempdir().unwrap();
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", cache.path(), &transport)
        .await
        .unwrap();

    let result = solver::solve_partial_update(
        &repo,
        &root,
        false,
        false,
        &locked_by_name,
        &["psr/container".to_string()],
        UpdateAllowMode::WithTransitiveDepsNoRootRequire,
    )
    .await
    .unwrap();

    let options = vivace::lock_writer::LockOptions {
        minimum_stability: result.minimum_stability,
        stability_flags: &result.stability_flags,
        prefer_stable: result.prefer_stable,
        prefer_lowest: result.prefer_lowest,
        platform_reqs: &result.platform_reqs,
        platform_dev_reqs: &result.platform_dev_reqs,
        platform_overrides: &result.platform_overrides,
        aliases: &result.aliases,
    };
    let got =
        vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
            .unwrap();
    assert_matches_expected(&got, &fixture.join("composer.lock"));
}

/// End-to-end: the real `viv require` binary, then Composer's own
/// `validate`/`install --dry-run`. Gated on `VIVACE_TEST_NETWORK=1`
/// (`tests/update.rs`'s own pattern) so a bare `cargo nextest run` stays
/// offline; unlike that test, this one still resolves against the local
/// `file://` mirror (`composer.json.before`'s `repositories` override, see
/// the module doc) rather than live Packagist, so it is not actually
/// network-dependent — only gated the same way for consistency and because
/// it shells out to the real `composer`/`viv` binaries rather than calling
/// the library directly.
#[test]
fn viv_require_matches_composer_and_validates() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv_require_matches_composer_and_validates: set VIVACE_TEST_NETWORK=1 to run"
        );
        return;
    }
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!("skipping viv_require_matches_composer_and_validates: composer is not on PATH");
        return;
    }

    let fixture = fixture_dir();
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs_err::copy(
        fixture.join("composer.json.before"),
        project.join("composer.json"),
    )
    .unwrap();
    fs_err::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
        project.join("composer.lock"),
    )
    .unwrap();

    // #145: `require`/`add` always normalize `composer.json` now
    // (`--no-normalize` is a deprecated no-op, same shape as
    // `install`/`dump-autoload`'s), so the written file is compared against
    // `viv normalize`'s own output of Composer's recorded edit
    // (`composer.json.after`), not Composer's own unnormalized
    // `JsonManipulator` formatting. `--no-install` matches the recorded
    // command itself (see the module doc) and keeps this test scoped to the
    // `composer.json`/lock edit, not a real dist fetch (#104: `viv require`
    // installs by default now).
    ctx.viv()
        .args(["require", "psr/container", "--no-install"])
        .assert()
        .success();

    let got_json = fs_err::read_to_string(project.join("composer.json")).unwrap();

    let want_normalized_ctx = TestContext::new();
    let want_normalized_path = want_normalized_ctx.project.path().join("composer.json");
    fs_err::copy(fixture.join("composer.json.after"), &want_normalized_path).unwrap();
    want_normalized_ctx
        .viv()
        .arg("normalize")
        .assert()
        .success();
    let want_normalized = fs_err::read_to_string(&want_normalized_path).unwrap();
    assert_eq!(
        got_json, want_normalized,
        "viv require's composer.json edit differs"
    );

    assert_eq!(
        lock_without_content_hash(&project.join("composer.lock")),
        lock_without_content_hash(&fixture.join("composer.lock")),
        "viv require's lock differs"
    );

    let validate = Command::new("composer")
        .args(["validate", "--strict", "--no-check-publish"])
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "composer validate --strict failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stderr)
    );
}

/// End-to-end: the real `viv remove` binary, same shape as
/// `viv_require_matches_composer_and_validates`.
#[test]
fn viv_remove_matches_composer_and_validates() {
    if std::env::var("VIVACE_TEST_NETWORK").as_deref() != Ok("1") {
        eprintln!(
            "skipping viv_remove_matches_composer_and_validates: set VIVACE_TEST_NETWORK=1 to run"
        );
        return;
    }
    if Command::new("composer").arg("--version").output().is_err() {
        eprintln!("skipping viv_remove_matches_composer_and_validates: composer is not on PATH");
        return;
    }

    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/remove-psr-container");
    let ctx = TestContext::new();
    let project = ctx.project.path();
    fs_err::copy(
        fixture.join("composer.json.before"),
        project.join("composer.json"),
    )
    .unwrap();
    fs_err::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/composer.lock"),
        project.join("composer.lock"),
    )
    .unwrap();

    // See `viv_require_matches_composer_and_validates`'s comment: #145
    // means `remove`/`rm` always normalize now, so the written file is
    // compared against `viv normalize`'s own output of Composer's recorded
    // edit. `--no-install` keeps this scoped to the edit rather than a real
    // dist fetch (#104: `viv remove` installs by default now).
    ctx.viv()
        .args(["remove", "psr/container", "--dev", "--no-install"])
        .assert()
        .success();

    let got_json = fs_err::read_to_string(project.join("composer.json")).unwrap();

    let want_normalized_ctx = TestContext::new();
    let want_normalized_path = want_normalized_ctx.project.path().join("composer.json");
    fs_err::copy(fixture.join("composer.json.after"), &want_normalized_path).unwrap();
    want_normalized_ctx
        .viv()
        .arg("normalize")
        .assert()
        .success();
    let want_normalized = fs_err::read_to_string(&want_normalized_path).unwrap();
    assert_eq!(
        got_json, want_normalized,
        "viv remove's composer.json edit differs"
    );

    assert_eq!(
        lock_without_content_hash(&project.join("composer.lock")),
        lock_without_content_hash(&fixture.join("composer.lock")),
        "viv remove's lock differs"
    );

    let validate = Command::new("composer")
        .args(["validate", "--strict", "--no-check-publish"])
        .current_dir(project)
        .output()
        .unwrap();
    assert!(
        validate.status.success(),
        "composer validate --strict failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&validate.stdout),
        String::from_utf8_lossy(&validate.stderr)
    );
}

/// Same recursive copy as `tests/update.rs`'s helper of the same name:
/// the monolog fixture's `src`/`lib` autoload sources.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// A single-file zip, in memory, for pre-populating the store without ever
/// touching the network. Duplicated from `tests/update.rs`'s copy of the
/// same name (private there, and small enough not to widen for one more
/// caller).
fn zip_of_one_file(name: &str, content: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file(name, zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(content).unwrap();
    writer.finish().unwrap().into_inner()
}

/// #158: pre-warms `ctx.cache`'s on-disk repository cache from the recorded
/// Packagist fixtures (`solver::solve_update`'s call here is only for that
/// side effect, same pattern as `tests/update.rs`'s
/// `offline_partial_update_context`) and pre-populates the store with a
/// synthetic archive for every package the monolog fixture's own
/// `composer.lock` already proves this solve converges to, so the real
/// `viv` binary can run entirely offline afterwards.
async fn warm_monolog_cache_and_store(ctx: &TestContext, fixture: &Path) {
    let transport = FixtureTransport {
        root: fixtures_root(),
    };
    let repo = Repository::load("https://repo.packagist.org", ctx.cache.path(), &transport)
        .await
        .unwrap();
    let root: Value =
        serde_json::from_slice(&fs_err::read(fixture.join("composer.json")).unwrap()).unwrap();
    solver::solve_update(&repo, &root, false, false)
        .await
        .unwrap();

    let store = vivace::store::Store::open(ctx.cache.path()).unwrap();
    let lock = vivace::lock::read_lock(&fixture.join("composer.lock")).unwrap();
    for package in lock.packages(true) {
        store
            .add_zip(
                package,
                &zip_of_one_file("marker.txt", package.name.as_bytes()),
            )
            .unwrap();
    }
}

/// #155/#158: `viv add`'s written `composer.lock` must have a `content-hash`
/// describing the `composer.json` bytes actually left on disk, not the
/// unnormalised bytes read before `write_composer_json`/`maybe_normalize`
/// ran. A deliberately unnormalised `require` (reverse key order, so the
/// normaliser's platform-first sort actually moves something) must still
/// leave a fresh lock behind, so the chained install prints no stale-lock
/// warning. Runs entirely offline now that `require::partial_update` builds
/// its repository set the same way `update::solve` does and honours
/// `--offline` (`#158`): a warm cache (`warm_monolog_cache_and_store`)
/// stands in for live Packagist, the same recorded-fixture pattern
/// `tests/update.rs`'s own #155 regression already uses. An explicit
/// `:^2.0` constraint sidesteps `synthesize_constraint`, which still
/// resolves against a hard-coded Packagist URL and ignores `--offline`
/// (out of `#158`'s scope; tracked separately).
#[tokio::test]
async fn viv_add_normalizes_composer_json_before_computing_the_content_hash() {
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");
    let ctx = TestContext::new();
    let project = ctx.project.path();

    warm_monolog_cache_and_store(&ctx, &fixture).await;

    let original: Value =
        serde_json::from_slice(&fs_err::read(fixture.join("composer.json")).unwrap()).unwrap();
    let mut require_reversed = Map::new();
    for (name, constraint) in original["require"].as_object().unwrap().iter().rev() {
        require_reversed.insert(name.clone(), constraint.clone());
    }
    let unnormalized = serde_json::json!({
        "require": require_reversed,
        "name": original["name"],
        "license": original["license"],
        "type": original["type"],
        "require-dev": original["require-dev"],
        "autoload": original["autoload"],
        "config": original["config"],
    });
    fs_err::write(
        project.join("composer.json"),
        serde_json::to_vec(&unnormalized).unwrap(),
    )
    .unwrap();
    for dir in ["src", "lib"] {
        copy_tree(&fixture.join(dir), &project.join(dir));
    }

    let output = ctx
        .viv()
        .args(["require", "psr/container:^2.0", "--offline"])
        .output()
        .unwrap();
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "viv require failed:\n{combined}");
    assert!(
        !combined.contains("is not up to date"),
        "the chained install must not see a stale lock: {combined}"
    );

    let lock = vivace::lock::read_lock(&project.join("composer.lock")).unwrap();
    let on_disk_composer_json = fs_err::read(project.join("composer.json")).unwrap();
    assert!(
        vivace::lock::is_fresh(&lock, &on_disk_composer_json).unwrap(),
        "content-hash should describe the normalized composer.json actually on disk"
    );
}

/// #158: `require::partial_update` must build its repository set from
/// `composer.json`'s own `repositories` (`#67`), not a hard-coded
/// `https://repo.packagist.org` — a `composer`-type repository at a
/// different host than Packagist, with the default disabled, resolves `viv
/// add` fine offline, and would error (the default is disabled, and the
/// warm cache below never touches `repo.packagist.org`) if the fix
/// regressed to the old hard-coded lookup.
///
/// `tests/fixtures/require-satis/repo` is a minimal `composer`-type
/// repository of its own (a relative `metadata-url`, like a real Satis or
/// Private Packagist instance, unlike `repo.packagist.org`'s own recorded
/// `packages.json`, whose `metadata-url` is absolute and would resolve back
/// to Packagist regardless of which host declared it): `p2/psr/log.json`
/// and `p2/psr/container.json` are verbatim copies of the same recorded
/// Packagist provider files `tests/fixtures/packagist/repo.packagist.org`
/// already commits.
#[tokio::test]
async fn viv_add_resolves_from_a_composer_type_repository() {
    let ctx = TestContext::new();
    let project = ctx.project.path();

    let repo_url = "https://satis.example.test";
    let transport = FixtureTransport {
        root: Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/require-satis/repo"),
    };
    let repo = Repository::load(repo_url, ctx.cache.path(), &transport)
        .await
        .unwrap();
    let root = serde_json::json!({
        "name": "vivace/fixture-satis",
        "license": "proprietary",
        "type": "project",
        "require": {"psr/log": "^3.0", "psr/container": "^2.0"},
        "repositories": [
            {"type": "composer", "url": repo_url},
            {"packagist.org": false},
        ],
    });
    // Only for its side effect of warming `ctx.cache`'s on-disk cache for
    // both packages (`offline_partial_update_context`'s own pattern in
    // `tests/update.rs`): the actual lock this test cares about is the one
    // the real `viv add` binary below writes, not this one.
    solver::solve_update(&repo, &root, false, false)
        .await
        .unwrap();

    let project_root = serde_json::json!({
        "name": "vivace/fixture-satis",
        "license": "proprietary",
        "type": "project",
        "require": {"psr/log": "^3.0"},
        "repositories": [
            {"type": "composer", "url": repo_url},
            {"packagist.org": false},
        ],
    });
    fs_err::write(
        project.join("composer.json"),
        serde_json::to_vec_pretty(&project_root).unwrap(),
    )
    .unwrap();

    ctx.viv()
        .args(["require", "psr/container:^2.0", "--offline", "--no-install"])
        .assert()
        .success();

    let lock: Value =
        serde_json::from_slice(&fs_err::read(project.join("composer.lock")).unwrap()).unwrap();
    let names: Vec<&str> = lock["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"psr/container") && names.contains(&"psr/log"),
        "expected psr/log and psr/container in the lock resolved from the composer-type \
         repository, got {names:?}"
    );
}
