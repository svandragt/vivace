//! Writes `viv.lock`, chapter 1's research lock format
//! (`docs/research.md`, #272): a second, TOML serialisation of the same
//! resolution `lock_writer::write` already turns into `composer.lock`. The
//! per-record fields and sort order are specified in `docs/research.md`'s
//! "Chapter 1" section; this module is the implementation of that spec, not
//! a second source of truth for it.
//!
//! `read`/`Record` are the shape `lock_merge` (#275) parses a lock into;
//! `reconcile` (#297) is `install`'s own use of the same reader, for a
//! project where both files are already present: `composer.lock` still
//! supplies each package's full entry then, and `install` refuses on any
//! mismatch rather than picking a side. `export` (#344) is the other use of
//! the same reader: once every record carries `raw`, `viv.lock` alone is
//! enough, and `install` generates or refreshes `composer.lock` from it
//! rather than requiring it up front.
//!
//! `viv lock convert` (#273) is the other direction: an existing
//! `composer.lock`, translated into this format without re-solving, for
//! projects (and historical commits) adopting the format after the fact.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lock::{self, Package};
use crate::solver::transaction::{AliasEntry, ResolvedPackage};

/// `viv lock` flags: which lock operation to run.
#[derive(Args, Debug, Clone)]
pub struct LockArgs {
    #[command(subcommand)]
    pub command: LockCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum LockCommand {
    /// Translate an existing `composer.lock` into `viv.lock` (#273):
    /// reads `DIR/composer.lock` and `DIR/composer.json`, and writes
    /// `DIR/viv.lock`, without re-solving anything.
    Convert {
        /// Project directory holding composer.json and composer.lock.
        #[arg(short = 'd', long = "project-dir", default_value = ".")]
        project_dir: PathBuf,
        /// Print the translated lock to stdout instead of writing
        /// `viv.lock` to disk.
        #[arg(long)]
        stdout: bool,
    },
    /// Write `DIR/composer.lock` from `DIR/viv.lock` and `DIR/composer.json`
    /// (#344), through the same `lock_writer::write` a solve feeds: a team
    /// that stops committing `composer.lock` regenerates it this way in CI
    /// or on checkout.
    Export {
        /// Project directory holding viv.lock and composer.json.
        #[arg(short = 'd', long = "project-dir", default_value = ".")]
        project_dir: PathBuf,
        /// Write nothing; exit 0 when the existing composer.lock already
        /// equals the export, 1 with a one-line diff summary otherwise.
        #[arg(long)]
        check: bool,
    },
    /// A git merge driver for `composer.lock`/`viv.lock` (#275): merges the
    /// three inputs by name-keyed package record instead of by text line,
    /// writes the result over `<ours>`, and exits 1 when a divergent
    /// name's own re-solve doesn't finish (a human decision then, not this
    /// chunk's job — see `docs/research.md` chapter 1). Git's merge-driver
    /// convention: `%O %A %B` (`git help gitattributes`'s "Defining a
    /// custom merge driver").
    Merge {
        /// The common ancestor's version of the lock (`%O`).
        base: PathBuf,
        /// This side's version (`%A`); overwritten with the merge result.
        ours: PathBuf,
        /// The other side's version (`%B`).
        theirs: PathBuf,
        /// Project directory whose composer.json supplies the root
        /// requirements and the repositories a divergent name's re-solve
        /// fetches against; for composer.lock, also the `content-hash`, and
        /// for viv.lock (#295), the sibling composer.lock the pinned set's
        /// `require` is read off.
        #[arg(short = 'd', long = "project-dir", default_value = ".")]
        project_dir: PathBuf,
        /// Skip the re-solve and go straight to chunk 1's conflict markers
        /// for any divergent name, in either format. For tests and offline
        /// use, where a network re-solve isn't wanted at all.
        #[arg(long)]
        no_resolve: bool,
        /// Resolve the divergent closure as the registry stood at this
        /// RFC 3339 timestamp: a released version Packagist's own `time`
        /// puts later than this is dropped from the pool, so a version that
        /// did not exist yet cannot be chosen. `dev-*` branch versions are
        /// never filtered this way — Packagist serves only a branch's
        /// current head, not a historical revision of one, so they always
        /// resolve to today's.
        #[arg(long, value_name = "TIMESTAMP")]
        as_of: Option<String>,
        /// How far a divergent name's re-solve may escalate before giving
        /// up on markers (#296): `closure` is chunk 2's original scope (the
        /// divergent names plus their own locked closure), `dependents`
        /// adds pinned packages that directly require one of those,
        /// `seeded` (the default) is a full solve that prefers every
        /// non-divergent locked version but pins none of them hard.
        #[arg(long, value_enum, default_value = "seeded")]
        max_scope: crate::lock_merge::Scope,
        /// Try one further, offline rung (#314) once the registry
        /// escalation above has already failed at every rung `--max-scope`
        /// allowed: one parent's own pinned record per divergent name
        /// (`ours` first, then `theirs`), checked against the two locks'
        /// own `require`/`conflict`/`replace`/`provide`/platform data with
        /// no registry fetch at all. Only ever a last resort before
        /// conflict markers, so it can never disagree with a registry
        /// answer that also finished — it only ever fires when there
        /// wasn't one. Off by default; a no-op for `viv.lock`, whose
        /// records carry none of the fields the check needs.
        #[arg(long)]
        offline_rung: bool,
    },
}

pub fn run(args: &LockArgs, cache_dir: Option<&Path>, offline: bool) -> Result<u8> {
    match &args.command {
        LockCommand::Convert {
            project_dir,
            stdout,
        } => {
            run_convert(project_dir, *stdout)?;
            Ok(0)
        }
        LockCommand::Export { project_dir, check } => run_export(project_dir, *check),
        LockCommand::Merge {
            base,
            ours,
            theirs,
            project_dir,
            no_resolve,
            as_of,
            max_scope,
            offline_rung,
        } => crate::lock_merge::run(
            base,
            ours,
            theirs,
            project_dir,
            *no_resolve,
            as_of.as_deref(),
            cache_dir,
            offline,
            *max_scope,
            *offline_rung,
        ),
    }
}

fn run_convert(project_dir: &Path, to_stdout: bool) -> Result<()> {
    let converted = convert(project_dir)?;
    if to_stdout {
        use std::io::Write as _;
        write!(std::io::stdout().lock(), "{converted}")?;
        return Ok(());
    }
    fs_err::write(project_dir.join("viv.lock"), converted)?;
    Ok(())
}

/// `DIR/composer.lock` + `DIR/composer.json` -> `viv.lock`'s body, without
/// re-solving: each lock entry's own `raw` (the untouched JSON object,
/// `dist`/`source` nested exactly like a pool package's provider-file raw)
/// already carries everything `record` reads, so it is reused as
/// [`ResolvedPackage::raw`] as-is rather than widening that type or adding a
/// narrower one just for this path.
pub fn convert(project_dir: &Path) -> Result<String> {
    let root: Value = serde_json::from_str(
        &fs_err::read_to_string(project_dir.join("composer.json"))
            .context("reading composer.json")?,
    )
    .context("parsing composer.json")?;
    let parsed =
        lock::read_lock(&project_dir.join("composer.lock")).context("reading composer.lock")?;
    let non_dev: Vec<ResolvedPackage> = parsed.packages(false).map(resolved).collect();
    let dev: Vec<ResolvedPackage> = parsed
        .packages
        .iter()
        .filter(|p| p.dev)
        .map(resolved)
        .collect();
    write(&non_dev, &dev, &root)
}

/// A locked package, as-is: `version` is already the pretty version
/// Composer wrote, and `raw` is the lock entry itself.
fn resolved(package: &Package) -> ResolvedPackage {
    ResolvedPackage {
        name: package.name.clone(),
        pretty_version: package.version.clone(),
        raw: package.raw.clone(),
    }
}

/// `viv lock export --check`: write nothing, report whether `DIR/composer.lock`
/// already equals what `export` would write.
fn run_export(project_dir: &Path, check: bool) -> Result<u8> {
    use std::io::Write as _;
    let generated = export(project_dir)?;
    if !check {
        fs_err::write(project_dir.join("composer.lock"), generated)?;
        return Ok(0);
    }
    match fs_err::read_to_string(project_dir.join("composer.lock")) {
        Ok(existing) if existing == generated => Ok(0),
        Ok(existing) => {
            writeln!(
                std::io::stdout().lock(),
                "{}",
                diff_summary(&existing, &generated)
            )?;
            Ok(1)
        }
        Err(_) => {
            writeln!(std::io::stdout().lock(), "composer.lock does not exist")?;
            Ok(1)
        }
    }
}

/// One line naming where two lock bodies first disagree, for `--check`'s
/// exit-1 report: the line number is enough to point a reader at the spot,
/// a full multi-line diff being a job for `diff composer.lock` against the
/// export, not this summary.
fn diff_summary(existing: &str, generated: &str) -> String {
    let existing_lines = existing.lines();
    let generated_lines = generated.lines();
    for (n, (a, b)) in existing_lines.zip(generated_lines).enumerate() {
        if a != b {
            return format!(
                "composer.lock differs from `viv lock export` at line {}: {:?} vs {:?}",
                n + 1,
                a,
                b
            );
        }
    }
    format!(
        "composer.lock differs from `viv lock export` in length: {} lines vs {}",
        existing.lines().count(),
        generated.lines().count()
    )
}

/// `DIR/viv.lock` + `DIR/composer.json` -> `composer.lock`'s bytes (#344),
/// through the same [`crate::lock_writer::write`] a solve feeds
/// (`update.rs`'s `lock_json`): "fed from viv.lock records instead of the
/// solve result", not a second formatter. Every record needs its own `raw`
/// (added by #344 for exactly this, alongside `time` in #347); a record
/// written before that change names its own package in the error, since
/// re-running `viv lock convert`/`viv update --lock native` is the fix, not
/// a partial `composer.lock`.
///
/// The aggregate fields no record carries (`minimum-stability`,
/// `stability-flags`, `prefer-stable`, `platform`/`platform-dev`,
/// `aliases`) are re-derived from root `composer.json` alone
/// (`pool_builder::root_lock_aggregates`, the same derivation a fresh solve
/// does, minus the closure walk), matched against which alias target a
/// record actually resolved to. `prefer-lowest` is the one exception: it
/// has no manifest source at all, only ever set from the test-only `update
/// --prefer-lowest` CLI flag, never persisted anywhere `export` can read —
/// so it defaults `false`, Composer's own default; a project whose lock was
/// written with that flag set needs composer.lock committed until #344
/// grows a place to record it.
pub fn export(project_dir: &Path) -> Result<String> {
    let root: Value = serde_json::from_str(
        &fs_err::read_to_string(project_dir.join("composer.json"))
            .context("reading composer.json")?,
    )
    .context("parsing composer.json")?;
    let composer_json =
        fs_err::read(project_dir.join("composer.json")).context("reading composer.json")?;
    let records = read(&project_dir.join("viv.lock"))?;

    let mut non_dev = Vec::new();
    let mut dev = Vec::new();
    for record in &records {
        let raw = record
            .raw
            .as_deref()
            .with_context(|| {
                format!(
                    "{}: viv.lock has no stored provider entry to export from (re-run `viv lock \
                     convert` or `viv update --lock native`)",
                    record.name
                )
            })
            .and_then(|raw| {
                serde_json::from_str::<Value>(raw).with_context(|| {
                    format!("{}: parsing viv.lock's stored raw entry", record.name)
                })
            })?;
        let resolved = ResolvedPackage {
            name: record.name.clone(),
            pretty_version: record.version.clone(),
            raw,
        };
        if record.dev {
            dev.push(resolved);
        } else {
            non_dev.push(resolved);
        }
    }

    let aggregates = crate::solver::pool_builder::root_lock_aggregates(&root)?;
    let by_name: HashMap<&str, &Record> = records.iter().map(|r| (r.name.as_str(), r)).collect();
    let aliases = used_aliases(&aggregates.root_aliases, &by_name)?;
    let options = crate::lock_writer::LockOptions {
        minimum_stability: aggregates.minimum_stability,
        stability_flags: &aggregates.stability_flags,
        prefer_stable: aggregates.prefer_stable,
        prefer_lowest: false,
        platform_reqs: &aggregates.platform_reqs,
        platform_dev_reqs: &aggregates.platform_dev_reqs,
        platform_overrides: &aggregates.platform_overrides,
        aliases: &aliases,
    };
    let generated = crate::lock_writer::write(&non_dev, Some(&dev), &options, &composer_json)?;
    let isolated_root = crate::lock::root_from_value(&root)?;
    with_isolate_extra(generated, &crate::isolate::prefix_map(&isolated_root))
}

/// #352: Composer's own lock format carries no top-level `extra` key
/// (`Locker::setLockData` never writes one), but it ignores unknown
/// top-level keys reading one back in — `devbox run -- composer validate
/// --no-check-all` against a lock built this way is the test for that, not
/// a guess. Added only once a project has at least one isolated plugin, so
/// one with none exports byte-identical to before this existed (the whole
/// reason this is a post-processing pass over `lock_writer::write`'s own
/// output rather than a new field on [`crate::lock_writer::LockOptions`]
/// threaded through every one of its other callers, none of which have
/// anything to say about `extra.viv.isolate`).
fn with_isolate_extra(
    generated: String,
    isolate: &std::collections::BTreeMap<String, String>,
) -> Result<String> {
    if isolate.is_empty() {
        return Ok(generated);
    }
    let mut value: Value =
        serde_json::from_str(&generated).context("parsing the generated composer.lock")?;
    let obj = value
        .as_object_mut()
        .context("the generated composer.lock must be a JSON object")?;
    obj.insert(
        "extra".to_string(),
        serde_json::json!({"viv": {"isolate": isolate}}),
    );
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&value, &mut serializer)?;
    buf.push(b'\n');
    Ok(String::from_utf8(buf)?)
}

