//! `viv audit`: security advisories from every repository that advertises
//! them, a native `Auditor`/`AuditCommand` port. Not wired into
//! `install`/`update` yet (Composer itself only runs it there when
//! `audit.abandoned`/network is enabled; that wiring is a separate task).
//!
//! One POST per advertising repository's own `security-advisories.api-url`
//! (#182, `run`'s own `Repository::security_advisory_urls` call), each with
//! every audited package's name
//! (`ComposerRepository::getSecurityAdvisories`'s plain, non-lazy path —
//! the `available-package-patterns` metadata path is not implemented,
//! matching `src/repository.rs`'s own skip list). No repository advertising
//! makes no request at all, the same as Composer's own
//! `RepositorySet::getSecurityAdvisoriesForConstraints` loop, which simply
//! has nothing to iterate. Each returned advisory is kept only if its
//! `affectedVersions` constraint matches the package's own locked/installed
//! version.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, ValueEnum};
use reqwest::Url;
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::fetch::Fetcher;
use crate::lock::{AbandonedPolicy, AuditIgnore, Config, read_lock};
use crate::repository::{HttpTransport as RepoHttpTransport, Repository};
use crate::semver;

/// `viv audit` flags.
#[derive(Args, Debug, Clone)]
pub struct AuditArgs {
    /// Disables auditing of `require-dev` packages.
    #[arg(long)]
    pub no_dev: bool,
    /// Output format.
    #[arg(short = 'f', long, value_enum, default_value = "table")]
    pub format: Format,
    /// Audit `composer.lock` instead of the installed packages
    /// (`vendor/composer/installed.json`).
    #[arg(long)]
    pub locked: bool,
    /// Behaviour on abandoned packages: `ignore`, `report`, or `fail`
    /// (Composer default: `fail`, overriding `config.audit.abandoned`).
    #[arg(long)]
    pub abandoned: Option<String>,
    /// Ignore advisories at these severity levels (`low`, `medium`, `high`,
    /// `critical`).
    #[arg(long = "ignore-severity")]
    pub ignore_severity: Vec<String>,
    /// Project directory holding `composer.json`/`composer.lock`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
}

/// `Auditor::FORMAT_*`.
#[derive(ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
#[value(rename_all = "lowercase")]
pub enum Format {
    Table,
    Plain,
    Json,
    Summary,
}

/// `Auditor::STATUS_OK`/`STATUS_FAILED`.
const STATUS_OK: u8 = 0;
const STATUS_FAILED: u8 = 1;

/// `->setColumnWidth(1, 80)` on every table `Auditor` renders, plus the
/// cell's own 1-space padding on each side.
const VALUE_COLUMN_WIDTH: usize = 82;

/// `Auditor::outputAbandonedPackages`'s vertical table headers.
const ABANDONED_HEADERS: [&str; 2] = ["Abandoned Package", "Suggested Replacement"];

/// Run `viv audit`, returning the process exit code (`0` clean, `1` an
/// active advisory or a failing abandoned package was found) rather than an
/// `Err`, which is reserved for a genuine failure to audit at all (no lock,
/// no network, ...).
pub fn run(args: &AuditArgs, cache_dir: Option<&Path>, offline: bool) -> Result<u8> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json =
        fs_err::read(project_dir.join("composer.json")).context("reading composer.json")?;
    let root = crate::lock::parse_root(&composer_json).context("parsing composer.json")?;

    let dev = !args.no_dev;
    let packages = load_packages(args, &project_dir, &root.config, dev)?;

    if packages.is_empty() {
        let has_requires = !root.require.is_empty() || (dev && !root.require_dev.is_empty());
        if has_requires {
            warn_out(
                "No installed packages found. Please run \"composer install\" before running \
                 \"audit\" or pass \"--locked\" to audit the lock file.",
            );
            return Ok(STATUS_FAILED);
        }
        warn_out("No packages - skipping audit.");
        return Ok(STATUS_OK);
    }

    let auth = crate::auth::Auth::load(&project_dir)?;
    let fetcher = Fetcher::new(auth)?
        .secure_http(root.config.secure_http)
        .offline(offline);
    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    // #182: the same repository set `viv update` solves against, just to
    // read off which one(s) advertise `security-advisories` -- a project
    // whose repositories are Satis/mirror-only never posts anywhere.
    let root_value: Value =
        serde_json::from_slice(&composer_json).context("parsing composer.json")?;
    let repo_transport = RepoHttpTransport { fetcher: &fetcher };
    let repo = runtime.block_on(Repository::from_composer_json(
        &root_value,
        &cache_dir,
        repo_transport,
    ))?;
    let endpoints = repo.security_advisory_urls();

    let transport = HttpTransport { fetcher: &fetcher };
    let (status, rendered) =
        runtime.block_on(audit(args, &packages, &root.config, &endpoints, &transport))?;
    if !rendered.is_empty() {
        out(&rendered);
    }
    Ok(status)
}

