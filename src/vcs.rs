//! `"vcs"`/`"git"`/`"github"` `repositories[]` entries (`#97`):
//! `VcsRepository`'s tag/branch walk over a single package, backed by
//! either `Vcs\GitDriver` (a local mirror clone, driven with the `git` CLI)
//! or `Vcs\GitHubDriver` (the REST API, through [`crate::repository::Transport`]
//! so a test can serve recorded fixtures with no network). `repository.rs`'s
//! `Source::load_versions` asks a `VcsSource` for a name's versions the
//! same way it asks a `composer`-type source; a VCS source answers with
//! every version of the one package it holds once `name` matches that
//! package (resolved lazily, from the default branch's `composer.json`,
//! and cached for the rest of the run), or an empty list otherwise.
//!
//! Skipped: every other Composer VCS driver (GitLab, Bitbucket, Forgejo,
//! Mercurial, Perforce, Fossil, SVN — `"vcs"` only ever autodetects GitHub
//! or falls back to the generic git driver here), GitHub Enterprise (a
//! custom `github-domains` host), a private repository's SSH fallback
//! (`GitHubDriver::generateSshUrl` — a 404/403 falls back to the *same*
//! (https) URL over the git driver instead), `funding`/`abandoned`/
//! `support.issues` (`support.source` is filled in, matching the design
//! brief), the GitHub API's `Link`-header pagination (a single
//! `per_page=100` page; [`crate::repository::Transport`]/[`crate::fetch::Conditional`]
//! don't expose response headers), and `VersionParser::parseConstraints`'s
//! extra branch-name validation (a branch whose name breaks constraint
//! syntax isn't rejected).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result, bail};
use regex::Regex;
use reqwest::Url;
use serde_json::{Map, Value, json};
use tokio::sync::OnceCell;

use crate::fetch::Conditional;
use crate::repository::{PackageVersion, Transport};

/// A tag or branch name mapped to the commit it currently points at
/// (`Vcs\GitDriver`/`Vcs\GitHubDriver`'s `getTags`/`getBranches`, a
/// `name => identifier` map upstream; a `Vec` here since ordering — the
/// default branch moved to the front — matters and names are already
/// known unique).
#[derive(Debug, Clone)]
struct RefEntry {
    name: String,
    sha: String,
}

/// One loaded VCS repository: its driver, plus the package name and full
/// version list, both resolved at most once per run
/// (`tokio::sync::OnceCell` rather than a plain `Mutex`, since resolving
/// either needs an `.await`).
pub(crate) struct VcsSource {
    driver: Driver,
    package_name: OnceCell<Option<String>>,
    versions: OnceCell<Vec<PackageVersion>>,
}

impl VcsSource {
    /// `repo_type` is the `composer.json` `"type"` string as written
    /// (`"vcs"`, `"git"` or `"github"`): `"git"` always forces the generic
    /// driver, `"github"` always forces the GitHub driver, and `"vcs"`
    /// autodetects (GitHub for a `github.com` URL, the generic driver
    /// otherwise) — `VcsRepository::getDriver`'s `$this->drivers[$type]`
    /// vs. its `supports()` autodetect loop.
    pub(crate) async fn load<T: Transport>(
        url: &str,
        repo_type: &str,
        cache_root: &Path,
        transport: &T,
    ) -> Result<VcsSource> {
        let requests = AtomicUsize::new(0);
        let github_repo = match repo_type {
            "git" => None,
            "github" => Some(parse_github_url(url).with_context(|| {
                format!("repository (type \"github\"): {url:?} is not a github.com URL")
            })?),
            _ => parse_github_url(url),
        };

        let driver = match github_repo {
            Some((owner, repo)) => {
                match GitHubDriver::load(&owner, &repo, transport, &requests).await? {
                    Some(driver) => Driver::GitHub(driver),
                    // GitHub API returned 404/403 listing tags/branches/repo
                    // data: `GitHubDriver::attemptCloneFallback`, minus the
                    // SSH URL swap (this crate has no SSH auth of its own).
                    None => Driver::Git(GitDriver::load(url, cache_root)?),
                }
            }
            None => Driver::Git(GitDriver::load(url, cache_root)?),
        };

        Ok(VcsSource {
            driver,
            package_name: OnceCell::new(),
            versions: OnceCell::new(),
        })
    }

