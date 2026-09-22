//! Writes `viv.lock`, chapter 1's research lock format
//! (`docs/research.md`, #272): a second, TOML serialisation of the same
//! resolution `lock_writer::write` already turns into `composer.lock`. The
//! per-record fields and sort order are specified in `docs/research.md`'s
//! "Chapter 1" section; this module is the implementation of that spec, not
//! a second source of truth for it.
//!
//! Reading `viv.lock` back (`install`/`update` accepting it in place of
//! `composer.lock`) is a separate piece of work, not implemented here; the
//! `read`/`Record` this module does export are for `lock_merge` (#275),
//! which needs the parsed record shape, not a resolved package.
//!
//! `viv lock convert` (#273) is the other direction: an existing
//! `composer.lock`, translated into this format without re-solving, for
//! projects (and historical commits) adopting the format after the fact.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::lock::{self, Package};
use crate::solver::transaction::ResolvedPackage;

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
        /// requirements, the `content-hash` and (composer.lock only) the
        /// repositories a divergent name's re-solve fetches against.
        #[arg(short = 'd', long = "project-dir", default_value = ".")]
        project_dir: PathBuf,
        /// Skip the re-solve and go straight to chunk 1's conflict markers
        /// for any divergent name (composer.lock only; viv.lock always
        /// does this in chunk 2, see `lock_merge`'s module docs). For tests
        /// and offline use, where a network re-solve isn't wanted at all.
        #[arg(long)]
        no_resolve: bool,
        /// Resolve the divergent closure as the registry stood at this
        /// RFC 3339 timestamp: a released version Packagist's own `time`
        /// puts later than this is dropped from the pool, so a version that
        /// did not exist yet cannot be chosen. `dev-*` branch versions are
        /// never filtered this way — Packagist serves only a branch's
        /// current head, not a historical revision of one, so they always
        /// resolve to today's. Composer.lock only (no effect merging
        /// viv.lock inputs, which never re-solve in this chunk).
        #[arg(long, value_name = "TIMESTAMP")]
        as_of: Option<String>,
        /// How far a divergent name's re-solve may escalate before giving
        /// up on markers (#296, composer.lock only): `closure` is chunk 2's
        /// original scope (the divergent names plus their own locked
        /// closure), `dependents` adds pinned packages that directly
        /// require one of those, `seeded` (the default) is a full solve
        /// that prefers every non-divergent locked version but pins none
        /// of them hard.
        #[arg(long, value_enum, default_value = "seeded")]
        max_scope: crate::lock_merge::Scope,
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
        LockCommand::Merge {
            base,
            ours,
            theirs,
            project_dir,
            no_resolve,
            as_of,
            max_scope,
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
        .collect();
    records.sort_by(|a, b| a.name.cmp(&b.name));
    write_records(&records)
}

/// Reads `viv.lock`'s `[[package]]` blocks back into [`Record`]s (#275):
/// no reader existed before this, chapter 1 having shipped the writer only.
pub(crate) fn read(path: &Path) -> Result<Vec<Record>> {
    let content = fs_err::read_to_string(path).context("reading viv.lock")?;
    let doc: Document = toml::from_str(&content).context("parsing viv.lock")?;
    Ok(doc.package)
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
) -> Record {
    let dist = package.raw.get("dist");
    let source = package.raw.get("source");
    Record {
        name: package.name.clone(),
        version: package.pretty_version.clone(),
        dist_url: str_field(dist, "url"),
        dist_hash: str_field(dist, "shasum").filter(|s| !s.is_empty()),
        source_ref: str_field(source, "reference"),
        dev,
        root_requirement: root_requirements
            .get(&package.name.to_ascii_lowercase())
            .cloned(),
    }
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

    fn package(name: &str, version: &str, raw: Value) -> ResolvedPackage {
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
