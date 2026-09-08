//! `viv show`/`viv tree`/`viv outdated`: read-only inspection of installed
//! packages (`Composer\Command\ShowCommand`/`OutdatedCommand`, #69, `viv
//! tree`'s spelling from #86). `viv outdated` is exactly `show --latest
//! --outdated` (`OutdatedCommand::execute` is a thin proxy to `show`), so
//! its selection logic (`findLatestPackage`/`VersionSelector`) lives here
//! too rather than in a second file.
//!
//! The detail view's `released`/`license`/`suggests`/`provides` lines and
//! `outdated --format=json`'s `release-age`/`release-date`/
//! `latest-release-date` fields are ported (#94): `released`'s relative
//! age (`ShowCommand::getRelativeTime`) reads a wall clock overridable via
//! `VIV_TEST_NOW` (`now()`, below) so a recorded fixture never drifts out
//! from under a later run, and `license` looks identifiers up in
//! `composer/spdx-licenses`' own resource file (`src/spdx-licenses.json`,
//! `spdx_licenses`, below).
//!
//! Still not ported: `--sort-by-age`, `--available`/`--all`/`--platform`/
//! `--self`/`--path`, and platform requirement filtering in
//! `findLatestPackage` (`--ignore-platform-req(s)`) — `require.rs`'s own
//! `version_selector` module skips the same thing for the same reason (not
//! wired at this stage).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use regex::Regex;
use serde_json::{Map, Value};

use crate::auth::Auth;
use crate::fetch::Fetcher;
use crate::lock;
use crate::repository::{DevAcceptance, HttpTransport, PackageVersion, Repository};
use crate::semver;

/// `viv show`/`viv tree` flags.
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's show flags"
)]
#[derive(Args, Debug, Clone)]
pub struct ShowArgs {
    /// Package to inspect (an exact name, no wildcard support).
    pub package: Option<String>,
    /// Render the require tree instead of the package list
    /// (`viv tree` sets this unconditionally, ignoring the flag itself).
    #[arg(short = 't', long)]
    pub tree: bool,
    /// Print names only, one per line.
    #[arg(long = "name-only")]
    pub name_only: bool,
    /// Only packages required directly by the root package.
    #[arg(short = 'D', long)]
    pub direct: bool,
    /// Skip `require-dev` packages.
    #[arg(long = "no-dev")]
    pub no_dev: bool,
    /// Read `composer.lock` instead of `vendor/composer/installed.json`.
    #[arg(long)]
    pub locked: bool,
    /// With `--tree`/`viv tree`, list which packages require `package`
    /// instead of rendering its own require tree (`composer why`/`depends`,
    /// #86; `viv why` sets this unconditionally).
    #[arg(long)]
    pub invert: bool,
    /// Output format for the list view.
    #[arg(long, value_enum, default_value = "text")]
    pub format: Format,
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
}

/// `viv outdated` flags (`OutdatedCommand`'s own definition, proxied to
/// `show --latest --outdated`).
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's outdated flags"
)]
#[derive(Args, Debug, Clone)]
pub struct OutdatedArgs {
    /// Package to inspect (an exact name, no wildcard support).
    pub package: Option<String>,
    /// Only packages required directly by the root package.
    #[arg(short = 'D', long)]
    pub direct: bool,
    /// Skip `require-dev` packages.
    #[arg(long = "no-dev")]
    pub no_dev: bool,
    /// Read `composer.lock` instead of `vendor/composer/installed.json`.
    #[arg(long)]
    pub locked: bool,
    /// Only major SemVer-compatible updates.
    #[arg(short = 'M', long = "major-only")]
    pub major_only: bool,
    /// Only minor SemVer-compatible updates.
    #[arg(short = 'm', long = "minor-only")]
    pub minor_only: bool,
    /// Only patch SemVer-compatible updates.
    #[arg(short = 'p', long = "patch-only")]
    pub patch_only: bool,
    /// Exit `1` when an outdated package is found.
    #[arg(long)]
    pub strict: bool,
    /// Package name globs (`*` wildcard) to leave out of the report.
    #[arg(long)]
    pub ignore: Vec<String>,
    /// Output format.
    #[arg(long, value_enum, default_value = "text")]
    pub format: Format,
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Format {
    Text,
    Json,
}

/// `viv show`: list, detail or tree view depending on `args`.
pub fn run(args: &ShowArgs) -> Result<()> {
    run_show(args, args.tree)
}

/// `viv tree`: `show --tree`'s spelling (#86), the flag is always on here
/// regardless of what `args.tree` parsed to.
pub fn run_tree(args: &ShowArgs) -> Result<()> {
    run_show(args, true)
}

/// `viv why`: `composer why`/`depends`'s alias (#86), `viv tree --invert`
/// with the flag forced on regardless of what `args.invert` parsed to,
/// same as `run_tree` forcing `--tree`.
pub fn run_why(args: &ShowArgs) -> Result<()> {
    let mut args = args.clone();
    args.invert = true;
    run_show(&args, true)
}

fn run_show(args: &ShowArgs, tree: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let root = read_root_value(&project_dir)?;
    let root_requires = root_require_names(&root);
    let source = PackageSource::load(&project_dir, args.locked, !args.no_dev)?;

    if tree && args.invert {
        let package = args
            .package
            .as_deref()
            .context("the package to inspect is required")?;
        return print_why(&source, &root, package);
    }

    if tree {
        return print_tree(&source, &root_requires, args.package.as_deref());
    }

    if let Some(name) = &args.package {
        return print_detail(&source, &project_dir, name);
    }

    print_list(&source, &root_requires, args)
}

/// `viv outdated`. Returns whether `--strict` should make the process exit
/// `1` (an outdated package was found); anything that stops the command
/// outright is an `Err`.
pub fn run_outdated(args: &OutdatedArgs, cache_dir: Option<&Path>, offline: bool) -> Result<bool> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let root = read_root_value(&project_dir)?;
    let root_requires = root_require_names(&root);
    let source = PackageSource::load(&project_dir, args.locked, !args.no_dev)?;