    /// This source's versions for `name` (already lowercased): every
    /// version of the one package this VCS repository holds if `name`
    /// matches it, otherwise none.
    pub(crate) async fn load_versions<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
        name: &str,
    ) -> Result<Vec<PackageVersion>> {
        let package_name = self.ensure_package_name(transport, requests).await?;
        let Some(package_name) = package_name.clone() else {
            return Ok(Vec::new());
        };
        if package_name != name {
            return Ok(Vec::new());
        }
        let versions = self
            .versions
            .get_or_try_init(|| build_versions(&self.driver, transport, requests, &package_name))
            .await?;
        Ok(versions.clone())
    }

    /// `VcsRepository::$packageName`: the root identifier's `composer.json`
    /// `"name"`, fetched (and cached) once. `None` — no composer.json at
    /// the root identifier, or it has no `"name"` — means this repository
    /// never matches any requested package name.
    async fn ensure_package_name<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
    ) -> Result<&Option<String>> {
        self.package_name
            .get_or_try_init(|| async {
                let root = self.driver.root_identifier();
                let sha = self
                    .driver
                    .branches()
                    .iter()
                    .chain(self.driver.tags())
                    .find(|r| r.name == root)
                    .map(|r| r.sha.clone());
                let Some(sha) = sha else {
                    return Ok(None);
                };
                let composer_json = self.driver.composer_json(transport, requests, &sha).await?;
                Ok(composer_json
                    .and_then(|v| v.get("name").and_then(Value::as_str).map(str::to_string)))
            })
            .await
    }
}

enum Driver {
    Git(GitDriver),
    GitHub(GitHubDriver),
}

impl Driver {
    fn tags(&self) -> &[RefEntry] {
        match self {
            Driver::Git(d) => &d.tags,
            Driver::GitHub(d) => &d.tags,
        }
    }

    fn branches(&self) -> &[RefEntry] {
        match self {
            Driver::Git(d) => &d.branches,
            Driver::GitHub(d) => &d.branches,
        }
    }

    fn root_identifier(&self) -> &str {
        match self {
            Driver::Git(d) => &d.root_identifier,
            Driver::GitHub(d) => &d.root_identifier,
        }
    }

    async fn composer_json<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
        identifier: &str,
    ) -> Result<Option<Value>> {
        match self {
            Driver::Git(d) => d.composer_json(identifier),
            Driver::GitHub(d) => d.composer_json(transport, requests, identifier).await,
        }
    }

    async fn commit_time<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
        identifier: &str,
    ) -> Result<Option<String>> {
        match self {
            Driver::Git(d) => d.commit_time(identifier),
            Driver::GitHub(d) => d.commit_time(transport, requests, identifier).await,
        }
    }

    fn source(&self, identifier: &str) -> Value {
        match self {
            Driver::Git(d) => d.source(identifier),
            Driver::GitHub(d) => d.source(identifier),
        }
    }

    fn dist(&self, identifier: &str) -> Option<Value> {
        match self {
            Driver::Git(_) => None,
            Driver::GitHub(d) => Some(d.dist(identifier)),
        }
    }
}

// ---------------------------------------------------------------------
// Shared tag/branch walk (`VcsRepository::initialize`).
// ---------------------------------------------------------------------