/// Which of `root_aliases`' declared alias targets (`extract_alias`'s own
/// output, keyed by name) an exported record actually resolved to
/// (`transaction::used_aliases`'s pool-based match, done here against
/// `viv.lock`'s own resolved version instead of a solved pool id): a
/// declared alias for a name with no matching resolved version is simply
/// unused, same as a real solve never creating that alias package.
fn used_aliases(
    root_aliases: &HashMap<String, Vec<(String, String, String)>>,
    by_name: &HashMap<&str, &Record>,
) -> Result<Vec<AliasEntry>> {
    let mut aliases = Vec::new();
    for (name, targets) in root_aliases {
        let Some(record) = by_name.get(name.as_str()) else {
            continue;
        };
        let normalized_version = crate::semver::normalize(&record.version)?;
        for (version, alias, alias_normalized) in targets {
            if normalized_version.as_str() == version {
                aliases.push(AliasEntry {
                    package: record.name.clone(),
                    version: record.version.clone(),
                    alias: alias.clone(),
                    alias_normalized: alias_normalized.clone(),
                });
            }
        }
    }
    aliases.sort_by(|a, b| a.package.cmp(&b.package));
    Ok(aliases)
}

#[derive(Serialize, Deserialize)]
struct Document {
    package: Vec<Record>,
}