/// The testable core: given already-loaded packages, the root
/// `composer.json`'s `config`, and the repositories that advertise
/// `security-advisories` (`endpoints`, #182), POST to each one,
/// filter/ignore/group the advisories, and render every section. `run`
/// wires CLI args and a real [`Fetcher`]-backed transport around this;
/// tests inject a fixture transport instead (`repository::Transport`'s own
/// seam, for the same reason).
pub async fn audit<T: AdvisoriesTransport>(
    args: &AuditArgs,
    packages: &[AuditPackage],
    config: &Config,
    endpoints: &[String],
    transport: &T,
) -> Result<(u8, String)> {
    let abandoned_policy = match &args.abandoned {
        Some(value) => AbandonedPolicy::parse(value)?,
        None => config.audit.abandoned,
    };

    let mut names: Vec<String> = Vec::new();
    for package in packages {
        if !names.iter().any(|n| n == &package.name) {
            names.push(package.name.clone());
        }
    }

    let response = fetch_advisories_from(transport, endpoints, &names).await?;
    let (advisories, ignored_advisories) = process_advisories(
        packages,
        &response,
        &config.audit.ignore,
        &args.ignore_severity,
    )?;

    let abandoned = match abandoned_policy {
        AbandonedPolicy::Ignore => Vec::new(),
        _ => packages
            .iter()
            .filter_map(|p| match p.abandonment() {
                Abandonment::NotAbandoned => None,
                Abandonment::NoReplacement => Some((p.name.clone(), None)),
                Abandonment::Replacement(name) => Some((p.name.clone(), Some(name))),
            })
            .collect::<Vec<_>>(),
    };
    let abandoned_fails = abandoned_policy == AbandonedPolicy::Fail && !abandoned.is_empty();

    let advisories_count: usize = advisories.values().map(Vec::len).sum();
    let status = if advisories_count > 0 || abandoned_fails {
        STATUS_FAILED
    } else {
        STATUS_OK
    };

    let rendered = render(args.format, &advisories, &ignored_advisories, &abandoned);
    Ok((status, rendered))
}

/// Where `viv audit` gets bytes for the advisories POST, one per
/// advertising repository's own `api-url` (#182). Production wraps
/// [`Fetcher`]; tests serve a recorded response body and count calls
/// (`repository::Transport`'s own seam, for the same reason).
pub trait AdvisoriesTransport {
    fn post_advisories(
        &self,
        url: &Url,
        packages: &[String],
    ) -> impl std::future::Future<Output = Result<Value>> + Send;
}

/// [`AdvisoriesTransport`] backed by a real [`Fetcher`], for production use.
pub struct HttpTransport<'a> {
    pub fetcher: &'a Fetcher,
}

/// A witness [`AdvisoriesTransport`] for `pool_builder`'s pre-#175
/// signatures (`build`/`build_partial`), which pass `None` for the pool
/// filter and so never actually call this: Rust still needs a concrete type
/// argument for `Option<pool_builder::AdvisoryFilter<'_, A>>`'s `A` even
/// though the value is `None`.
pub struct NoAdvisories;

impl AdvisoriesTransport for NoAdvisories {
    #[allow(
        clippy::unused_async_trait_impl,
        reason = "never actually called (this type is only ever a None witness); async only to \
                  satisfy the trait"
    )]
    async fn post_advisories(&self, _url: &Url, _packages: &[String]) -> Result<Value> {
        unreachable!("NoAdvisories is only ever used as a None witness type, never called")
    }
}