async fn build_versions<T: Transport>(
    driver: &Driver,
    transport: &T,
    requests: &AtomicUsize,
    package_name: &str,
) -> Result<Vec<PackageVersion>> {
    let mut used_normalized: HashSet<String> = HashSet::new();
    let mut versions = Vec::new();

    for tag in driver.tags() {
        // "strip the release- prefix from tags if present"
        // (`VcsRepository.php:223`, a plain `str_replace` — every
        // occurrence, not just a leading one).
        let stripped = tag.name.replace("release-", "");
        let Ok(parsed_tag) = crate::version::normalize(&stripped) else {
            continue;
        };
        let Some(composer_json) = driver.composer_json(transport, requests, &tag.sha).await? else {
            continue;
        };
        let Some(mut data) = composer_json.as_object().cloned() else {
            continue;
        };

        let (pretty_version, version_normalized) = match data.get("version").and_then(Value::as_str)
        {
            Some(v) => {
                let Ok(normalized) = crate::version::normalize(v) else {
                    continue;
                };
                (v.to_string(), normalized)
            }
            None => (stripped.clone(), parsed_tag.clone()),
        };
        let pretty_version = strip_trailing_dev(&pretty_version);
        let version_normalized = strip_normalized_dev(&version_normalized);
        data.remove("default-branch");

        // "broken package, version doesn't match tag"
        if version_normalized != parsed_tag {
            continue;
        }
        if !used_normalized.insert(version_normalized.clone()) {
            continue;
        }

        versions.push(
            finish_version(
                data,
                driver,
                &tag.sha,
                package_name,
                (&pretty_version, &version_normalized),
                transport,
                requests,
            )
            .await?,
        );
    }

    // "make sure the root identifier branch gets loaded first" (so a
    // later branch/tag that collides with it on `version_normalized` is
    // the one skipped, not the default branch).
    let mut branches: Vec<&RefEntry> = driver.branches().iter().collect();
    if let Some(pos) = branches
        .iter()
        .position(|b| b.name == driver.root_identifier())
    {
        let root = branches.remove(pos);
        branches.insert(0, root);
    }

    for branch in branches {
        let is_default = branch.name == driver.root_identifier();
        let parsed_branch = crate::version::normalize_branch(&branch.name);

        let (pretty_version, version_normalized) = if parsed_branch.starts_with("dev-") {
            (
                format!("dev-{}", branch.name.replace('#', "+")),
                parsed_branch.replace('#', "+"),
            )
        } else {
            let prefix = if branch.name.starts_with('v') {
                "v"
            } else {
                ""
            };
            (
                format!("{prefix}{}", collapse_branch_wildcard(&parsed_branch)),
                parsed_branch.clone(),
            )
        };

        let Some(composer_json) = driver
            .composer_json(transport, requests, &branch.sha)
            .await?
        else {
            continue;
        };
        let Some(mut data) = composer_json.as_object().cloned() else {
            continue;
        };
        data.remove("default-branch");
        if is_default {
            data.insert("default-branch".to_string(), Value::Bool(true));
        }

        if !used_normalized.insert(version_normalized.clone()) {
            continue;
        }

        versions.push(
            finish_version(
                data,
                driver,
                &branch.sha,
                package_name,
                (&pretty_version, &version_normalized),
                transport,
                requests,
            )
            .await?,
        );
    }

    Ok(versions)
}

/// `VcsRepository::preProcess` plus the loader's own name/version
/// override: build the provider-file-shaped `Value` for one version entry
/// and hand it to [`PackageVersion::from_value`] rather than duplicating
/// its field extraction. `version` is `(pretty, normalized)`.
async fn finish_version<T: Transport>(
    mut data: Map<String, Value>,
    driver: &Driver,
    identifier: &str,
    package_name: &str,
    version: (&str, &str),
    transport: &T,
    requests: &AtomicUsize,
) -> Result<PackageVersion> {
    let (pretty_version, version_normalized) = version;
    data.insert("name".to_string(), Value::String(package_name.to_string()));
    data.insert(
        "version".to_string(),
        Value::String(pretty_version.to_string()),
    );
    data.insert(
        "version_normalized".to_string(),
        Value::String(version_normalized.to_string()),
    );
    // `ArrayLoader::load`: `$data['type'] = !empty($data['type']) ? ... :
    // 'library'` — Packagist's own provider files always carry an
    // explicit `type`, but a VCS repository's `composer.json` is written
    // by hand and often doesn't.
    if data
        .get("type")
        .and_then(Value::as_str)
        .is_none_or(str::is_empty)
    {
        data.insert("type".to_string(), Value::String("library".to_string()));
    }

    if !data.contains_key("source") {
        data.insert("source".to_string(), driver.source(identifier));
    }
    if !data.contains_key("dist")
        && let Some(dist) = driver.dist(identifier)
    {
        data.insert("dist".to_string(), dist);
    }
    // "if custom dist info is provided but does not provide a reference,
    // copy the source reference to it"
    let source_reference = data
        .get("source")
        .and_then(|s| s.get("reference"))
        .and_then(Value::as_str)
        .map(str::to_string);
    if let Some(Value::Object(dist)) = data.get_mut("dist")
        && !dist.contains_key("reference")
        && let Some(reference) = source_reference
    {
        dist.insert("reference".to_string(), Value::String(reference));
    }

    let has_time = data
        .get("time")
        .and_then(Value::as_str)
        .is_some_and(|t| !t.is_empty());
    if !has_time && let Some(time) = driver.commit_time(transport, requests, identifier).await? {
        data.insert("time".to_string(), Value::String(time));
    }

    PackageVersion::from_value(&Value::Object(data))
}