/// One `[[package]]` block. Deliberately excludes every field
/// `composer.lock` keeps at the file level (`content-hash`,
/// `plugin-api-version`, `platform`) and the `packages`/`packages-dev`
/// split (`dev` carries that instead): see `docs/research.md` chapter 1 for
/// why. `pub(crate)`/fields `pub(crate)`: `lock_merge` (#275) reads and
/// carries these whole, rather than a narrower type just for that path.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub(crate) struct Record {
    pub(crate) name: String,
    pub(crate) version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dist_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) dist_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) source_ref: Option<String>,
    pub(crate) dev: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) root_requirement: Option<String>,
    /// The resolved package's commit/release time, RFC 3339 as
    /// `composer.lock`'s own `time` carries it (#347): absent when the
    /// resolution has none, same as every other optional field here. Last
    /// in the struct, matching `dump_package`'s own "`time` moves to the
    /// end" rule for `composer.lock` (`lock_writer::record_time`, the
    /// shared source of the value both writers put here).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) time: Option<String>,
    /// The package's `composer.lock` block, already through
    /// [`crate::lock_writer::dump_package`], as compact JSON text: not
    /// `ResolvedPackage::raw` untouched, because a fresh solve's raw (a
    /// provider file's own entry, extra keys such as `version_normalized`/
    /// `published-time`, its own key order) and `convert`'s raw (already a
    /// dumped `composer.lock` entry) must produce the same record for the
    /// same resolution, and `dump_package`'s own canonical shape is the one
    /// place both agree. Every other field on this record is a lossy
    /// projection of it (only `dist.url`/`dist.shasum`/`source.reference`,
    /// never `require`/`autoload`/`license`/the rest of `docs/research.md`
    /// chapter 1's "no require/autoload/metadata"), so `viv lock export`
    /// (#344) needed the entry itself, not one more typed field, to
    /// reconstruct a package block byte-for-byte — feeding it straight back
    /// through `dump_package` is then a no-op re-canonicalisation, not a
    /// second, different formatting pass. A JSON string rather
    /// than a nested TOML table: `toml`'s own table type does not promise
    /// to keep a map's key order through a parse, and `dump_package`'s
    /// output depends on it (an object's key order inside an array field
    /// such as `authors`, which `KEY_ORDER` does not itself re-sort).
    /// Added after `time` the same way (#347): optional, so a record
    /// written before this change simply has none and cannot be exported
    /// until `viv lock convert`/`viv update --lock native` runs again.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) raw: Option<String>,
}