    let lock_raw = read_lock_value(&project_dir)?;
    let minimum_stability = lock_raw
        .get("minimum-stability")
        .and_then(Value::as_str)
        .unwrap_or("stable")
        .to_string();
    let stability_flags: HashMap<String, u8> = lock_raw
        .get("stability-flags")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, rank)| u8::try_from(rank.as_u64()?).ok().map(|r| (name.clone(), r)))
        .collect();
    let prefer_stable = lock_raw
        .get("prefer-stable")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let secure_http = root
        .pointer("/config/secure-http")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => crate::update::default_cache_dir()?,
    };
    let auth = Auth::load(&project_dir)?;
    let fetcher = Fetcher::new(auth)?
        .secure_http(secure_http)
        .offline(offline);
    let transport = HttpTransport { fetcher: &fetcher };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let repo = runtime.block_on(Repository::from_composer_json(&root, &cache_dir, transport))?;

    let ignore = build_name_filter(&args.ignore)?;

    let mut rows = Vec::new();
    for pkg in &source.packages {
        let name = name_of(pkg);
        if ignore.as_ref().is_some_and(|re| re.is_match(&name)) {
            continue;
        }
        let direct = root_requires.contains(&name);
        if args.direct && !direct {
            continue;
        }
        let version = str_field(pkg, "version");
        let package_stability = match stability_flags.get(&name) {
            Some(&rank) => stability_name(rank),
            None => &minimum_stability,
        };
        let best_stability: &str = if prefer_stable {
            semver::stability(semver::normalize(&version)?.as_str())
        } else {
            package_stability
        };
        let latest = runtime.block_on(find_latest(
            &repo,
            &name,
            &version,
            best_stability,
            args.major_only,
            args.minor_only,
            args.patch_only,
        ))?;

        let Some(latest) = latest else {
            // `findLatestPackage` returning nothing for `--major-only` is
            // itself "up to date" (no bigger major exists); for any other
            // flag combination it's the rare "[none matched]" row Composer
            // still prints, which this port skips (see the module doc).
            if !args.major_only {
                tracing::debug!(name, "no candidate matched --latest's constraint");
            }
            continue;
        };
        if latest.version == version {
            continue;
        }
        let latest_version = strip_leading_v(&latest.version);
        let constraint = if version.starts_with("dev-") {
            version.clone()
        } else {
            format!("^{version}")
        };
        let semver_safe = semver::parse_constraint(&constraint)
            .ok()
            .zip(semver::normalize(&latest.version_normalized).ok())
            .is_some_and(|(c, v)| c.matches(&v));
        rows.push(OutdatedRow {
            name: name.clone(),
            direct,
            version: strip_leading_v(&version),
            latest: latest_version,
            marker: if semver_safe { '!' } else { '~' },
            description: str_field(pkg, "description"),
            homepage: str_field(pkg, "homepage"),
            source_view: view_source_url(pkg),
            abandoned: pkg.get("abandoned").cloned().unwrap_or(Value::Bool(false)),
            time: pkg.get("time").and_then(Value::as_str).map(str::to_string),
            latest_time: latest.time.clone(),
        });
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    let any_outdated = !rows.is_empty();
    match args.format {
        Format::Json => print_outdated_json(&rows)?,
        Format::Text => print_outdated_text(&rows, args.direct)?,
    }
    Ok(args.strict && any_outdated)
}

// ---------------------------------------------------------------------
// Shared root/lock/package-source plumbing
// ---------------------------------------------------------------------

fn read_root_value(project_dir: &Path) -> Result<Value> {
    let path = project_dir.join("composer.json");
    let bytes = fs_err::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {} as JSON", path.display()))
}

fn read_lock_value(project_dir: &Path) -> Result<Value> {
    let path = project_dir.join("composer.lock");
    let bytes = fs_err::read(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parsing {} as JSON", path.display()))
}

/// `ShowCommand::getRootRequires`: `require` and `require-dev` names,
/// lowercased, merged.
fn root_require_names(root: &Value) -> std::collections::HashSet<String> {
    ["require", "require-dev"]
        .into_iter()
        .filter_map(|key| root.get(key).and_then(Value::as_object))
        .flat_map(|map| map.keys())
        .map(|name| name.to_ascii_lowercase())
        .collect()
}

/// Every package this run considers, as raw lock/`installed.json` JSON
/// objects (already carrying every field the detail/tree/list views need).
struct PackageSource {
    packages: Vec<Value>,
}

impl PackageSource {
    fn load(project_dir: &Path, locked: bool, dev: bool) -> Result<Self> {
        if locked {
            let lock_path = project_dir.join("composer.lock");
            let lock = lock::read_lock(&lock_path)
                .with_context(|| format!("reading {}", lock_path.display()))?;
            let packages = lock.packages(dev).map(|p| p.raw.clone()).collect();
            return Ok(PackageSource { packages });
        }
        let path = project_dir.join("vendor/composer/installed.json");
        let content =
            fs_err::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
        let file: Value = serde_json::from_str(&content)
            .with_context(|| format!("parsing {} as JSON", path.display()))?;
        let dev_names: std::collections::HashSet<String> = file
            .get("dev-package-names")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_ascii_lowercase))
            .collect();
        let packages = file
            .get("packages")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter(|pkg| dev || !dev_names.contains(&name_of(pkg)))
            .collect();
        Ok(PackageSource { packages })
    }

    fn find(&self, name: &str) -> Option<&Value> {
        let name = name.to_ascii_lowercase();
        self.packages.iter().find(|p| name_of(p) == name)
    }
}

fn name_of(pkg: &Value) -> String {
    pkg.get("name")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_ascii_lowercase()
}