/// `Preg::replace('{[.-]?dev$}i', '', ...)`.
static TRAILING_DEV: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)[.-]?dev$").unwrap());
/// `Preg::replace('{(^dev-|[.-]?dev$)}i', '', ...)`.
static NORMALIZED_DEV: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(^dev-|[.-]?dev$)").unwrap());
/// `Preg::replace('{(\.9{7})+}', '.x', ...)`.
static NINES_RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?:\.9999999)+").unwrap());

fn strip_trailing_dev(s: &str) -> String {
    TRAILING_DEV.replace_all(s, "").into_owned()
}

fn strip_normalized_dev(s: &str) -> String {
    NORMALIZED_DEV.replace_all(s, "").into_owned()
}

fn collapse_branch_wildcard(parsed_branch: &str) -> String {
    NINES_RUN.replace_all(parsed_branch, ".x").into_owned()
}

// ---------------------------------------------------------------------
// Driver A: generic git, via the `git` CLI (`Vcs\GitDriver`).
// ---------------------------------------------------------------------

struct GitDriver {
    repo_dir: PathBuf,
    url: String,
    tags: Vec<RefEntry>,
    branches: Vec<RefEntry>,
    root_identifier: String,
}

impl GitDriver {
    /// Refs are listed up front here, matching every other driver;
    /// `composer.json` for a given ref is read lazily, only when
    /// [`build_versions`] actually walks that tag/branch.
    fn load(url: &str, cache_root: &Path) -> Result<GitDriver> {
        // `VcsDriver`'s constructor runs every local-path URL through
        // `Filesystem::getPlatformPath` (strips a `file://` scheme)
        // *before* any driver-specific logic — so `getUrl()`'s later
        // `.git`-suffix strip lands on that already-scheme-stripped
        // value, and a local repo's `source.url` in the lock never has
        // one, unlike a remote URL's.
        let (repo_dir, canonical_url) = if let Some(path) = local_repo_path(url) {
            let path = strip_git_dir_suffix(&path);
            if !Path::new(&path).is_dir() {
                bail!("{url}: not a directory");
            }
            (PathBuf::from(&path), path)
        } else {
            let dir = cache_root.join("vcs-v0").join(slugify(url));
            sync_mirror(url, &dir)?;
            (dir, url.to_string())
        };
        let tags = git_tags(&repo_dir)?;
        let branches = git_branches(&repo_dir)?;
        let root_identifier = detect_default_branch(&repo_dir);
        Ok(GitDriver {
            repo_dir,
            url: canonical_url,
            tags,
            branches,
            root_identifier,
        })
    }

    fn composer_json(&self, identifier: &str) -> Result<Option<Value>> {
        let output = Command::new("git")
            .args(["show", &format!("{identifier}:composer.json")])
            .current_dir(&self.repo_dir)
            .output()
            .with_context(|| format!("running git show {identifier}:composer.json"))?;
        if !output.status.success() || output.stdout.iter().all(u8::is_ascii_whitespace) {
            return Ok(None);
        }
        match serde_json::from_slice::<Value>(&output.stdout) {
            Ok(v) if v.is_object() => Ok(Some(v)),
            _ => Ok(None),
        }
    }

    fn commit_time(&self, identifier: &str) -> Result<Option<String>> {
        let output = Command::new("git")
            .args(["log", "-1", "--format=%at", identifier])
            .current_dir(&self.repo_dir)
            .output()
            .with_context(|| format!("running git log -1 --format=%at {identifier}"))?;
        if !output.status.success() {
            return Ok(None);
        }
        let text = String::from_utf8_lossy(&output.stdout);
        match text.trim().parse::<i64>() {
            Ok(ts) => Ok(Some(format_unix_utc(ts))),
            Err(_) => Ok(None),
        }
    }

    fn source(&self, identifier: &str) -> Value {
        json!({"type": "git", "url": self.url, "reference": identifier})
    }
}

/// `Filesystem::isLocalPath`, restricted to the two forms this crate's
/// tests actually need: an explicit `file://` URL, or a URL with no
/// `scheme://` at all (a plain filesystem path).
fn local_repo_path(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("file://") {
        return Some(rest.to_string());
    }
    if !url.contains("://") && !url.starts_with("git@") {
        return Some(url.to_string());
    }
    None
}