/// `viv.lock`'s body: one record per resolved package (`non_dev` and `dev`
/// folded into a single sorted list), no aggregate fields.
pub fn write(non_dev: &[ResolvedPackage], dev: &[ResolvedPackage], root: &Value) -> Result<String> {
    let root_requirements = root_requirements(root);
    let mut records: Vec<Record> = non_dev
        .iter()
        .map(|package| record(package, false, &root_requirements))
        .chain(
            dev.iter()
                .map(|package| record(package, true, &root_requirements)),
        )
        .collect::<Result<_>>()?;
    records.sort_by(|a, b| a.name.cmp(&b.name));
    write_records(&records)
}

/// Reads `viv.lock`'s `[[package]]` blocks back into [`Record`]s (#275):
/// no reader existed before this, chapter 1 having shipped the writer only.
pub(crate) fn read(path: &Path) -> Result<Vec<Record>> {
    let content = fs_err::read_to_string(path).context("reading viv.lock")?;
    parse(&content)
}

/// The parse half of [`read`] (#299), for a caller already holding
/// `viv.lock`'s bytes in memory (a git index-stage read has no real file to
/// read from).
pub(crate) fn parse(content: &str) -> Result<Vec<Record>> {
    let doc: Document = toml::from_str(content).context("parsing viv.lock")?;
    Ok(doc.package)
}