fn str_field(pkg: &Value, key: &str) -> String {
    pkg.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `Composer\Util\PackageInfo::getViewSourceUrl`: `support.source` when
/// it's set and non-empty, else `source.url`.
fn view_source_url(pkg: &Value) -> String {
    if let Some(url) = pkg
        .pointer("/support/source")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    {
        return url.to_string();
    }
    pkg.pointer("/source/url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// `ltrim($version, 'v')`: strip every leading `v`, not just one.
fn strip_leading_v(version: &str) -> String {
    version.trim_start_matches('v').to_string()
}

/// `BasePackage::STABILITIES`' reverse lookup: the name for a stored rank.
fn stability_name(rank: u8) -> &'static str {
    match rank {
        5 => "RC",
        10 => "beta",
        15 => "alpha",
        20 => "dev",
        _ => "stable",
    }
}

fn stability_rank(stability: &str) -> u8 {
    match stability {
        "RC" => 5,
        "beta" => 10,
        "alpha" => 15,
        "dev" => 20,
        _ => 0,
    }
}

/// `BasePackage::packageNamesToRegexp`, cut down to `--ignore`'s own
/// single-name-list shape (no separate only/exclude arrays to combine).
fn build_name_filter(names: &[String]) -> Result<Option<Regex>> {
    if names.is_empty() {
        return Ok(None);
    }
    let alternation = names
        .iter()
        .map(|n| regex::escape(n).replace(r"\*", ".*"))
        .collect::<Vec<_>>()
        .join("|");
    Ok(Some(
        Regex::new(&format!("(?i)^(?:{alternation})$")).context("building an --ignore pattern")?,
    ))
}

// ---------------------------------------------------------------------
// List view
// ---------------------------------------------------------------------

struct Row {
    name: String,
    direct: bool,
    version: String,
    description: String,
    homepage: String,
    source_view: String,
    abandoned: Value,
}

impl Row {
    fn from_raw(pkg: &Value, direct: bool) -> Row {
        Row {
            name: str_field(pkg, "name"),
            direct,
            version: str_field(pkg, "version"),
            description: str_field(pkg, "description"),
            homepage: str_field(pkg, "homepage"),
            source_view: view_source_url(pkg),
            abandoned: pkg.get("abandoned").cloned().unwrap_or(Value::Bool(false)),
        }
    }
}

fn print_list(
    source: &PackageSource,
    root_requires: &std::collections::HashSet<String>,
    args: &ShowArgs,
) -> Result<()> {
    let mut rows: Vec<Row> = source
        .packages
        .iter()
        .map(|pkg| Row::from_raw(pkg, root_requires.contains(&name_of(pkg))))
        .collect();
    if args.direct {
        rows.retain(|r| r.direct);
    }
    rows.sort_by(|a, b| a.name.cmp(&b.name));

    match args.format {
        Format::Json => print_list_json(&rows, args.name_only),
        Format::Text => print_list_text(&rows, !args.name_only, !args.name_only, None),
    }
}

/// Shared width-fit/padding/truncation logic
/// (`ShowCommand::printPackages`/its width computation just above it):
/// used by both `show`'s plain list and `outdated`'s (which adds a
/// `latest` column). `latest` is `None` for `show`.
struct LatestColumn<'a> {
    rows: &'a [OutdatedRow],
}

fn print_list_text(
    rows: &[Row],
    write_version: bool,
    write_description: bool,
    latest: Option<&LatestColumn<'_>>,
) -> Result<()> {
    let name_len = rows
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(0);
    let version_len = if write_version {
        rows.iter()
            .map(|r| strip_leading_v(&r.version).chars().count())
            .max()
            .unwrap_or(0)
    } else {
        0
    };
    let latest_len = latest.map_or(0, |l| {
        l.rows
            .iter()
            .map(|r| strip_leading_v(&r.latest).chars().count())
            .max()
            .unwrap_or(0)
    });
    let width = terminal_width();
    let version_fits = name_len + version_len + 3 <= width;
    let latest_fits = name_len + version_len + latest_len + 3 <= width;
    let description_fits = name_len + version_len + latest_len + 24 <= width;
    // Non-decorated output prepends a one-char marker and a space to the
    // latest column (`printPackages`'s own `!$io->isDecorated()` bonus).
    let latest_len = if latest.is_some() && latest_fits {
        latest_len + 2
    } else {
        latest_len
    };

    let write_version = write_version && version_fits;
    let write_latest = latest.is_some() && latest_fits;
    let write_description = write_description && description_fits;
    let pad_name = write_version || write_latest || write_description;
    let pad_version = write_latest || write_description;
    let pad_latest = write_description;

    let mut stdout = std::io::stdout().lock();
    for row in rows {
        let mut line = pad(&row.name, if pad_name { name_len } else { 0 });
        if write_version {
            line.push(' ');
            line.push_str(&pad(
                &strip_leading_v(&row.version),
                if pad_version { version_len } else { 0 },
            ));
        }
        if let Some(latest) = latest
            && let Some(outdated_row) = latest.rows.iter().find(|r| r.name == row.name)
        {
            line.push(' ');
            let text = format!(
                "{} {}",
                outdated_row.marker,
                strip_leading_v(&outdated_row.latest)
            );
            line.push_str(&pad(&text, if pad_latest { latest_len } else { 0 }));
        }
        if write_description {
            let subtract = name_len + version_len + 4 + if write_latest { latest_len } else { 0 };
            let remaining = i64::try_from(width).unwrap_or(i64::MAX)
                - i64::try_from(subtract).unwrap_or(i64::MAX);
            line.push(' ');
            line.push_str(&truncate_description(&row.description, remaining));
        }
        writeln!(stdout, "{line}")?;
    }
    Ok(())
}

fn pad(s: &str, width: usize) -> String {
    let len = s.chars().count();
    if len >= width {
        s.to_string()
    } else {
        format!("{s}{}", " ".repeat(width - len))
    }
}

/// `printPackages`'s description column: `strtok($description, "\r\n")`
/// then `mb_strimwidth` to `$remaining` (which already accounts for the
/// `...` marker's own width).
fn truncate_description(description: &str, remaining: i64) -> String {
    let remaining = usize::try_from(remaining).unwrap_or(0);
    if remaining == 0 {
        return String::new();
    }
    let first_line = description.split(['\r', '\n']).next().unwrap_or("");
    let count = first_line.chars().count();
    if count <= remaining {
        return first_line.to_string();
    }
    let keep = remaining.saturating_sub(3);
    let truncated: String = first_line.chars().take(keep).collect();
    format!("{truncated}...")
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&w| w > 0)
        .unwrap_or(80)
}

fn print_list_json(rows: &[Row], name_only: bool) -> Result<()> {
    let installed: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut obj = Map::new();
            obj.insert("name".into(), Value::String(row.name.clone()));
            obj.insert("direct-dependency".into(), Value::Bool(row.direct));
            if !name_only {
                obj.insert("homepage".into(), Value::String(row.homepage.clone()));
                obj.insert("source".into(), Value::String(row.source_view.clone()));
                obj.insert("version".into(), Value::String(row.version.clone()));
                obj.insert("description".into(), Value::String(row.description.clone()));
            }
            obj.insert("abandoned".into(), row.abandoned.clone());
            Value::Object(obj)
        })
        .collect();
    write_json(&serde_json::json!({ "installed": installed }))
}

/// `JsonFile::encode`'s default shape: 4-space pretty print, unescaped
/// slashes (`serde_json` never escapes them), trailing newline.
fn write_json(value: &Value) -> Result<()> {
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(value, &mut serializer)?;
    buf.push(b'\n');
    std::io::stdout().write_all(&buf)?;
    Ok(())
}

// ---------------------------------------------------------------------
// Detail view
// ---------------------------------------------------------------------