/// `Preg::replace('{[\\/]\.git\/?$}', '', ...)`.
fn strip_git_dir_suffix(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    trimmed.strip_suffix("/.git").unwrap_or(trimmed).to_string()
}

/// `Preg::replace('{[^a-z0-9.]}i', '-', ...)`: `GitDriver`'s cache
/// directory name for a remote URL.
fn slugify(url: &str) -> String {
    url.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// `GitUtil::syncMirror`: a fresh `git clone --mirror` if the cache
/// directory isn't a git repo yet, otherwise `git remote update --prune`
/// (a `HEAD` file at the top of the directory is `--mirror`'s own
/// tell — a plain `git clone` never puts one there).
fn sync_mirror(url: &str, dir: &Path) -> Result<()> {
    if dir.join("HEAD").is_file() {
        run_git(dir, &["remote", "set-url", "origin", url])?;
        run_git(dir, &["remote", "update", "--prune", "origin"])?;
        return Ok(());
    }
    if dir.exists() {
        fs_err::remove_dir_all(dir)?;
    }
    if let Some(parent) = dir.parent() {
        fs_err::create_dir_all(parent)?;
    }
    let dir_str = dir
        .to_str()
        .context("cache directory path is not valid UTF-8")?;
    let output = Command::new("git")
        .args(["clone", "--mirror", "--", url, dir_str])
        .output()
        .with_context(|| format!("running git clone --mirror {url}"))?;
    if !output.status.success() {
        bail!(
            "git clone --mirror {url}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn run_git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .with_context(|| format!("running git {args:?} in {}", dir.display()))?;
    if !output.status.success() {
        bail!(
            "git {args:?} in {}: {}",
            dir.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `Vcs\GitDriver::getTags`: `git show-ref --tags --dereference`, keeping
/// the peeled (`^{}`) commit sha for an annotated tag by letting a later
/// line for the same name overwrite an earlier one. A repository with no
/// tags exits non-zero with empty output — not an error.
static TAG_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([0-9a-f]{40}) refs/tags/(.+?)(\^\{\})?$").unwrap());

fn git_tags(dir: &Path) -> Result<Vec<RefEntry>> {
    let output = Command::new("git")
        .args(["show-ref", "--tags", "--dereference"])
        .current_dir(dir)
        .output()
        .context("running git show-ref --tags --dereference")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut tags: Vec<RefEntry> = Vec::new();
    for line in text.lines() {
        let Some(caps) = TAG_LINE.captures(line) else {
            continue;
        };
        let sha = caps[1].to_string();
        let name = caps[2].to_string();
        if let Some(existing) = tags.iter_mut().find(|t| t.name == name) {
            existing.sha = sha;
        } else {
            tags.push(RefEntry { name, sha });
        }
    }
    Ok(tags)
}

/// `Vcs\GitDriver::getBranches`: `git branch --no-color --no-abbrev -v`,
/// skipping a remote's `HEAD` pointer line and any name starting with
/// `-` (an option, not a branch). Also exits non-zero with empty output
/// on a repository with no branches.
static BRANCH_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:\* )? *(\S+) *([0-9a-f]+)(?: .*)?$").unwrap());

fn git_branches(dir: &Path) -> Result<Vec<RefEntry>> {
    let output = Command::new("git")
        .args(["branch", "--no-color", "--no-abbrev", "-v"])
        .current_dir(dir)
        .output()
        .context("running git branch --no-color --no-abbrev -v")?;
    let text = String::from_utf8_lossy(&output.stdout);
    let mut branches = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() || line.contains("/HEAD ") {
            continue;
        }
        let Some(caps) = BRANCH_LINE.captures(line) else {
            continue;
        };
        let name = caps[1].to_string();
        if name.starts_with('-') {
            continue;
        }
        branches.push(RefEntry {
            name,
            sha: caps[2].to_string(),
        });
    }
    Ok(branches)
}

/// `Vcs\GitDriver::getRootIdentifier`, simplified to one command: a mirror
/// clone's `HEAD` tracks the remote's default branch (set at clone time),
/// and a local non-bare repository's `HEAD` tracks whatever's checked
/// out — both are exactly `git symbolic-ref --short HEAD`. Falls back to
/// `"master"` (Composer's own default) if that fails.
fn detect_default_branch(dir: &Path) -> String {
    let output = Command::new("git")
        .args(["symbolic-ref", "--short", "HEAD"])
        .current_dir(dir)
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if name.is_empty() {
                "master".to_string()
            } else {
                name
            }
        }
        _ => "master".to_string(),
    }
}

/// Epoch seconds (UTC) to Composer's `DATE_RFC3339` (`"Y-m-d\TH:i:sP"`),
/// e.g. `1704067200` -> `"2024-01-01T00:00:00+00:00"`. Civil-date math is
/// `crate::time::civil_from_days`.
fn format_unix_utc(ts: i64) -> String {
    let days = ts.div_euclid(86400);
    let secs_of_day = ts.rem_euclid(86400);
    let (h, mi, s) = (
        secs_of_day / 3600,
        (secs_of_day / 60) % 60,
        secs_of_day % 60,
    );

    let crate::time::Ymd { y, m, d } = crate::time::civil_from_days(days);

    format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}+00:00")
}

// ---------------------------------------------------------------------
// Driver B: GitHub, via the REST API (`Vcs\GitHubDriver`).
// ---------------------------------------------------------------------

struct GitHubDriver {
    owner: String,
    repo: String,
    tags: Vec<RefEntry>,
    branches: Vec<RefEntry>,
    root_identifier: String,
}

const GITHUB_API: &str = "https://api.github.com";

impl GitHubDriver {
    /// `None` signals a 404/403 listing tags/branches/the repo itself —
    /// [`VcsSource::load`] falls back to the generic driver in that case,
    /// same as `GitHubDriver::attemptCloneFallback`.
    async fn load<T: Transport>(
        owner: &str,
        repo: &str,
        transport: &T,
        requests: &AtomicUsize,
    ) -> Result<Option<GitHubDriver>> {
        let repo_url = format!("{GITHUB_API}/repos/{owner}/{repo}");
        let Some(repo_data) = api_get(transport, requests, &repo_url).await? else {
            return Ok(None);
        };
        let root_identifier = repo_data
            .get("default_branch")
            .and_then(Value::as_str)
            .unwrap_or("master")
            .to_string();

        let tags_url = format!("{GITHUB_API}/repos/{owner}/{repo}/tags?per_page=100");
        let Some(tags_data) = api_get(transport, requests, &tags_url).await? else {
            return Ok(None);
        };
        let mut tags = Vec::new();
        for tag in tags_data.as_array().into_iter().flatten() {
            let (Some(name), Some(sha)) = (
                tag.get("name").and_then(Value::as_str),
                tag.get("commit")
                    .and_then(|c| c.get("sha"))
                    .and_then(Value::as_str),
            ) else {
                continue;
            };
            tags.push(RefEntry {
                name: name.to_string(),
                sha: sha.to_string(),
            });
        }

        let branches_url = format!("{GITHUB_API}/repos/{owner}/{repo}/git/refs/heads?per_page=100");
        let Some(branches_data) = api_get(transport, requests, &branches_url).await? else {
            return Ok(None);
        };
        let mut branches = Vec::new();
        for branch in branches_data.as_array().into_iter().flatten() {
            let (Some(ref_name), Some(sha)) = (
                branch.get("ref").and_then(Value::as_str),
                branch
                    .get("object")
                    .and_then(|o| o.get("sha"))
                    .and_then(Value::as_str),
            ) else {
                continue;
            };
            let Some(name) = ref_name.strip_prefix("refs/heads/") else {
                continue;
            };
            if name == "gh-pages" {
                continue;
            }
            branches.push(RefEntry {
                name: name.to_string(),
                sha: sha.to_string(),
            });
        }

        Ok(Some(GitHubDriver {
            owner: owner.to_string(),
            repo: repo.to_string(),
            tags,
            branches,
            root_identifier,
        }))
    }

    /// `Vcs\GitHubDriver::getComposerInformation`'s GitHub-specific
    /// addition: `support.source`, when the file doesn't already declare
    /// one.
    async fn composer_json<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
        identifier: &str,
    ) -> Result<Option<Value>> {
        let url = format!(
            "{GITHUB_API}/repos/{}/{}/contents/composer.json?ref={identifier}",
            self.owner, self.repo
        );
        let Some(resource) = api_get(transport, requests, &url).await? else {
            return Ok(None);
        };
        let (Some(content), Some("base64")) = (
            resource.get("content").and_then(Value::as_str),
            resource.get("encoding").and_then(Value::as_str),
        ) else {
            return Ok(None);
        };
        let bytes = base64_decode(content)?;
        let Ok(mut parsed) = serde_json::from_slice::<Value>(&bytes) else {
            return Ok(None);
        };
        let Some(obj) = parsed.as_object_mut() else {
            return Ok(None);
        };
        let has_source = obj
            .get("support")
            .and_then(Value::as_object)
            .is_some_and(|s| s.contains_key("source"));
        if !has_source {
            let label = self.label_for(identifier);
            let support = obj
                .entry("support")
                .or_insert_with(|| Value::Object(Map::new()));
            if !support.is_object() {
                *support = Value::Object(Map::new());
            }
            support.as_object_mut().unwrap().insert(
                "source".to_string(),
                Value::String(format!(
                    "https://github.com/{}/{}/tree/{label}",
                    self.owner, self.repo
                )),
            );
        }
        Ok(Some(parsed))
    }

    async fn commit_time<T: Transport>(
        &self,
        transport: &T,
        requests: &AtomicUsize,
        identifier: &str,
    ) -> Result<Option<String>> {
        let url = format!(
            "{GITHUB_API}/repos/{}/{}/commits/{identifier}",
            self.owner, self.repo
        );
        let Some(commit) = api_get(transport, requests, &url).await? else {
            return Ok(None);
        };
        let date = commit
            .get("commit")
            .and_then(|c| c.get("committer"))
            .and_then(|c| c.get("date"))
            .and_then(Value::as_str);
        Ok(date.map(|d| match d.strip_suffix('Z') {
            Some(rest) => format!("{rest}+00:00"),
            None => d.to_string(),
        }))
    }

    fn source(&self, identifier: &str) -> Value {
        json!({
            "type": "git",
            "url": format!("https://github.com/{}/{}.git", self.owner, self.repo),
            "reference": identifier,
        })
    }

    fn dist(&self, identifier: &str) -> Value {
        json!({
            "type": "zip",
            "url": format!("{GITHUB_API}/repos/{}/{}/zipball/{identifier}", self.owner, self.repo),
            "reference": identifier,
            "shasum": "",
        })
    }

    fn label_for(&self, identifier: &str) -> String {
        self.tags
            .iter()
            .chain(&self.branches)
            .find(|r| r.sha == identifier)
            .map_or_else(|| identifier.to_string(), |r| r.name.clone())
    }
}