impl AdvisoriesTransport for HttpTransport<'_> {
    async fn post_advisories(&self, url: &Url, packages: &[String]) -> Result<Value> {
        self.fetcher
            .post_json("security advisories", url, packages)
            .await
    }
}

/// `pub(crate)`: `solver::pool_builder`'s update/require pool filter (#175)
/// posts through this same function, so an offline run or an auth failure
/// fails the exact same way `viv audit`'s own POST does.
pub(crate) async fn fetch_advisories<T: AdvisoriesTransport>(
    transport: &T,
    url: &Url,
    names: &[String],
) -> Result<AdvisoriesResponse> {
    let body = transport.post_advisories(url, names).await?;
    serde_json::from_value(body).context("parsing security-advisories response")
}

/// Every `endpoints` entry (one repository's own advertised `api-url`,
/// #182) asked with the *same*, un-narrowed `names` list, then merged into
/// one response (`RepositorySet::getSecurityAdvisoriesForConstraints`'s
/// `array_merge_recursive` of each repository's own `advisories` map — the
/// `ksort` afterwards is dropped, nothing here depends on map order).
/// Composer asks every advertising repository the full package-constraint
/// map, not a per-repository subset narrowed to the names that repository
/// actually resolved (`RepositorySet.php`'s loop passes the one shared
/// `$packageConstraintMap` to each repository's own `getSecurityAdvisories`
/// call). An empty `endpoints` makes no request at all and returns an empty
/// response, matching `viv audit`'s and the pool filter's own
/// no-repository-advertises behaviour.
pub(crate) async fn fetch_advisories_from<T: AdvisoriesTransport>(
    transport: &T,
    endpoints: &[String],
    names: &[String],
) -> Result<AdvisoriesResponse> {
    let mut merged = AdvisoriesResponse {
        advisories: HashMap::new(),
    };
    for endpoint in endpoints {
        let url = Url::parse(endpoint)
            .with_context(|| format!("{endpoint:?}: invalid security-advisories api-url"))?;
        let response = fetch_advisories(transport, &url, names).await?;
        for (name, mut list) in response.advisories {
            merged.advisories.entry(name).or_default().append(&mut list);
        }
    }
    Ok(merged)
}

/// `SecurityAdvisoryPoolFilter::getMatchingAdvisories`, minus the caller's
/// own `$package->isDev()` skip (`pool_builder`'s filter applies that before
/// ever calling this, since a dev package is exempt from the check
/// entirely, not just from a version match): every advisory id from
/// `response` whose `affectedVersions` matches `version` and that `ignore`
/// doesn't cover, for one already-fetched package name. No
/// `--ignore-severity` equivalent exists for blocking (only `viv audit`
/// itself has that flag), so severity-based ignoring never applies here.
pub(crate) fn matching_advisory_ids(
    response: &AdvisoriesResponse,
    ignore: &AuditIgnore,
    name: &str,
    version: &semver::NormalizedVersion,
) -> Vec<String> {
    let Some(raw_advisories) = response.advisories.get(name) else {
        return Vec::new();
    };
    raw_advisories
        .iter()
        .filter(|raw| {
            semver::parse_constraint(&raw.affected_versions)
                .is_ok_and(|constraint| constraint.matches(version))
        })
        .filter(|raw| matches!(ignore_status(raw, ignore, &[]), IgnoreStatus::Active))
        .map(|raw| raw.advisory_id.clone())
        .collect()
}

/// `CompletePackageInterface::isAbandoned`, read straight off a pool
/// package's raw provider-metadata `abandoned` field for the update/require
/// pool filter's `audit.block-abandoned` (`AuditPackage::abandonment` is
/// this same check for the audited-install path, which reads from
/// `installed.json`/the lock instead, and also carries the suggested
/// replacement — the pool filter only ever needs the yes/no, never reports
/// it, so this stays a `bool`).
pub(crate) fn is_abandoned(raw: &Value) -> bool {
    matches!(
        raw.get("abandoned"),
        Some(Value::Bool(true) | Value::String(_))
    )
}

/// One audited package: name, its own version (to filter advisories by
/// `affectedVersions`), and its lock/`installed.json` `abandoned` value, if
/// any. Public so a test can build a fixture project directory and call
/// [`load_packages`]/[`audit`] directly, the same way `tests/repository.rs`
/// drives `Repository::load` against recorded fixtures rather than the
/// `viv` binary.
#[derive(Debug, Clone)]
pub struct AuditPackage {
    pub name: String,
    pub version: String,
    pub abandoned: Option<Value>,
}