fn print_detail(source: &PackageSource, project_dir: &Path, name: &str) -> Result<()> {
    let Some(pkg) = source.find(name) else {
        bail!("Package \"{name}\" not found.");
    };
    let mut buf = String::new();

    writeln!(buf, "name     : {}", str_field(pkg, "name"))?;
    writeln!(buf, "descrip. : {}", str_field(pkg, "description"))?;
    writeln!(buf, "keywords : {}", join_str_array(pkg, "keywords"))?;
    writeln!(buf, "versions : * {}", str_field(pkg, "version"))?;
    if let Some(line) = released_line(pkg) {
        writeln!(buf, "released : {line}")?;
    }
    let package_type = pkg.get("type").and_then(Value::as_str).unwrap_or("library");
    writeln!(buf, "type     : {package_type}")?;
    print_licenses(&mut buf, pkg)?;
    writeln!(buf, "homepage : {}", str_field(pkg, "homepage"))?;
    writeln!(buf, "source   : {}", source_line(pkg))?;
    writeln!(buf, "dist     : {}", dist_line(pkg))?;
    writeln!(buf, "path     : {}", install_path_line(project_dir, pkg))?;
    writeln!(buf, "names    : {}", names_line(pkg))?;

    if let Some(support) = pkg
        .get("support")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
    {
        writeln!(buf)?;
        writeln!(buf, "support")?;
        for (kind, value) in support {
            writeln!(buf, "{kind} : {}", value.as_str().unwrap_or_default())?;
        }
    }

    if let Some(autoload) = pkg
        .get("autoload")
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
    {
        writeln!(buf)?;
        writeln!(buf, "autoload")?;
        for (kind, value) in autoload {
            writeln!(buf, "{kind}")?;
            match kind.as_str() {
                "psr-0" | "psr-4" => {
                    if let Some(map) = value.as_object() {
                        for (namespace, target) in map {
                            let label = if namespace.is_empty() { "*" } else { namespace };
                            let target = match target {
                                Value::Array(paths) => paths
                                    .iter()
                                    .filter_map(Value::as_str)
                                    .collect::<Vec<_>>()
                                    .join(", "),
                                Value::String(s) if !s.is_empty() => s.clone(),
                                _ => ".".to_string(),
                            };
                            writeln!(buf, "{label} => {target}")?;
                        }
                    }
                }
                "classmap" => {
                    if let Some(list) = value.as_array() {
                        writeln!(
                            buf,
                            "{}",
                            list.iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join(", ")
                        )?;
                    }
                }
                _ => {}
            }
        }
    }

    print_links(&mut buf, pkg, "require", "requires")?;
    print_links(&mut buf, pkg, "require-dev", "requires (dev)")?;
    print_links(&mut buf, pkg, "suggest", "suggests")?;
    print_links(&mut buf, pkg, "provide", "provides")?;

    write!(std::io::stdout(), "{buf}")?;
    Ok(())
}

fn join_str_array(pkg: &Value, key: &str) -> String {
    pkg.get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

fn source_line(pkg: &Value) -> String {
    let r#type = pkg
        .pointer("/source/type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let url = pkg
        .pointer("/source/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let reference = pkg
        .pointer("/source/reference")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("[{type}] {url} {reference}")
}

fn dist_line(pkg: &Value) -> String {
    let r#type = pkg
        .pointer("/dist/type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let url = pkg
        .pointer("/dist/url")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let reference = pkg
        .pointer("/dist/reference")
        .and_then(Value::as_str)
        .unwrap_or_default();
    format!("[{type}] {url} {reference}")
}

/// `getInstallationManager()->getInstallPath($package)`, resolved: the
/// `installed.json` entry's own `install-path` (relative to
/// `vendor/composer`) when present, else the plain `vendor/<name>` a
/// `--locked` package (no such field) structurally resolves to. `null`
/// when nothing exists there, matching Composer's own `realpath` fallback.
fn install_path_line(project_dir: &Path, pkg: &Value) -> String {
    let vendor_dir = project_dir.join("vendor");
    let full = match pkg.get("install-path").and_then(Value::as_str) {
        Some(rel) => vendor_dir.join("composer").join(rel),
        None => vendor_dir.join(str_field(pkg, "name")),
    };
    fs_err::canonicalize(&full).map_or_else(|_| "null".to_string(), |p| p.display().to_string())
}

/// `PackageInterface::getNames()`: the package's own name plus every name
/// it `provide`s.
fn names_line(pkg: &Value) -> String {
    let mut names = vec![str_field(pkg, "name")];
    if let Some(provide) = pkg.get("provide").and_then(Value::as_object) {
        names.extend(provide.keys().cloned());
    }
    names.join(", ")
}

fn print_links(buf: &mut String, pkg: &Value, key: &str, title: &str) -> std::fmt::Result {
    if let Some(map) = pkg
        .get(key)
        .and_then(Value::as_object)
        .filter(|m| !m.is_empty())
    {
        writeln!(buf)?;
        writeln!(buf, "{title}")?;
        for (name, constraint) in map {
            writeln!(buf, "{name} {}", constraint.as_str().unwrap_or_default())?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------
// Release age (`released`, `outdated --format=json`'s `release-age`) and
// licence (`license`) lines
// ---------------------------------------------------------------------

/// `composer/spdx-licenses`' own `res/spdx-licenses.json`
/// (`src/spdx-licenses.json`, copied verbatim, README's Licence section),
/// `{id: [full name, osi-approved, deprecated]}`. Parsed once, keyed
/// lowercase like `SpdxLicenses::getLicenseByIdentifier` itself does; the
/// deprecated flag is dropped, matching `ShowCommand::printLicenses`,
/// which never reads it either.
fn spdx_licenses() -> &'static HashMap<String, SpdxLicense> {
    static LICENSES: std::sync::OnceLock<HashMap<String, SpdxLicense>> = std::sync::OnceLock::new();
    LICENSES.get_or_init(|| {
        let raw: HashMap<String, (String, bool, bool)> =
            serde_json::from_str(include_str!("spdx-licenses.json"))
                .expect("src/spdx-licenses.json is valid JSON");
        raw.into_iter()
            .map(|(id, (name, osi, _deprecated))| {
                (id.to_ascii_lowercase(), SpdxLicense { id, name, osi })
            })
            .collect()
    })
}

/// One `spdx-licenses.json` entry. `id` is the list's canonical casing:
/// `SpdxLicenses::getLicenseByIdentifier` builds the licence-text URL from
/// it, while the identifier echoed in parentheses keeps the caller's own.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SpdxLicense {
    id: String,
    name: String,
    osi: bool,
}

/// Case-insensitive lookup, as `getLicenseByIdentifier` lowercases too.
fn spdx_license(identifier: &str) -> Option<&'static SpdxLicense> {
    spdx_licenses().get(&identifier.to_ascii_lowercase())
}

/// `ShowCommand::printLicenses`: one `license` line per `license` array
/// entry, expanded through [`spdx_license`] when known, else the raw
/// identifier verbatim (Composer's own fallback for an unrecognised one).
fn print_licenses(buf: &mut String, pkg: &Value) -> std::fmt::Result {
    let Some(licenses) = pkg.get("license").and_then(Value::as_array) else {
        return Ok(());
    };
    for identifier in licenses.iter().filter_map(Value::as_str) {
        let line = match spdx_license(identifier) {
            Some(SpdxLicense {
                id,
                name,
                osi: true,
            }) => format!(
                "{name} ({identifier}) (OSI approved) https://spdx.org/licenses/{id}.html#licenseText"
            ),
            Some(SpdxLicense {
                id,
                name,
                osi: false,
            }) => {
                format!("{name} ({identifier}) https://spdx.org/licenses/{id}.html#licenseText")
            }
            None => identifier.to_string(),
        };
        writeln!(buf, "license  : {line}")?;
    }
    Ok(())
}

use crate::time::{Ymd, civil_from_days, days_from_civil, parse_time};

/// Wall-clock "now" for the age math below, overridable via
/// `VIV_TEST_NOW` (same shape as [`parse_time`]) so a fixture recorded
/// once never drifts out from under a later test run (`#94`).
fn now() -> Ymd {
    if let Some(value) = std::env::var("VIV_TEST_NOW")
        .ok()
        .and_then(|value| parse_time(&value))
    {
        return value;
    }
    let days = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs() / 86_400).unwrap_or(0));
    civil_from_days(days)
}

/// `DateTimeImmutable::diff`'s year/month components between two dates:
/// borrow a month when the day-of-month regressed, then a year when that
/// pushes the month component negative. `getRelativeTime` only branches on
/// whether each is zero or `>= 1`, so the day itself is never returned.
fn calendar_diff(from: Ymd, to: Ymd) -> (i64, u32) {
    let mut years = to.y - from.y;
    let mut months = i64::from(to.m) - i64::from(from.m);
    if i64::from(to.d) - i64::from(from.d) < 0 {
        months -= 1;
    }
    if months < 0 {
        years -= 1;
        months += 12;
    }
    #[allow(
        clippy::cast_sign_loss,
        clippy::cast_possible_truncation,
        reason = "just added back up to 0..12 above"
    )]
    (years, months as u32)
}