/// One unconditional GET, JSON-decoded; `Ok(None)` for a 404 (or any
/// transport error — GitHub's rate-limit/auth failures come back as a
/// non-404/304 status, which the real `Fetcher` turns into an `Err`) so a
/// caller can treat "listing failed" as a fallback signal rather than a
/// hard error.
async fn api_get<T: Transport>(
    transport: &T,
    requests: &AtomicUsize,
    url: &str,
) -> Result<Option<Value>> {
    let parsed = Url::parse(url).with_context(|| format!("invalid GitHub API URL {url:?}"))?;
    requests.fetch_add(1, Ordering::Relaxed);
    match transport.get(&parsed, None).await {
        Ok(Conditional::Fresh { body, .. }) => Ok(Some(
            serde_json::from_slice(&body).with_context(|| format!("{url}: not valid JSON"))?,
        )),
        Ok(Conditional::NotModified) => bail!("{url}: unexpected 304 for an unconditional request"),
        Ok(Conditional::NotFound) | Err(_) => Ok(None),
    }
}

/// `Vcs\GitHubDriver::initialize`'s URL regex, restricted to `github.com`
/// (an enterprise `github-domains` host isn't supported here): an
/// `https://github.com/owner/repo(.git)?` or `git@github.com:owner/repo.git`
/// URL.
static GITHUB_URL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(?:(?:https?|git)://(?:www\.)?github\.com/|git@github\.com:)([^/]+)/([^/]+?)(?:\.git)?/?$")
        .unwrap()
});