impl AuditPackage {
    /// `CompletePackageInterface::isAbandoned`/`getReplacementPackage`.
    fn abandonment(&self) -> Abandonment {
        match &self.abandoned {
            Some(Value::Bool(true)) => Abandonment::NoReplacement,
            Some(Value::String(name)) => Abandonment::Replacement(name.clone()),
            None | Some(_) => Abandonment::NotAbandoned,
        }
    }
}

/// [`AuditPackage::abandonment`]'s result: whether a package is abandoned,
/// and its suggested replacement, if any (a plain `Option<Option<String>>`
/// draws a clippy `option_option` lint, and reads worse here anyway).
enum Abandonment {
    NotAbandoned,
    NoReplacement,
    Replacement(String),
}

/// `AuditCommand::getPackages`: `--locked` reads `composer.lock`; otherwise
/// `vendor/composer/installed.json` (a fresh `viv install`'s own output).
pub fn load_packages(
    args: &AuditArgs,
    project_dir: &Path,
    config: &Config,
    dev: bool,
) -> Result<Vec<AuditPackage>> {
    if args.locked {
        let lock_path = project_dir.join("composer.lock");
        if !lock_path.is_file() {
            bail!(
                "Valid composer.json and composer.lock files are required to run this command \
                 with --locked"
            );
        }
        let lock = read_lock(&lock_path)?;
        return Ok(lock
            .packages(dev)
            .map(|p| AuditPackage {
                name: p.name.clone(),
                version: p.version.clone(),
                abandoned: p.raw.get("abandoned").cloned(),
            })
            .collect());
    }
    read_installed(
        &project_dir
            .join(&config.vendor_dir)
            .join("composer/installed.json"),
        dev,
    )
}

#[derive(Deserialize)]
struct InstalledFile {
    #[serde(default)]
    packages: Vec<InstalledPackage>,
    #[serde(default, rename = "dev-package-names")]
    dev_package_names: Vec<String>,
}

#[derive(Deserialize)]
struct InstalledPackage {
    name: String,
    version: String,
    #[serde(default)]
    abandoned: Option<Value>,
}

/// Missing `installed.json` (no `viv install`/`composer install` yet) means
/// no packages, exactly like a missing file does in `plan::plan`.
fn read_installed(path: &Path, dev: bool) -> Result<Vec<AuditPackage>> {
    let content = match fs_err::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err.into()),
    };
    let file: InstalledFile =
        serde_json::from_str(&content).with_context(|| format!("parsing {}", path.display()))?;
    let dev_names: std::collections::HashSet<String> = file
        .dev_package_names
        .iter()
        .map(|name| name.to_lowercase())
        .collect();
    Ok(file
        .packages
        .into_iter()
        .map(|p| (p.name.to_lowercase(), p))
        .filter(|(name, _)| dev || !dev_names.contains(name))
        .map(|(name, p)| AuditPackage {
            name,
            version: p.version,
            abandoned: p.abandoned,
        })
        .collect())
}

/// One `security-advisories` API response: `{"advisories": {"pkg": [...]}}`.
/// `pub`: `pool_builder::AdvisoryFilter`'s own `prefetched` field is `pub`
/// too (a test outside this crate builds one, #189), so this must be
/// reachable at the same visibility even though it's otherwise an internal
/// detail — `pool_builder`'s filter holds one across its own per-package
/// loop instead of re-fetching per package (#175).
#[derive(Debug, Deserialize)]
pub struct AdvisoriesResponse {
    #[serde(default)]
    advisories: HashMap<String, Vec<RawAdvisory>>,
}

/// One returned advisory, keeping only the fields `SecurityAdvisory`
/// reads: extra ones the real API sends (`remoteId`, `source`,
/// `composerRepository`) are dropped by serde's default "ignore unknown
/// fields" behaviour.
#[derive(Debug, Clone, Deserialize)]
struct RawAdvisory {
    #[serde(rename = "advisoryId")]
    advisory_id: String,
    #[serde(rename = "packageName")]
    package_name: String,
    #[serde(rename = "affectedVersions")]
    affected_versions: String,
    title: String,
    cve: Option<String>,
    link: Option<String>,
    #[serde(rename = "reportedAt")]
    reported_at: String,
    #[serde(default)]
    sources: Vec<Value>,
    severity: Option<String>,
}