/// `ShowCommand::getRelativeTime`.
fn relative_time(release: Ymd, now: Ymd) -> String {
    if release == now {
        return "today".to_string();
    }
    let days =
        days_from_civil(now.y, now.m, now.d) - days_from_civil(release.y, release.m, release.d);
    if days < 7 {
        return "this week".to_string();
    }
    if days < 14 {
        return "last week".to_string();
    }
    let (years, months) = calendar_diff(release, now);
    if years < 1 && days < 31 {
        return format!("{} weeks ago", days / 7);
    }
    if years < 1 {
        return format!("{months} month{} ago", if months > 1 { "s" } else { "" });
    }
    format!("{years} year{} ago", if years > 1 { "s" } else { "" })
}

/// `ShowCommand::printMeta`'s `released` line: `Y-m-d`, then the relative
/// age. `None` when the package carries no `time` (Composer's own
/// `getReleaseDate() !== null` guard).
fn released_line(pkg: &Value) -> Option<String> {
    let release = parse_time(pkg.get("time").and_then(Value::as_str)?)?;
    Some(format!(
        "{:04}-{:02}-{:02}, {}",
        release.y,
        release.m,
        release.d,
        relative_time(release, now())
    ))
}

/// `outdated --format=json`'s `release-age`/`release-date` fields:
/// `getRelativeTime`'s own text with `" ago"` turned into `" old"`, `"from
/// "`-prefixed when that swap didn't fire (`"today"`/`"this week"`/`"last
/// week"` never contain `" ago"`), alongside the raw `time` field
/// unchanged — it's already `DATE_ATOM`, Composer's own JSON date shape.
fn release_age_fields(time: Option<&str>) -> (String, String) {
    let Some(time) = time else {
        return (String::new(), String::new());
    };
    let Some(release) = parse_time(time) else {
        return (String::new(), String::new());
    };
    let relative = relative_time(release, now()).replace(" ago", " old");
    let age = if relative.contains(" old") {
        relative
    } else {
        format!("from {relative}")
    };
    (age, time.to_string())
}

// ---------------------------------------------------------------------
// Why (invert) view
// ---------------------------------------------------------------------

struct WhyRow {
    name: String,
    version: String,
    constraint: String,
}

/// `BaseDependencyCommand::doExecute`'s non-recursive, non-tree case
/// (`printTable`): every package (including the root) that directly
/// `require`s `package`, one line each. Not ported: `-r`/`--recursive` and
/// `-t`/`--tree`'s own nested, coloured rendering (a second traversal with
/// cycle tracking and ANSI-only colour cycling), and the inverted
/// (`why-not`) direction — only the plain "why" case #86 asks for.
fn print_why(source: &PackageSource, root: &Value, package: &str) -> Result<()> {
    let needle = package.to_ascii_lowercase();
    let root_name = root
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("__root__")
        .to_string();

    let mut rows = Vec::new();
    for (name, constraint) in require_entries(root, &["require", "require-dev"]) {
        if name.eq_ignore_ascii_case(&needle) {
            // `RootPackage::DEFAULT_PRETTY_VERSION`: with no VCS to derive
            // a version from (this project's own environment never has
            // one to offer), Composer's root package always falls back to
            // this, which `printTable` renders as a bare `-`.
            rows.push(WhyRow {
                name: root_name.clone(),
                version: "-".to_string(),
                constraint,
            });
        }
    }
    for pkg in &source.packages {
        for (name, constraint) in sorted_requires(pkg) {
            if name.eq_ignore_ascii_case(&needle) {
                rows.push(WhyRow {
                    name: str_field(pkg, "name"),
                    version: str_field(pkg, "version"),
                    constraint,
                });
            }
        }
    }

    if rows.is_empty() {
        bail!("There is no installed package depending on \"{package}\"");
    }
    write!(std::io::stdout(), "{}", render_why(&rows, package))?;
    Ok(())
}