fn parse_github_url(url: &str) -> Option<(String, String)> {
    let caps = GITHUB_URL.captures(url)?;
    Some((caps[1].to_string(), caps[2].to_string()))
}

/// Minimal base64 (RFC 4648, standard alphabet, `=` padding) decoder for
/// the GitHub contents API's file bodies — mirrors `auth.rs`'s own
/// minimal encoder; no base64 crate is a dependency of this workspace.
fn base64_decode(input: &str) -> Result<Vec<u8>> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    static TABLE: LazyLock<[u8; 256]> = LazyLock::new(|| {
        let mut table = [255u8; 256];
        for (i, &b) in ALPHABET.iter().enumerate() {
            table[b as usize] = u8::try_from(i).unwrap();
        }
        table
    });

    let filtered: Vec<u8> = input.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    let mut out = Vec::with_capacity(filtered.len() / 4 * 3);
    for chunk in filtered.chunks(4) {
        let mut buf = [0u8; 4];
        let mut pad = 0usize;
        for (i, &b) in chunk.iter().enumerate() {
            if b == b'=' {
                pad += 1;
            } else {
                let v = TABLE[b as usize];
                if v == 255 {
                    bail!("invalid base64 input");
                }
                buf[i] = v;
            }
        }
        let n = (u32::from(buf[0]) << 18)
            | (u32::from(buf[1]) << 12)
            | (u32::from(buf[2]) << 6)
            | u32::from(buf[3]);
        out.push(u8::try_from(n >> 16).expect("top byte of a 24-bit value fits in a u8"));
        if pad < 2 {
            out.push(u8::try_from((n >> 8) & 0xFF).expect("masked byte fits in a u8"));
        }
        if pad < 1 {
            out.push(u8::try_from(n & 0xFF).expect("masked byte fits in a u8"));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{
        base64_decode, collapse_branch_wildcard, format_unix_utc, local_repo_path,
        parse_github_url, slugify, strip_git_dir_suffix, strip_normalized_dev, strip_trailing_dev,
    };

    #[test]
    fn format_unix_utc_matches_composers_date_rfc3339() {
        assert_eq!(format_unix_utc(1_704_067_200), "2024-01-01T00:00:00+00:00");
        assert_eq!(format_unix_utc(0), "1970-01-01T00:00:00+00:00");
    }

    #[test]
    fn collapse_branch_wildcard_keeps_the_dev_suffix() {
        assert_eq!(
            collapse_branch_wildcard("1.9999999.9999999.9999999-dev"),
            "1.x-dev"
        );
        assert_eq!(collapse_branch_wildcard("dev-main"), "dev-main");
    }

    #[test]
    fn strip_dev_markers_match_composers_regexes() {
        assert_eq!(strip_trailing_dev("1.0.0-dev"), "1.0.0");
        assert_eq!(strip_trailing_dev("1.0.0"), "1.0.0");
        assert_eq!(strip_normalized_dev("dev-1.0.0.0"), "1.0.0.0");
        assert_eq!(strip_normalized_dev("1.0.0.0-dev"), "1.0.0.0");
    }

    #[test]
    fn local_repo_path_recognises_file_urls_and_plain_paths() {
        assert_eq!(
            local_repo_path("file:///tmp/repo"),
            Some("/tmp/repo".to_string())
        );
        assert_eq!(local_repo_path("/tmp/repo"), Some("/tmp/repo".to_string()));
        assert_eq!(local_repo_path("https://github.com/a/b.git"), None);
        assert_eq!(local_repo_path("git@github.com:a/b.git"), None);
    }

    #[test]
    fn strip_git_dir_suffix_only_strips_a_dot_git_directory() {
        assert_eq!(strip_git_dir_suffix("/tmp/repo/.git"), "/tmp/repo");
        assert_eq!(strip_git_dir_suffix("/tmp/repo/.git/"), "/tmp/repo");
        assert_eq!(strip_git_dir_suffix("/tmp/repo.git"), "/tmp/repo.git");
    }

    #[test]
    fn slugify_replaces_non_alphanumeric_bytes() {
        assert_eq!(
            slugify("https://github.com/a/b.git"),
            "https---github.com-a-b.git"
        );
    }

    #[test]
    fn parse_github_url_extracts_owner_and_repo() {
        assert_eq!(
            parse_github_url("https://github.com/acme/widget.git"),
            Some(("acme".to_string(), "widget".to_string()))
        );
        assert_eq!(
            parse_github_url("git@github.com:acme/widget.git"),
            Some(("acme".to_string(), "widget".to_string()))
        );
        assert_eq!(
            parse_github_url("https://example.com/acme/widget.git"),
            None
        );
    }

    #[test]
    fn base64_decode_round_trips_a_known_vector() {
        // echo -n '{"name":"a/b"}' | base64
        assert_eq!(
            base64_decode("eyJuYW1lIjoiYS9iIn0=").unwrap(),
            br#"{"name":"a/b"}"#
        );
    }
}