/// `install`'s adapter onto the #275 reader (#297): `viv.lock` decides the
/// locked set and each record's own `(version, source-ref, dev)`, but a
/// record carries no `require`/`autoload`/metadata, which
/// `installed.json`/`installed.php` need — so `composer.lock` stays the
/// source of each package's full entry, matched by name and identity.
/// Refuses unless the two files name the same set with the same identities
/// (a record with no matching entry, or an entry with no record), since
/// silently picking one side would make `viv.lock` a fork of `composer.lock`
/// instead of a companion to it (`docs/research.md` chapter 1).
pub fn reconcile(lock: &mut lock::Lock, viv_lock_path: &Path) -> Result<()> {
    let records = read(viv_lock_path)?;
    let by_name: HashMap<&str, &Package> = lock
        .packages
        .iter()
        .map(|package| (package.name.as_str(), package))
        .collect();

    let mut mismatches = Vec::new();
    let mut keep = std::collections::HashSet::with_capacity(records.len());
    for record in &records {
        let name = record.name.to_ascii_lowercase();
        match by_name.get(name.as_str()) {
            Some(package) if identity_matches(record, package) => {
                keep.insert(name);
            }
            Some(package) => mismatches.push(format!(
                "{}: viv.lock has {}, composer.lock has {}",
                record.name,
                identity(&record.version, record.source_ref.as_deref(), record.dev),
                identity(
                    &package.version,
                    package.source.as_ref().and_then(|s| s.reference.as_deref()),
                    package.dev
                )
            )),
            None => mismatches.push(format!(
                "{}: viv.lock has {}, absent from composer.lock",
                record.name,
                identity(&record.version, record.source_ref.as_deref(), record.dev)
            )),
        }
    }
    // The other direction matters as much: a package `composer.lock`
    // gained (a Composer `require`, say) that `viv.lock` never saw would
    // otherwise vanish from vendor/ without a word.
    for package in &lock.packages {
        if !keep.contains(&package.name) {
            mismatches.push(format!(
                "{}: composer.lock has {}, absent from viv.lock",
                package.name,
                identity(
                    &package.version,
                    package.source.as_ref().and_then(|s| s.reference.as_deref()),
                    package.dev
                )
            ));
        }
    }
    if !mismatches.is_empty() {
        mismatches.push("run `viv update --lock native` to bring them back in step".to_string());
        mismatches.push(
            "run `viv lock export` to rewrite composer.lock from viv.lock, or `viv lock convert` \
             to rewrite viv.lock from composer.lock"
                .to_string(),
        );
        bail!(mismatches.join("\n"));
    }
    Ok(())
}