/// `require`/`require-dev` entries in `composer.json`'s own key order
/// (`preserve_order`), unlike [`sorted_requires`]: matches
/// `PackageInterface::getRequires`/`getDevRequires`' declaration order,
/// which `getDependents`' per-package link loop iterates as-is.
fn require_entries(value: &Value, keys: &[&str]) -> Vec<(String, String)> {
    keys.iter()
        .filter_map(|key| value.get(*key).and_then(Value::as_object))
        .flat_map(Map::iter)
        .map(|(name, constraint)| {
            (
                name.clone(),
                constraint.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `BaseCommand::renderTable`'s `compact`-style `Table`: each column
/// (name, version, the constant `requires`, `target (constraint)`) padded
/// to its own widest cell and joined by a single space, plus one further
/// trailing space Symfony's compact style still emits after the last
/// column (recorded byte-for-byte in `tests/fixtures/show/expected`).
fn render_why(rows: &[WhyRow], package: &str) -> String {
    let name_len = rows
        .iter()
        .map(|r| r.name.chars().count())
        .max()
        .unwrap_or(0);
    let version_len = rows
        .iter()
        .map(|r| r.version.chars().count())
        .max()
        .unwrap_or(0);
    let targets: Vec<String> = rows
        .iter()
        .map(|row| format!("{package} ({})", row.constraint))
        .collect();
    let target_len = targets.iter().map(|t| t.chars().count()).max().unwrap_or(0);

    let mut out = String::new();
    for (row, target) in rows.iter().zip(&targets) {
        let _ = writeln!(
            out,
            "{} {} requires {} ",
            pad(&row.name, name_len),
            pad(&row.version, version_len),
            pad(target, target_len),
        );
    }
    out
}

// ---------------------------------------------------------------------
// Tree view
// ---------------------------------------------------------------------

struct TreeNode {
    name: String,
    version: String,
    children: Vec<TreeNode>,
}

fn print_tree(
    source: &PackageSource,
    root_requires: &std::collections::HashSet<String>,
    package: Option<&str>,
) -> Result<()> {
    let tops: Vec<(TreeNode, String)> = if let Some(name) = package {
        let pkg = source
            .find(name)
            .with_context(|| format!("Package \"{name}\" not found."))?;
        vec![generate_tree(source, pkg)]
    } else {
        let mut named: Vec<&Value> = source
            .packages
            .iter()
            .filter(|pkg| root_requires.contains(&name_of(pkg)))
            .collect();
        named.sort_by_key(|pkg| name_of(pkg));
        named
            .into_iter()
            .map(|pkg| generate_tree(source, pkg))
            .collect()
    };
    write!(std::io::stdout(), "{}", render_tree(&tops))?;
    Ok(())
}

/// `ShowCommand::generatePackageTree`: the shown package's own require map
/// (ksorted), one level of `addTree` deep already folded into
/// [`requires_tree`].
fn generate_tree(source: &PackageSource, pkg: &Value) -> (TreeNode, String) {
    let name = str_field(pkg, "name");
    let children = sorted_requires(pkg)
        .into_iter()
        .map(|(child_name, constraint)| {
            let path = vec![name.to_ascii_lowercase(), child_name.to_ascii_lowercase()];
            requires_tree(source, &child_name, &constraint, &path)
        })
        .collect();
    (
        TreeNode {
            name,
            version: str_field(pkg, "version"),
            children,
        },
        str_field(pkg, "description"),
    )
}

/// `ShowCommand::addTree`: recurses into `name`'s own requires (if `name`
/// resolves to a known package; a platform requirement like `php` never
/// does, and is rendered as a bare leaf), stopping along the current path
/// (`packagesInTree`) rather than globally, matching Composer's own cycle
/// guard.
fn requires_tree(
    source: &PackageSource,
    name: &str,
    constraint: &str,
    path: &[String],
) -> TreeNode {
    let mut children = Vec::new();
    if let Some(pkg) = source.find(name) {
        for (child_name, child_constraint) in sorted_requires(pkg) {
            let key = child_name.to_ascii_lowercase();
            let node = if path.contains(&key) {
                TreeNode {
                    name: child_name,
                    version: child_constraint,
                    children: Vec::new(),
                }
            } else {
                let mut next_path = path.to_vec();
                next_path.push(key);
                requires_tree(source, &child_name, &child_constraint, &next_path)
            };
            children.push(node);
        }
    }
    TreeNode {
        name: name.to_string(),
        version: constraint.to_string(),
        children,
    }
}

fn sorted_requires(pkg: &Value) -> Vec<(String, String)> {
    let mut requires: Vec<(String, String)> = pkg
        .get("require")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(name, constraint)| {
            (
                name.clone(),
                constraint.as_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    requires.sort_by(|a, b| a.0.cmp(&b.0));
    requires
}

/// `ShowCommand::displayPackageTree`/`displayTree`, replicated verbatim
/// (bar characters, indent, the non-decorated ASCII substitution) rather
/// than derived: see the module doc's tree section in `docs/`.
fn render_tree(tops: &[(TreeNode, String)]) -> String {
    let mut out = String::new();
    for (node, description) in tops {
        let first_line = description.split(['\r', '\n']).next().unwrap_or("");
        let _ = writeln!(out, "{} {} {first_line}", node.name, node.version);
        let total = node.children.len();
        for (i, child) in node.children.iter().enumerate() {
            let is_last = i + 1 == total;
            let bar = if is_last { '└' } else { '├' };
            write_tree_line(
                &mut out,
                &format!("{bar}──{} {}", child.name, child.version),
            );
            let next_prefix = if is_last {
                " ".to_string()
            } else {
                bar.to_string()
            };
            render_tree_children(&mut out, child, &next_prefix);
        }
    }
    out
}

fn render_tree_children(out: &mut String, node: &TreeNode, previous_bar: &str) {
    let converted = previous_bar.replace('├', "│");
    let total = node.children.len();
    for (i, child) in node.children.iter().enumerate() {
        let is_last = i + 1 == total;
        let bar = if is_last { '└' } else { '├' };
        let prefix = format!("{converted}  {bar}");
        write_tree_line(out, &format!("{prefix}──{} {}", child.name, child.version));
        render_tree_children(out, child, &prefix.replace('└', " "));
    }
}

/// `ShowCommand::writeTreeLine`'s non-decorated substitution.
fn write_tree_line(out: &mut String, raw: &str) {
    let ascii = raw
        .replace('└', "`-")
        .replace('├', "|-")
        .replace("──", "-")
        .replace('│', "|");
    out.push_str(&ascii);
    out.push('\n');
}

// ---------------------------------------------------------------------
// Outdated view
// ---------------------------------------------------------------------

#[derive(Clone)]
struct OutdatedRow {
    name: String,
    direct: bool,
    version: String,
    latest: String,
    marker: char,
    description: String,
    homepage: String,
    source_view: String,
    abandoned: Value,
    /// Installed and latest packages' own `time` field, `--format=json`'s
    /// `release-age`/`release-date`/`latest-release-date` (#94).
    time: Option<String>,
    latest_time: Option<String>,
}

/// `ShowCommand::findLatestPackage`/`VersionSelector::findBestCandidate`,
/// minus platform-requirement filtering (see the module doc): the
/// major/minor/patch-only synthetic constraint, then the same
/// preferred-stability-aware "highest acceptable version, or else just the
/// highest" pick `require.rs`'s own `version_selector::pick_best` already
/// implements for `viv add`.
async fn find_latest<T: crate::repository::Transport>(
    repo: &Repository<T>,
    name: &str,
    installed_version: &str,
    preferred_stability: &str,
    major_only: bool,
    minor_only: bool,
    patch_only: bool,
) -> Result<Option<PackageVersion>> {
    let is_dev = installed_version.starts_with("dev-");
    if is_dev && major_only {
        return Ok(None);
    }
    let target_version: Option<String> = if is_dev {
        Some(installed_version.to_string())
    } else if major_only {
        target_version_major_only(installed_version)
    } else if minor_only {
        Some(format!("^{installed_version}"))
    } else if patch_only {
        Some(target_version_patch_only(installed_version))
    } else {
        None
    };
    let constraint = target_version
        .as_deref()
        .map(semver::parse_constraint)
        .transpose()?;

    let dev = if preferred_stability == "dev" {
        DevAcceptance::Both
    } else {
        DevAcceptance::NonDevOnly
    };
    let versions = repo.load_package(name, dev).await?;
    let preferred_rank = stability_rank(preferred_stability);
    let mut best: Option<(&PackageVersion, semver::NormalizedVersion, &'static str)> = None;
    for candidate in &versions {
        let normalized = semver::normalize(&candidate.version_normalized)?;
        if let Some(constraint) = &constraint
            && !constraint.matches(&normalized)
        {
            continue;
        }
        let stability = semver::stability(normalized.as_str());
        let rank = stability_rank(stability);
        let candidate_is_worse = preferred_rank < rank;
        let better = match &best {
            None => true,
            Some((_, best_version, best_stability)) => {
                let best_rank = stability_rank(best_stability);
                let best_is_worse = preferred_rank < best_rank;
                if candidate_is_worse && !best_is_worse {
                    false
                } else if !candidate_is_worse && best_is_worse {
                    true
                } else {
                    semver::compare(&normalized, best_version) == std::cmp::Ordering::Greater
                }
            }
        };
        if better {
            best = Some((candidate, normalized, stability));
        }
    }
    Ok(best.map(|(candidate, ..)| candidate.clone()))
}

/// `Preg::isMatch('{^(?P<zero_major>(?:0\.)+)?(?P<first_meaningful>\d+)\.}', ...)`
/// then `>=<zero_major><first_meaningful+1>,<9999999-dev`.
fn target_version_major_only(installed_version: &str) -> Option<String> {
    let re = Regex::new(r"^(?P<zero_major>(?:0\.)+)?(?P<first_meaningful>\d+)\.").ok()?;
    let caps = re.captures(installed_version)?;
    let zero_major = caps.name("zero_major").map_or("", |m| m.as_str());
    let first_meaningful: u64 = caps.name("first_meaningful")?.as_str().parse().ok()?;
    Some(format!(
        ">={zero_major}{},<9999999-dev",
        first_meaningful + 1
    ))
}

/// `Preg::replace('{(\.0)+$}D', '', ...)` then pad back out to 3 (or 4 for
/// a `0.x` version) dot-separated parts before the `~` operator.
fn target_version_patch_only(installed_version: &str) -> String {
    let mut trimmed = installed_version;
    while let Some(stripped) = trimmed.strip_suffix(".0") {
        trimmed = stripped;
    }
    let parts_needed = if trimmed.starts_with('0') { 4 } else { 3 };
    let mut trimmed = trimmed.to_string();
    while trimmed.matches('.').count() + 1 < parts_needed {
        trimmed.push_str(".0");
    }
    format!("~{trimmed}")
}

fn print_outdated_text(rows: &[OutdatedRow], direct_only: bool) -> Result<()> {
    if !rows.is_empty() {
        err_out("Legend:");
        err_out("! patch or minor release available - update recommended");
        err_out("~ major release available - update possible");
    }
    if direct_only {
        let as_rows: Vec<Row> = rows.iter().map(outdated_row_as_row).collect();
        return print_list_text(&as_rows, true, true, Some(&LatestColumn { rows }));
    }
    let (direct, transitive): (Vec<&OutdatedRow>, Vec<&OutdatedRow>) =
        rows.iter().partition(|r| r.direct);
    err_out("");
    err_out("Direct dependencies required in composer.json:");
    print_outdated_group(&direct)?;
    err_out("");
    err_out("Transitive dependencies not required in composer.json:");
    print_outdated_group(&transitive)
}

fn print_outdated_group(rows: &[&OutdatedRow]) -> Result<()> {
    if rows.is_empty() {
        err_out("Everything up to date");
        return Ok(());
    }
    let owned: Vec<OutdatedRow> = rows.iter().map(|r| (*r).clone()).collect();
    let as_rows: Vec<Row> = owned.iter().map(outdated_row_as_row).collect();
    print_list_text(&as_rows, true, true, Some(&LatestColumn { rows: &owned }))
}

fn outdated_row_as_row(row: &OutdatedRow) -> Row {
    Row {
        name: row.name.clone(),
        direct: row.direct,
        version: row.version.clone(),
        description: row.description.clone(),
        homepage: row.homepage.clone(),
        source_view: row.source_view.clone(),
        abandoned: row.abandoned.clone(),
    }
}

fn print_outdated_json(rows: &[OutdatedRow]) -> Result<()> {
    let installed: Vec<Value> = rows
        .iter()
        .map(|row| {
            let mut obj = Map::new();
            obj.insert("name".into(), Value::String(row.name.clone()));
            obj.insert("direct-dependency".into(), Value::Bool(row.direct));
            obj.insert("homepage".into(), Value::String(row.homepage.clone()));
            obj.insert("source".into(), Value::String(row.source_view.clone()));
            obj.insert("version".into(), Value::String(row.version.clone()));
            let (release_age, release_date) = release_age_fields(row.time.as_deref());
            obj.insert("release-age".into(), Value::String(release_age));
            obj.insert("release-date".into(), Value::String(release_date));
            obj.insert("latest".into(), Value::String(row.latest.clone()));
            let status = match row.marker {
                '!' => "semver-safe-update",
                _ => "update-possible",
            };
            obj.insert("latest-status".into(), Value::String(status.to_string()));
            obj.insert(
                "latest-release-date".into(),
                Value::String(row.latest_time.clone().unwrap_or_default()),
            );
            obj.insert("description".into(), Value::String(row.description.clone()));
            obj.insert("abandoned".into(), row.abandoned.clone());
            Value::Object(obj)
        })
        .collect();
    write_json(&serde_json::json!({ "installed": installed }))
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr` lint.
fn err_out(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use crate::repository::{DevAcceptance, Repository, Transport};

    use super::find_latest;

    /// Same fixture-replaying transport as `tests/require.rs`/`tests/update.rs`.
    struct FixtureTransport {
        root: PathBuf,
    }

    impl Transport for &FixtureTransport {
        #[allow(clippy::unused_async_trait_impl)]
        async fn get(
            &self,
            url: &reqwest::Url,
            _if_modified_since: Option<&str>,
        ) -> anyhow::Result<crate::fetch::Conditional> {
            let path = self.root.join(url.path().trim_start_matches('/'));
            match fs_err::read(&path) {
                Ok(body) => Ok(crate::fetch::Conditional::Fresh {
                    body,
                    last_modified: None,
                }),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    Ok(crate::fetch::Conditional::NotFound)
                }
                Err(err) => Err(err.into()),
            }
        }
    }

    async fn monolog_repo(cache_root: &Path) -> Repository<&'static FixtureTransport> {
        let transport: &'static FixtureTransport = Box::leak(Box::new(FixtureTransport {
            root: Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/packagist/repo.packagist.org"),
        }));
        Repository::load("https://repo.packagist.org", cache_root, transport)
            .await
            .unwrap()
    }

    /// `viv outdated` on `tests/fixtures/show/outdated-monolog` (monolog
    /// pinned at 3.9.0): a real Composer 2.10.2 run against the same
    /// recorded fixture (`tests/fixtures/packagist/repo.packagist.org`, as
    /// a local `file://` repository) picked 3.11.0 as the latest with the
    /// default flags, matching `tests/fixtures/show/expected/outdated-default.txt`.
    #[tokio::test]
    async fn find_latest_matches_composers_recorded_outdated() {
        let cache = tempfile::tempdir().unwrap();
        let repo = monolog_repo(cache.path()).await;

        let latest = find_latest(
            &repo,
            "monolog/monolog",
            "3.9.0",
            "stable",
            false,
            false,
            false,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(latest.version, "3.11.0");
    }

    /// `--major-only` on the same 3.x install: no newer major exists, so
    /// `findLatestPackage` returns nothing (`outdated-major-only.txt` is
    /// empty — "Everything up to date").
    #[tokio::test]
    async fn find_latest_major_only_finds_nothing_within_the_same_major() {
        let cache = tempfile::tempdir().unwrap();
        let repo = monolog_repo(cache.path()).await;

        let latest = find_latest(
            &repo,
            "monolog/monolog",
            "3.9.0",
            "stable",
            true,
            false,
            false,
        )
        .await
        .unwrap();
        assert!(latest.is_none());
    }

    /// `--patch-only` on 3.9.0: the fixture has no other `3.9.x` release,
    /// so the best candidate within `~3.9.0` is 3.9.0 itself (already
    /// installed), matching `outdated-patch-only.txt`'s empty output.
    #[tokio::test]
    async fn find_latest_patch_only_stays_on_the_installed_version() {
        let cache = tempfile::tempdir().unwrap();
        let repo = monolog_repo(cache.path()).await;

        let latest = find_latest(
            &repo,
            "monolog/monolog",
            "3.9.0",
            "stable",
            false,
            false,
            true,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(latest.version, "3.9.0");
    }

    /// `--minor-only` on 3.9.0: `^3.9.0` still admits 3.11.0, matching
    /// `outdated-minor-only.txt`.
    #[tokio::test]
    async fn find_latest_minor_only_finds_the_latest_3x() {
        let cache = tempfile::tempdir().unwrap();
        let repo = monolog_repo(cache.path()).await;

        let latest = find_latest(
            &repo,
            "monolog/monolog",
            "3.9.0",
            "stable",
            false,
            true,
            false,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(latest.version, "3.11.0");
    }

    #[tokio::test]
    async fn find_latest_never_asks_for_the_dev_provider_file_unless_dev_stability() {
        let cache = tempfile::tempdir().unwrap();
        let repo = monolog_repo(cache.path()).await;
        // Stable-preferred lookups only ever fetch the non-dev provider
        // file; a missing `~dev` fixture would otherwise turn a bug in
        // `DevAcceptance` selection into a silent `Ok(vec![])` instead of a
        // loud test failure.
        let versions = repo
            .load_package("monolog/monolog", DevAcceptance::NonDevOnly)
            .await
            .unwrap();
        assert!(!versions.is_empty());
    }

    use super::{SpdxLicense, Ymd, relative_time, spdx_license};

    /// `composer/spdx-licenses`' own resource file, not the old
    /// hand-picked handful: a lowercase lookup on two identifiers
    /// (`SpdxLicenses::getLicenseByIdentifier` lowercases too, #94).
    #[test]
    fn spdx_license_resolves_known_identifiers_case_insensitively() {
        assert_eq!(
            spdx_license("Apache-2.0"),
            Some(&SpdxLicense {
                id: "Apache-2.0".to_string(),
                name: "Apache License 2.0".to_string(),
                osi: true,
            })
        );
        // The URL takes the list's casing, not the caller's.
        assert_eq!(
            spdx_license("bsd-3-clause").map(|l| l.id.as_str()),
            Some("BSD-3-Clause")
        );
    }

    /// `ShowCommand::getRelativeTime`: a real Composer 2.10.2
    /// `outdated --format=json` run on `outdated-monolog` (monolog/monolog
    /// 3.9.0, `time: "2025-03-24T10:02:05+00:00"`) recorded
    /// `"release-age": "1 year old"` on 2026-09-07 (`#94`).
    #[test]
    fn relative_time_matches_composers_recorded_release_age() {
        let release = Ymd {
            y: 2025,
            m: 3,
            d: 24,
        };
        let now = Ymd {
            y: 2026,
            m: 9,
            d: 7,
        };
        assert_eq!(relative_time(release, now), "1 year ago");
    }

    #[test]
    fn relative_time_same_day_is_today() {
        let day = Ymd {
            y: 2026,
            m: 9,
            d: 7,
        };
        assert_eq!(relative_time(day, day), "today");
    }
}