/// A [`RawAdvisory`] paired with whether it's ignored and why
/// (`Auditor::processAdvisories`'s `$ignoreReason`: `ignored` alone can be
/// true with `ignore_reason` still `None`, meaning no reason was
/// configured — table/plain falls back to "None specified" then, JSON
/// reports a bare `null`).
#[derive(Debug, Clone)]
struct Advisory {
    raw: RawAdvisory,
    ignored: bool,
    ignore_reason: Option<String>,
}

/// Package name -> its advisories, `process_advisories`'s return shape for
/// both the active and the ignored bucket.
type AdvisoryMap = HashMap<String, Vec<Advisory>>;

/// Split every advisory Packagist returned for an audited package into the
/// active list and the ignored one, filtering by `affectedVersions` first
/// (`PartialSecurityAdvisory::create`'s own version-matches check) and then
/// by `config.audit.ignore`/`--ignore-severity`
/// (`Auditor::processAdvisories`).
fn process_advisories(
    packages: &[AuditPackage],
    response: &AdvisoriesResponse,
    ignore: &AuditIgnore,
    ignore_severity: &[String],
) -> Result<(AdvisoryMap, AdvisoryMap)> {
    let mut advisories: AdvisoryMap = HashMap::new();
    let mut ignored: AdvisoryMap = HashMap::new();
    for package in packages {
        let Some(raw_advisories) = response.advisories.get(&package.name) else {
            continue;
        };
        let version = semver::normalize(&package.version)
            .with_context(|| format!("{}: version", package.name))?;
        for raw in raw_advisories {
            let Ok(constraint) = semver::parse_constraint(&raw.affected_versions) else {
                continue;
            };
            if !constraint.matches(&version) {
                continue;
            }
            let (ignored_flag, reason) = match ignore_status(raw, ignore, ignore_severity) {
                IgnoreStatus::Active => (false, None),
                IgnoreStatus::Ignored(reason) => (true, reason),
            };
            let advisory = Advisory {
                raw: raw.clone(),
                ignored: ignored_flag,
                ignore_reason: reason,
            };
            let bucket = if ignored_flag {
                &mut ignored
            } else {
                &mut advisories
            };
            bucket
                .entry(package.name.clone())
                .or_default()
                .push(advisory);
        }
    }
    Ok((advisories, ignored))
}

/// [`ignore_status`]'s result: whether an advisory is ignored, and its
/// configured reason, if any (a plain `Option<Option<String>>` draws a
/// clippy `option_option` lint, and reads worse here anyway).
enum IgnoreStatus {
    Active,
    Ignored(Option<String>),
}

/// Whether this advisory is ignored by package name, advisory ID, severity,
/// CVE, or any source's remote ID — checked in that order, each match
/// overwriting the previous reason, matching `Auditor::processAdvisories`'s
/// own fall-through.
fn ignore_status(
    advisory: &RawAdvisory,
    ignore: &AuditIgnore,
    ignore_severity: &[String],
) -> IgnoreStatus {
    let mut reason = None;
    let mut matched = false;
    if let Some(r) = ignore.reason_for(&advisory.package_name) {
        matched = true;
        reason = r;
    }
    if let Some(r) = ignore.reason_for(&advisory.advisory_id) {
        matched = true;
        reason = r;
    }
    if let Some(severity) = &advisory.severity
        && ignore_severity.iter().any(|s| s == severity)
    {
        matched = true;
        reason = Some(format!("{severity} severity is ignored"));
    }
    if let Some(cve) = &advisory.cve
        && let Some(r) = ignore.reason_for(cve)
    {
        matched = true;
        reason = r;
    }
    for source in &advisory.sources {
        if let Some(remote_id) = source.get("remoteId").and_then(Value::as_str)
            && let Some(r) = ignore.reason_for(remote_id)
        {
            matched = true;
            reason = r;
        }
    }
    if matched {
        IgnoreStatus::Ignored(reason)
    } else {
        IgnoreStatus::Active
    }
}