fn identity_matches(record: &Record, package: &Package) -> bool {
    record.version == package.version
        && record.source_ref.as_deref()
            == package.source.as_ref().and_then(|s| s.reference.as_deref())
        && record.dev == package.dev
}

fn identity(version: &str, source_ref: Option<&str>, dev: bool) -> String {
    match source_ref {
        Some(reference) => format!("version {version}, ref {reference}, dev={dev}"),
        None => format!("version {version}, dev={dev}"),
    }
}

/// Serialises already-built [`Record`]s as-is, no resolution or
/// `root-requirement` derivation: `write`'s own tail, and `lock_merge`'s
/// clean (non-divergent) path, which already has the winning side's
/// records and needs only [`Document`]'s sort/shape, not a fresh solve.
pub(crate) fn write_records(records: &[Record]) -> Result<String> {
    toml::to_string_pretty(&Document {
        package: records.to_vec(),
    })
    .context("serialising viv.lock")
}

/// One record: `dist`/`source` come off `ResolvedPackage::raw`, the same
/// provider-file JSON `lock_writer::dump_package` reads for
/// `composer.lock`, so this never re-fetches or re-derives anything the
/// resolution didn't already produce.
fn record(
    package: &ResolvedPackage,
    dev: bool,
    root_requirements: &HashMap<String, String>,
) -> Result<Record> {
    let dist = package.raw.get("dist");
    let source = package.raw.get("source");
    // #344's `raw` stores `dump_package`'s own canonical shape, not
    // `package.raw` untouched: a fresh solve's raw is a provider file's own
    // entry (extra keys such as `version_normalized`/`published-time`, its
    // own key order) while `convert`'s raw is already a dumped
    // `composer.lock` entry, and the two must produce the same record for
    // the same resolution (`native_lock_reproduces_the_monolog_viv_lock`).
    // `dump_package` is idempotent on its own output, so this also means
    // `export` feeding a record's `raw` straight back into it is a no-op
    // re-canonicalisation, not a second, different formatting pass.
    let dumped = crate::lock_writer::dump_package(&package.raw)?;
    Ok(Record {
        name: package.name.clone(),
        version: package.pretty_version.clone(),
        dist_url: str_field(dist, "url"),
        dist_hash: str_field(dist, "shasum").filter(|s| !s.is_empty()),
        source_ref: str_field(source, "reference"),
        dev,
        root_requirement: root_requirements
            .get(&package.name.to_ascii_lowercase())
            .cloned(),
        time: crate::lock_writer::record_time(&package.raw),
        raw: Some(serde_json::to_string(&dumped).expect("dump_package's own output is valid JSON")),
    })
}

