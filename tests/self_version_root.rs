//! `#304`: `self.version` in the *root* `composer.json`'s own `require`
//! (`#115` already covers a dependency's own require of `self.version` via
//! the closure walk; the root require hit the same literal-string parse
//! failure, untouched by that fix). Offline, no-network fixtures, same
//! `"package"`-type repository and in-process solve/write as
//! `tests/package_repository.rs`.
//!
//! `default-version/composer.lock` was captured with `devbox run --
//! composer -d <copy outside any git checkout> update --no-install`: run
//! inside this repo, Composer's `VersionGuesser` would find the enclosing
//! vivace checkout's own tags and guess a version from those instead of
//! falling back to `1.0.0`, so it was generated from a copy in a directory
//! with no `.git` above it. Regenerate the same way if this fixture's
//! `composer.lock` ever needs to change.
//!
//! `#312`: the solve itself never called `vcs::guess_root_version` (only
//! `install.rs`'s `installed.php` write did), so a root `self.version`
//! require resolved against the `1.0.0` default even inside a real git
//! checkout. `git-tag/`'s and `git-branch/`'s own `composer.lock` goldens
//! were captured the same `devbox run -- composer update --no-install` way,
//! from a throwaway git repo built the same way [`update_lock_in_git_repo`]
//! builds one at test time (`git tag`/`git branch` name a version, never a
//! commit hash, so the golden stays reproducible despite a fresh commit
//! every run — `tests/root_version.rs`'s own `init_repo`/`git` pattern,
//! reused here rather than a third copy).

mod common;

use std::path::Path;

use common::git_command;
use reqwest::Url;
use serde_json::Value;
use tokio::sync::Mutex;
use vivace::fetch::Conditional;
use vivace::repository::{Repository, Transport};

/// Serialises every test here that can observe `COMPOSER_ROOT_VERSION`:
/// `composer_root_version_env_wins_over_the_guess` mutates process env,
/// which `root_pretty_version`'s own `std::env::var` read lets any other
/// test in this file see mid-flight too, since cargo test runs a file's
/// tests as threads sharing one process (nextest gives each test its own
/// process and never needs this lock). Only tests whose fixture has no
/// explicit root `version` are ever affected — the others short-circuit
/// before `root_pretty_version` would check the env var at all. `tokio::sync`
/// rather than `std::sync`, since the guard is held across each test's own
/// `.await` points and clippy's `await_holding_lock` flags a `std::sync`
/// guard there.
static ENV_LOCK: Mutex<()> = Mutex::const_new(());

fn fixture(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/self-version-root")
        .join(name)
}

/// Same panic-on-any-request transport as `tests/package_repository.rs`: a
/// `"package"`-type repository set never fetches anything.
struct PanicTransport;

impl Transport for &PanicTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &Url,
        _if_modified_since: Option<&str>,
    ) -> anyhow::Result<Conditional> {
        panic!(
            "unexpected HTTP request to {url}: a package-only repository set should never need it"
        );
    }
}