/// `PartialSecurityAdvisory::create`'s `reportedAt` (`"YYYY-MM-DD
/// HH:MM:SS"`, always UTC) reformatted to `DATE_ATOM`/`DATE_RFC3339`
/// (`"YYYY-MM-DDTHH:MM:SS+00:00"`); Packagist never sends an offset of its
/// own; there is nothing to preserve if it did.
fn format_reported_at(raw: &str) -> String {
    format!("{}+00:00", raw.replacen(' ', "T", 1))
}

fn cve_text(cve: Option<&str>) -> String {
    cve.map_or_else(|| "NO CVE".to_string(), str::to_string)
}

fn url_text(link: Option<&str>) -> String {
    link.unwrap_or_default().to_string()
}

fn severity_text(severity: Option<&str>) -> String {
    severity.unwrap_or_default().to_string()
}

/// One advisory's headers/values, in `Auditor::outputAdvisoriesTable`/
/// `outputAdvisoriesPlain`'s field order, with the trailing "Ignore reason"
/// row only for an ignored advisory.
fn advisory_fields(advisory: &Advisory) -> Vec<(&'static str, String)> {
    let raw = &advisory.raw;
    let mut fields = vec![
        ("Package", raw.package_name.clone()),
        ("Severity", severity_text(raw.severity.as_deref())),
        ("Advisory ID", raw.advisory_id.clone()),
        ("CVE", cve_text(raw.cve.as_deref())),
        ("Title", raw.title.clone()),
        ("URL", url_text(raw.link.as_deref())),
        ("Affected versions", raw.affected_versions.clone()),
        ("Reported at", format_reported_at(&raw.reported_at)),
    ];
    if advisory.ignored {
        let reason = advisory
            .ignore_reason
            .clone()
            .unwrap_or_else(|| "None specified".to_string());
        fields.push(("Ignore reason", reason));
    }
    fields
}

/// Render every section into one string, in the same order `Auditor::audit`
/// writes them (ignored pass, active pass, the summary follow-up line,
/// then abandoned packages) — a plain join of "one `io->write` call per
/// line", since none of the pieces below embed a trailing newline of their
/// own.
fn render(
    format: Format,
    advisories: &HashMap<String, Vec<Advisory>>,
    ignored: &HashMap<String, Vec<Advisory>>,
    abandoned: &[(String, Option<String>)],
) -> String {
    if format == Format::Json {
        return render_json(advisories, ignored, abandoned);
    }

    let mut lines = Vec::new();
    let ignored_count = ignored.values().map(Vec::len).sum::<usize>();
    let advisories_count = advisories.values().map(Vec::len).sum::<usize>();

    if ignored_count > 0 {
        lines.push(pass_header(
            ignored,
            "ignored security vulnerability",
            format,
        ));
        lines.extend(render_advisories(format, ignored));
    }
    if advisories_count > 0 {
        lines.push(pass_header(advisories, "security vulnerability", format));
        lines.extend(render_advisories(format, advisories));
    }
    if advisories_count == 0 && ignored_count == 0 {
        lines.push("No security vulnerability advisories found.".to_string());
    }
    if format == Format::Summary && (advisories_count > 0 || ignored_count > 0) {
        lines.push("Run \"composer audit\" for a full list of advisories.".to_string());
    }

    if !abandoned.is_empty() && format != Format::Summary {
        lines.push(format!(
            "Found {} abandoned package{}:",
            abandoned.len(),
            if abandoned.len() > 1 { "s" } else { "" }
        ));
        lines.extend(render_abandoned(format, abandoned));
    }
    lines.join("\n")
}

fn pass_header(advisories: &HashMap<String, Vec<Advisory>>, noun: &str, format: Format) -> String {
    let package_count = advisories.len();
    let total: usize = advisories.values().map(Vec::len).sum();
    let plurality = if total == 1 { "y" } else { "ies" };
    let package_plurality = if package_count == 1 { "" } else { "s" };
    let punctuation = if format == Format::Summary { "." } else { ":" };
    format!(
        "Found {total} {noun} advisor{plurality} affecting {package_count} package{package_plurality}{punctuation}"
    )
}