fn str_field(object: Option<&Value>, key: &str) -> Option<String> {
    object
        .and_then(|value| value.get(key))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The root `composer.json`'s `require`/`require-dev` names, lowercased,
/// mapped to their constraint string: the honest "root requirement that
/// selected it" for a record is exactly the one root composer.json already
/// states for that package's name, nothing inferred from the solve. A
/// package pulled in transitively (never named by root) has no root
/// requirement and the field is left off its record.
fn root_requirements(root: &Value) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for key in ["require", "require-dev"] {
        let Some(entries) = root.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (name, constraint) in entries {
            if let Some(constraint) = constraint.as_str() {
                map.insert(name.to_ascii_lowercase(), constraint.to_string());
            }
        }
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `raw` gains `name`/`version` if the caller's fixture omitted them:
    /// `dump_package` (#344's canonicalisation in `record`) requires both,
    /// same as every real provider/lock entry already carries them.
    fn package(name: &str, version: &str, mut raw: Value) -> ResolvedPackage {
        let object = raw.as_object_mut().expect("raw fixture is a JSON object");
        object
            .entry("name")
            .or_insert_with(|| Value::String(name.to_string()));
        object
            .entry("version")
            .or_insert_with(|| Value::String(version.to_string()));
        ResolvedPackage {
            name: name.to_string(),
            pretty_version: version.to_string(),
            raw,
        }
    }

    #[test]
    fn records_are_sorted_by_name_and_carry_the_root_requirement() {
        let root = json!({"require": {"acme/widget": "^2.0"}});
        let non_dev = vec![
            package(
                "acme/widget",
                "2.3.0",
                json!({
                    "dist": {"url": "https://example.test/widget.zip", "shasum": "abc"},
                    "source": {"reference": "deadbeef"},
                }),
            ),
            package("acme/aardvark", "1.0.0", json!({})),
        ];
        let got = write(&non_dev, &[], &root).unwrap();
        let parsed: toml::Table = toml::from_str(&got).unwrap();
        let packages = parsed["package"].as_array().unwrap();
        assert_eq!(packages[0]["name"].as_str(), Some("acme/aardvark"));
        assert_eq!(packages[1]["name"].as_str(), Some("acme/widget"));
        assert_eq!(packages[1]["root-requirement"].as_str(), Some("^2.0"));
        assert!(packages[0].get("root-requirement").is_none());
        assert_eq!(
            packages[1]["dist-url"].as_str(),
            Some("https://example.test/widget.zip")
        );
        assert_eq!(packages[1]["source-ref"].as_str(), Some("deadbeef"));
    }

    #[test]
    fn an_empty_dist_shasum_is_omitted() {
        let root = json!({});
        let non_dev = vec![package(
            "acme/widget",
            "1.0.0",
            json!({"dist": {"url": "https://example.test/widget.zip", "shasum": ""}}),
        )];
        let got = write(&non_dev, &[], &root).unwrap();
        assert!(!got.contains("dist-hash"));
    }

    #[test]
    fn dev_flag_matches_which_list_a_package_came_from() {
        let root = json!({});
        let non_dev = vec![package("acme/prod", "1.0.0", json!({}))];
        let dev = vec![package("acme/dev", "1.0.0", json!({}))];
        let got = write(&non_dev, &dev, &root).unwrap();
        let parsed: toml::Table = toml::from_str(&got).unwrap();
        let packages = parsed["package"].as_array().unwrap();
        let by_name = |name: &str| {
            packages
                .iter()
                .find(|p| p["name"].as_str() == Some(name))
                .unwrap()
        };
        assert_eq!(by_name("acme/prod")["dev"].as_bool(), Some(false));
        assert_eq!(by_name("acme/dev")["dev"].as_bool(), Some(true));
    }

    /// #275's reader requirement: reading the committed fixture and writing
    /// it straight back must reproduce it byte-for-byte, or `lock_merge`'s
    /// clean path (read three, merge, write one) would drift from what
    /// chapter 1's writer itself produces.
    #[test]
    fn viv_lock_round_trips_through_read_and_write() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog/viv.lock");
        let original = fs_err::read_to_string(&path).unwrap();
        let records = read(&path).unwrap();
        let rewritten = write_records(&records).unwrap();
        assert_eq!(rewritten, original);
    }
}