/// Solves `name`'s `composer.json` and writes the lock exactly like `viv
/// update --no-install` would.
async fn update_lock(name: &str) -> String {
    let fixture = fixture(name);
    let composer_json = fs_err::read(fixture.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let cache = tempfile::tempdir().unwrap();
    let transport = PanicTransport;
    let repo = Repository::from_composer_json(&fixture, &root, cache.path(), &transport)
        .await
        .unwrap();

    let result = vivace::solver::solve_update(&repo, &root, &fixture, false, false)
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
    vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
        .unwrap()
}

/// `git` in `dir`, with a fixed identity and signing disabled — matching
/// `tests/root_version.rs`'s own helper of the same name and for the same
/// reasons (a hermetic commit/tag regardless of the host's global
/// `commit.gpgsign`/`tag.gpgsign`, or whether `user.name`/`user.email` are
/// configured at all).
fn git(dir: &Path, args: &[&str]) {
    let status = git_command()
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

/// Same solve/write as [`update_lock`], but `name`'s `composer.json` is
/// copied into a fresh git checkout instead of read straight from
/// `tests/fixtures/`: `root_pretty_version`'s guess needs a real `.git` to
/// find (`vcs::guess_root_version`'s own precheck), which a checked-in
/// fixture directory can never carry (`tests/fixtures/self-version-root/`'s
/// own `default-version` case relies on exactly the opposite — no nested
/// `.git`, so the guess is skipped and `1.0.0` wins). `prepare` runs after
/// the initial commit, with the checkout's directory, to tag or branch it
/// however the caller's own guess needs.
async fn update_lock_in_git_repo(name: &str, prepare: impl FnOnce(&Path)) -> String {
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    fs_err::copy(
        fixture(name).join("composer.json"),
        dir.join("composer.json"),
    )
    .unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "tag.gpgsign", "false"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    prepare(dir);

    let composer_json = fs_err::read(dir.join("composer.json")).unwrap();
    let root: Value = serde_json::from_slice(&composer_json).unwrap();

    let cache = tempfile::tempdir().unwrap();
    let transport = PanicTransport;
    let repo = Repository::from_composer_json(dir, &root, cache.path(), &transport)
        .await
        .unwrap();

    let result = vivace::solver::solve_update(&repo, &root, dir, false, false)
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
    vivace::lock_writer::write(&result.non_dev, Some(&result.dev), &options, &composer_json)
        .unwrap()
}

/// The root has an explicit `version` (`2.3.0`) and requires its sibling at
/// `self.version`, with the sibling published at both `2.3.0` and a newer
/// `2.4.0`: `self.version` must pin the older, root-matching release, not
/// whatever the solver would otherwise prefer.
#[tokio::test]
async fn root_self_version_locks_the_roots_own_version() {
    let got = update_lock("with-version").await;
    let want = fs_err::read_to_string(fixture("with-version").join("composer.lock")).unwrap();
    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}

/// The root has no `version` at all, and (`tests/fixtures/` carries no
/// nested `.git`, #312's own precheck) no VCS checkout to guess one from
/// either: `self.version` falls back to `RootPackageLoader`'s own `1.0.0`
/// default, the same one `root_package` already applies.
#[tokio::test]
async fn root_self_version_falls_back_to_the_default_root_version() {
    let _lock = ENV_LOCK.lock().await;
    let got = update_lock("default-version").await;
    let want = fs_err::read_to_string(fixture("default-version").join("composer.lock")).unwrap();
    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}

/// A root with `require-dev` takes the dev-split second solve, whose
/// request is rebuilt from the raw root JSON: the `self.version` literal
/// reached the constraint parser there after the first solve had already
/// succeeded (found on a monorepo member with a `phpunit` dev requirement).
#[tokio::test]
async fn root_self_version_survives_the_dev_split_second_solve() {
    let got = update_lock("with-require-dev").await;
    let want = fs_err::read_to_string(fixture("with-require-dev").join("composer.lock")).unwrap();
    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}

/// #312: no explicit `version`, but a real git checkout with a tag at
/// `HEAD` (`git tag`, then `git checkout` to the tag, matching
/// `tests/root_version.rs`'s own `tag_detached_head_reads_the_tag_name`
/// exactly, since that's Composer's own `VersionGuesser::guessGitVersion`
/// tag-at-`HEAD` branch): `self.version` must resolve to the tag, `1.2.0`,
/// which is the only published `acme/sibling` release the fixture's own
/// `1.0.0`-constrained fallback could never have matched (its only other
/// release, `1.3.0`, is there to prove the solve isn't just falling back
/// to "newest available" either).
#[tokio::test]
async fn root_self_version_guesses_a_tag_at_head() {
    let _lock = ENV_LOCK.lock().await;
    let got = update_lock_in_git_repo("git-tag", |dir| {
        git(dir, &["tag", "-m", "1.2.0", "1.2.0"]);
        git(dir, &["checkout", "-q", "1.2.0"]);
    })
    .await;
    let want = fs_err::read_to_string(fixture("git-tag").join("composer.lock")).unwrap();
    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}

/// #312: no explicit `version`, and no tag at `HEAD` either — just `main`,
/// a non-feature branch name (`vcs::is_feature_branch`'s own built-in
/// list): `self.version` must resolve to `dev-main`, matching
/// `tests/root_version.rs`'s `named_branch_becomes_dev_prefixed`. The
/// fixture's other published `acme/sibling` release, a stable `9.9.9`, is
/// there for the same "not just picking newest/most-stable" reason
/// `root_self_version_guesses_a_tag_at_head`'s `1.3.0` is.
#[tokio::test]
async fn root_self_version_guesses_the_current_branch() {
    let _lock = ENV_LOCK.lock().await;
    let got = update_lock_in_git_repo("git-branch", |_dir| {}).await;
    let want = fs_err::read_to_string(fixture("git-branch").join("composer.lock")).unwrap();
    assert_eq!(got, want, "viv's lock does not byte-match composer's");
}

/// #312: `COMPOSER_ROOT_VERSION` wins over the git guess, same as
/// Composer's own `RootPackageLoader` (`getenv('COMPOSER_ROOT_VERSION') ?:
/// $this->versionGuesser->guessVersion(...)`) — reuses `git-tag`'s own
/// checkout (tag `1.2.0` at `HEAD`) but the env var names a *third*,
/// unpublished-at-the-tag version, so a solve that ignored it and used the
/// guess instead would fail outright (no `acme/sibling` release named
/// `1.2.0` in this run) rather than quietly picking the wrong one.
///
/// Takes [`ENV_LOCK`] for the same reason `src/auth.rs`'s own test-only
/// `EnvGuard` does: `make check`'s gate is `cargo nextest run`
/// (`Makefile`/`AGENTS.md`), which process-isolates every test and never
/// needs this, but a plain `cargo test` run of this same file shares one
/// process's threads, and without the lock the three fixtures with no
/// explicit root `version` (`default-version`, `git-tag`, `git-branch`)
/// could read this var mid-mutation and fail to resolve `acme/sibling`.
#[tokio::test]
#[allow(
    unsafe_code,
    reason = "serialised by ENV_LOCK for the guard's lifetime"
)]
async fn composer_root_version_env_wins_over_the_guess() {
    let _lock = ENV_LOCK.lock().await;
    let previous = std::env::var("COMPOSER_ROOT_VERSION").ok();
    // SAFETY: serialised by ENV_LOCK for this guard's lifetime — see this
    // test's own doc comment.
    unsafe {
        std::env::set_var("COMPOSER_ROOT_VERSION", "2.3.4");
    }
    let dir = tempfile::tempdir().unwrap();
    let dir = dir.path();
    fs_err::copy(
        fixture("git-tag").join("composer.json"),
        dir.join("composer.json"),
    )
    .unwrap();
    // `2.3.4` (the env var) is a real `acme/sibling` release the git tag
    // (`1.2.0`) never is, so `self.version` resolving to the tag instead of
    // the env var breaks the solve rather than merely picking a different
    // package.
    let mut composer_json: Value =
        serde_json::from_slice(&fs_err::read(dir.join("composer.json")).unwrap()).unwrap();
    composer_json["repositories"][1]["package"]
        .as_array_mut()
        .unwrap()
        .push(serde_json::json!({
            "name": "acme/sibling",
            "version": "2.3.4",
            "dist": {"type": "zip", "url": "https://example.com/dist/acme/sibling-2.3.4.zip"}
        }));
    fs_err::write(
        dir.join("composer.json"),
        serde_json::to_vec_pretty(&composer_json).unwrap(),
    )
    .unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
    git(dir, &["config", "tag.gpgsign", "false"]);
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-q", "-m", "initial"]);
    git(dir, &["tag", "-m", "1.2.0", "1.2.0"]);
    git(dir, &["checkout", "-q", "1.2.0"]);

    let cache = tempfile::tempdir().unwrap();
    let transport = PanicTransport;
    let repo = Repository::from_composer_json(dir, &composer_json, cache.path(), &transport)
        .await
        .unwrap();
    let result = vivace::solver::solve_update(&repo, &composer_json, dir, false, false).await;

    // SAFETY: see this function's own doc comment.
    unsafe {
        match &previous {
            Some(v) => std::env::set_var("COMPOSER_ROOT_VERSION", v),
            None => std::env::remove_var("COMPOSER_ROOT_VERSION"),
        }
    }

    let sibling = result
        .unwrap()
        .non_dev
        .into_iter()
        .find(|p| p.name == "acme/sibling")
        .expect("acme/sibling must resolve");
    assert_eq!(
        sibling.pretty_version, "2.3.4",
        "COMPOSER_ROOT_VERSION must win over the tag guess"
    );
}