fn render_advisories(format: Format, advisories: &HashMap<String, Vec<Advisory>>) -> Vec<String> {
    if format == Format::Summary {
        return Vec::new();
    }
    let mut lines = Vec::new();
    for name in sorted_names(advisories) {
        for advisory in &advisories[&name] {
            let fields = advisory_fields(advisory);
            lines.push(match format {
                Format::Table => render_advisory_table(&fields),
                Format::Plain => render_advisory_plain(&fields),
                Format::Json | Format::Summary => unreachable!(),
            });
        }
    }
    lines
}

fn sorted_names<V>(map: &HashMap<String, V>) -> Vec<String> {
    let mut names: Vec<String> = map.keys().cloned().collect();
    names.sort();
    names
}

/// `Auditor::outputAdvisoriesTable`'s `setHorizontal()` table: one full
/// bordered table per advisory, a row per field.
fn render_advisory_table(fields: &[(&str, String)]) -> String {
    let label_width = fields.iter().map(|(h, _)| h.len()).max().unwrap_or(0) + 2;
    let border = format!(
        "+{}+{}+",
        "-".repeat(label_width),
        "-".repeat(VALUE_COLUMN_WIDTH)
    );
    let mut lines = vec![border.clone()];
    for (label, value) in fields {
        lines.push(format!(
            "| {label:<lw$} | {value:<vw$} |",
            lw = label_width - 2,
            vw = VALUE_COLUMN_WIDTH - 2
        ));
    }
    lines.push(border);
    lines.join("\n")
}

fn render_advisory_plain(fields: &[(&str, String)]) -> String {
    fields
        .iter()
        .map(|(label, value)| format!("{label}: {value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_abandoned(format: Format, abandoned: &[(String, Option<String>)]) -> Vec<String> {
    match format {
        Format::Plain => abandoned
            .iter()
            .map(|(name, replacement)| {
                let sentence = match replacement {
                    Some(replacement) => format!("Use {replacement} instead"),
                    None => "No replacement was suggested".to_string(),
                };
                format!("{name} is abandoned. {sentence}.")
            })
            .collect(),
        Format::Table => vec![render_abandoned_table(abandoned)],
        Format::Json | Format::Summary => Vec::new(),
    }
}

fn render_abandoned_table(abandoned: &[(String, Option<String>)]) -> String {
    let label_width = abandoned
        .iter()
        .map(|(name, _)| name.len())
        .chain(std::iter::once(ABANDONED_HEADERS[0].len()))
        .max()
        .unwrap_or(0)
        + 2;
    let border = format!(
        "+{}+{}+",
        "-".repeat(label_width),
        "-".repeat(VALUE_COLUMN_WIDTH)
    );
    let mut lines = vec![border.clone()];
    lines.push(format!(
        "| {:<lw$} | {:<vw$} |",
        ABANDONED_HEADERS[0],
        ABANDONED_HEADERS[1],
        lw = label_width - 2,
        vw = VALUE_COLUMN_WIDTH - 2
    ));
    lines.push(border.clone());
    for (name, replacement) in abandoned {
        let replacement = replacement.as_deref().unwrap_or("none");
        lines.push(format!(
            "| {name:<lw$} | {replacement:<vw$} |",
            lw = label_width - 2,
            vw = VALUE_COLUMN_WIDTH - 2
        ));
    }
    lines.push(border);
    lines.join("\n")
}

fn render_json(
    advisories: &HashMap<String, Vec<Advisory>>,
    ignored: &HashMap<String, Vec<Advisory>>,
    abandoned: &[(String, Option<String>)],
) -> String {
    let mut json = Map::new();
    json.insert("advisories".to_string(), advisories_json(advisories, false));
    if !ignored.is_empty() {
        json.insert(
            "ignored-advisories".to_string(),
            advisories_json(ignored, true),
        );
    }
    json.insert(
        "abandoned".to_string(),
        if abandoned.is_empty() {
            Value::Array(Vec::new())
        } else {
            Value::Object(
                abandoned
                    .iter()
                    .map(|(name, replacement)| {
                        (
                            name.clone(),
                            replacement.clone().map_or(Value::Null, Value::String),
                        )
                    })
                    .collect(),
            )
        },
    );
    json.insert("filter".to_string(), Value::Array(Vec::new()));

    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&Value::Object(json), &mut serializer)
        .expect("serializing a Value never fails");
    String::from_utf8(buf).expect("serde_json always writes valid UTF-8")
}

/// PHP's empty-array-serialises-as-`[]` quirk: `advisories`/
/// `ignored-advisories` is an object keyed by package name when non-empty,
/// or a bare `[]` (never `{}`) when there is nothing to report.
fn advisories_json(advisories: &HashMap<String, Vec<Advisory>>, ignored: bool) -> Value {
    if advisories.is_empty() {
        return Value::Array(Vec::new());
    }
    Value::Object(
        sorted_names(advisories)
            .into_iter()
            .map(|name| {
                let list = advisories[&name]
                    .iter()
                    .map(|advisory| advisory_json(advisory, ignored))
                    .collect();
                (name, Value::Array(list))
            })
            .collect(),
    )
}

fn advisory_json(advisory: &Advisory, ignored: bool) -> Value {
    let raw = &advisory.raw;
    let mut fields = vec![
        (
            "advisoryId".to_string(),
            Value::String(raw.advisory_id.clone()),
        ),
        (
            "packageName".to_string(),
            Value::String(raw.package_name.clone()),
        ),
        (
            "affectedVersions".to_string(),
            Value::String(raw.affected_versions.clone()),
        ),
        ("title".to_string(), Value::String(raw.title.clone())),
        (
            "cve".to_string(),
            raw.cve.clone().map_or(Value::Null, Value::String),
        ),
        (
            "link".to_string(),
            raw.link.clone().map_or(Value::Null, Value::String),
        ),
        (
            "reportedAt".to_string(),
            Value::String(format_reported_at(&raw.reported_at)),
        ),
        ("sources".to_string(), Value::Array(raw.sources.clone())),
        (
            "severity".to_string(),
            raw.severity.clone().map_or(Value::Null, Value::String),
        ),
    ];
    if ignored {
        fields.push((
            "ignoreReason".to_string(),
            advisory
                .ignore_reason
                .clone()
                .map_or(Value::Null, Value::String),
        ));
    }
    Value::Object(fields.into_iter().collect())
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout`
/// lint (`install.rs`'s `out` helper).
fn out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr`
/// lint.
fn warn_out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_advisory_table_matches_composers_column_widths() {
        let fields = vec![
            ("Package", "monolog/monolog".to_string()),
            ("Severity", "low".to_string()),
            ("Advisory ID", "PKSA-dmw8-jd8k-q3c6".to_string()),
            ("CVE", "NO CVE".to_string()),
            (
                "Title",
                "Header injection in NativeMailerHandler".to_string(),
            ),
            (
                "URL",
                "https://github.com/Seldaek/monolog/pull/448#issuecomment-68208704".to_string(),
            ),
            ("Affected versions", ">=1.8.0,<1.12.0".to_string()),
            ("Reported at", "2014-12-29T00:00:00+00:00".to_string()),
        ];
        let rendered = render_advisory_table(&fields);
        let expected = "\
+-------------------+----------------------------------------------------------------------------------+
| Package           | monolog/monolog                                                                  |
| Severity          | low                                                                              |
| Advisory ID       | PKSA-dmw8-jd8k-q3c6                                                              |
| CVE               | NO CVE                                                                           |
| Title             | Header injection in NativeMailerHandler                                          |
| URL               | https://github.com/Seldaek/monolog/pull/448#issuecomment-68208704                |
| Affected versions | >=1.8.0,<1.12.0                                                                  |
| Reported at       | 2014-12-29T00:00:00+00:00                                                        |
+-------------------+----------------------------------------------------------------------------------+";
        assert_eq!(rendered, expected);
    }

    #[test]
    fn format_reported_at_matches_date_atom() {
        assert_eq!(
            format_reported_at("2014-12-29 00:00:00"),
            "2014-12-29T00:00:00+00:00"
        );
    }

    #[test]
    fn audit_ignore_reason_for_package_or_advisory_id() {
        let ignore = AuditIgnore::Map(
            [(
                "PKSA-dmw8-jd8k-q3c6".to_string(),
                Some("we accept this risk".to_string()),
            )]
            .into_iter()
            .collect(),
        );
        assert_eq!(
            ignore.reason_for("PKSA-dmw8-jd8k-q3c6"),
            Some(Some("we accept this risk".to_string()))
        );
        assert_eq!(ignore.reason_for("other"), None);
    }
}
