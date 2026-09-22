//! `viv lock merge <base> <ours> <theirs>` (#275 chunk 1): a git merge
//! driver for `composer.lock`/`viv.lock` that merges by name-keyed package
//! record instead of by text line. `docs/research.md` chapter 1's
//! archaeology (`bench/results/lockmerge.md`) is why: of the merges where
//! the lock still conflicts textually, most are either format adjacency (no
//! package changed on both sides) or one package both sides changed to
//! different results while `composer.json` merged clean. A three-way merge
//! over a name-keyed record set has neither: every package is unchanged,
//! changed on one side, changed identically, or genuinely divergent, and
//! only the last needs a human.
//!
//! Chunk 2 replaces the marker output for a `composer.lock` input with a
//! re-solve of the divergent names' closure (`resolve_divergent_closure`),
//! keeping markers as the fallback when the solve can't finish (no network,
//! a dead package, a genuine constraint conflict in the merged manifest) or
//! when `--no-resolve` asks for chunk 1's behaviour outright.
//!
//! `viv.lock` inputs keep markers unconditionally in this chunk: a
//! `native_lock::Record` carries only `name`/`version`/`dist-url`/
//! `dist-hash`/`source-ref`/`dev`/`root-requirement` — no `require`, no
//! `autoload`, none of what [`solver::pool_builder::build_partial`] reads
//! off a locked-out entry to load it into the pool
//! (`package_from_lock_entry`) or off `locked_by_name`'s own `require` to
//! expand a `WithTransitiveDeps` closure. Pinning from the record alone
//! would either invent those fields or silently resolve as if every locked
//! package had none, understating the very cascade chapter 1's archaeology
//! measured. `composer.lock` is the primary target; teaching `viv.lock` the
//! same trick needs its own richer record, not a guess here.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::lock::{self, Lock};
use crate::lock_writer::{self, LockAggregates};
use crate::native_lock;
use crate::repository::{Repository, Transport};
use crate::solver::{self, pool_builder::UpdateAllowMode, transaction::ResolvedPackage};
use crate::update;

/// A locked package's comparable identity for merge purposes
/// (`docs/research.md` chapter 1): two records with the same identity are
/// the same resolution, whatever else differs in how they're stored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    version: String,
    source_ref: Option<String>,
    dev: bool,
}

/// One side's view of a name: its identity, plus enough payload to write it
/// back out untouched when it wins (the full `composer.lock` entry, or a
/// parsed `viv.lock` [`native_lock::Record`]).
#[derive(Debug, Clone)]
struct Entry<T> {
    identity: Identity,
    payload: T,
}

/// The record merge, format-independent: for every name in the union of
/// `base`/`ours`/`theirs`, this is the plain three-way merge of a single
/// value (`Option<Identity>`, `None` meaning "absent/removed") applied
/// per-name — unchanged on both sides, changed on exactly one side
/// (including added or removed), changed on both sides identically, or
/// changed on both sides differently (divergent, returned by name so the
/// caller can render markers). A winning identity is never synthesised:
/// the record itself is reused from whichever of `ours`/`theirs`/`base`
/// (in that order) actually carries it.
fn merge<T: Clone>(
    base: &BTreeMap<String, Entry<T>>,
    ours: &BTreeMap<String, Entry<T>>,
    theirs: &BTreeMap<String, Entry<T>>,
) -> (BTreeMap<String, Entry<T>>, BTreeSet<String>) {
    let mut merged = BTreeMap::new();
    let mut divergent = BTreeSet::new();
    let names: BTreeSet<&String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs.keys())
        .collect();

    for name in names {
        let b = base.get(name);
        let o = ours.get(name);
        let t = theirs.get(name);
        let b_id = b.map(|e| &e.identity);
        let o_id = o.map(|e| &e.identity);
        let t_id = t.map(|e| &e.identity);

        let winning_id = if o_id == t_id {
            o_id
        } else if o_id == b_id {
            t_id
        } else if t_id == b_id {
            o_id
        } else {
            divergent.insert(name.clone());
            continue;
        };

        let Some(winning_id) = winning_id else {
            continue; // removed on both sides (or absent everywhere).
        };

        let record = [o, t, b]
            .into_iter()
            .flatten()
            .find(|e| &e.identity == winning_id)
            .expect("a winning identity always comes from one of the three inputs")
            .clone();
        merged.insert(name.clone(), record);
    }

    (merged, divergent)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    ComposerLock,
    VivLock,
}

impl Format {
    fn name(self) -> &'static str {
        match self {
            Format::ComposerLock => "composer.lock (JSON)",
            Format::VivLock => "viv.lock (TOML)",
        }
    }
}

/// JSON object -> `composer.lock`; otherwise TOML -> `viv.lock`.
fn detect_format(path: &Path) -> Result<Format> {
    let content =
        fs_err::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    if serde_json::from_str::<Value>(&content).is_ok_and(|v| v.is_object()) {
        return Ok(Format::ComposerLock);
    }
    toml::from_str::<toml::Table>(&content).with_context(|| {
        format!(
            "{} is neither a composer.lock (JSON object) nor a viv.lock (TOML document)",
            path.display()
        )
    })?;
    Ok(Format::VivLock)
}

/// `viv lock merge <base> <ours> <theirs> -d DIR`: 0 when the result is
/// written clean (a re-solve included), 1 when it still holds conflict
/// markers a human must resolve.
#[expect(
    clippy::too_many_arguments,
    reason = "the three merge-driver paths plus --no-resolve/--as-of/--cache-dir/--offline"
)]
pub fn run(
    base: &Path,
    ours: &Path,
    theirs: &Path,
    project_dir: &Path,
    no_resolve: bool,
    as_of: Option<&str>,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<u8> {
    let as_of = as_of
        .map(|value| {
            lock_writer::parse_time_to_epoch(value)
                .with_context(|| format!("--as-of {value:?} is not a recognised timestamp"))
        })
        .transpose()?;

    let base_format =
        detect_format(base).with_context(|| format!("detecting {}'s format", base.display()))?;
    for (label, path) in [("ours", ours), ("theirs", theirs)] {
        let format = detect_format(path)
            .with_context(|| format!("detecting {label}'s format ({})", path.display()))?;
        if format != base_format {
            bail!(
                "base is {} but {label} is {}: viv lock merge requires all three inputs in the \
                 same format",
                base_format.name(),
                format.name()
            );
        }
    }

    match base_format {
        Format::ComposerLock => merge_composer_lock(
            base,
            ours,
            theirs,
            project_dir,
            no_resolve,
            as_of,
            cache_dir,
            offline,
        ),
        Format::VivLock => merge_viv_lock(base, ours, theirs),
    }
}

fn composer_entries(lock: &Lock) -> BTreeMap<String, Entry<Value>> {
    lock.packages
        .iter()
        .map(|package| {
            (
                package.name.clone(),
                Entry {
                    identity: Identity {
                        version: package.version.clone(),
                        source_ref: package.source.as_ref().and_then(|s| s.reference.clone()),
                        dev: package.dev,
                    },
                    payload: package.raw.clone(),
                },
            )
        })
        .collect()
}

/// A line-initial `<<<<<<<`/`>>>>>>>` git conflict marker: `composer.json`
/// left mid-merge is a source conflict this chunk cannot guess at (chapter
/// 1's archaeology: 17 of 355 merges, no lock format touches those).
fn has_conflict_markers(content: &[u8]) -> bool {
    content
        .split(|&b| b == b'\n')
        .any(|line| line.starts_with(b"<<<<<<<") || line.starts_with(b">>>>>>>"))
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors run's own --no-resolve/--as-of/--cache-dir/--offline, plus the three paths"
)]
fn merge_composer_lock(
    base: &Path,
    ours: &Path,
    theirs: &Path,
    project_dir: &Path,
    no_resolve: bool,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<u8> {
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path)
        .with_context(|| format!("reading {}", composer_json_path.display()))?;
    if has_conflict_markers(&composer_json) {
        bail!(
            "{} still has unresolved merge conflicts; resolve composer.json before the lock \
             merge can finish",
            composer_json_path.display()
        );
    }

    let base_lock = lock::read_lock(base).with_context(|| format!("reading {}", base.display()))?;
    let ours_lock = lock::read_lock(ours).with_context(|| format!("reading {}", ours.display()))?;
    let theirs_lock =
        lock::read_lock(theirs).with_context(|| format!("reading {}", theirs.display()))?;

    let base_entries = composer_entries(&base_lock);
    let ours_entries = composer_entries(&ours_lock);
    let theirs_entries = composer_entries(&theirs_lock);

    let (merged, divergent) = merge(&base_entries, &ours_entries, &theirs_entries);

    if !divergent.is_empty() && !no_resolve {
        match try_resolve_composer_lock(
            &merged,
            &divergent,
            &composer_json,
            project_dir,
            as_of,
            cache_dir,
            offline,
        ) {
            Ok(text) => {
                fs_err::write(ours, text)?;
                return Ok(0);
            }
            Err(err) => {
                let names: Vec<&str> = divergent.iter().map(String::as_str).collect();
                warn_out(&format!(
                    "viv lock merge: re-solving {} against the merged composer.json did not \
                     finish ({err:#}); falling back to conflict markers",
                    names.join(", ")
                ));
            }
        }
    }

    let mut non_dev = Vec::new();
    let mut dev = Vec::new();
    for (name, entry) in &merged {
        let resolved = ResolvedPackage {
            name: name.clone(),
            pretty_version: entry.identity.version.clone(),
            raw: entry.payload.clone(),
        };
        if entry.identity.dev {
            dev.push(resolved);
        } else {
            non_dev.push(resolved);
        }
    }

    let ours_value: Value =
        serde_json::from_slice(&fs_err::read(ours)?).context("re-parsing ours as JSON")?;
    let aggregates = LockAggregates::from_lock_value(&ours_value);
    let dev_present = ours_value.get("packages-dev").is_some_and(|v| !v.is_null());
    let dev_opt = dev_present.then_some(dev.as_slice());

    let doc = lock_writer::write(&non_dev, dev_opt, &aggregates.as_options(), &composer_json)?;

    if divergent.is_empty() {
        fs_err::write(ours, doc)?;
        return Ok(0);
    }

    let patched =
        inject_composer_conflicts(&doc, &merged, &divergent, &ours_entries, &theirs_entries)?;
    fs_err::write(ours, patched)?;
    Ok(1)
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr`
/// lint (`update.rs`'s own `warn_out` does the same).
fn warn_out(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

/// The merge's non-divergent records, in the exact shape
/// `update::locked_packages_by_name` builds from a real lock (lowercased
/// name -> its full `composer.lock` entry): wrapped in a throwaway
/// `packages`/`packages-dev` document just to hand it to that same
/// function, rather than re-deriving the mapping here.
fn locked_by_name_from_merge(merged: &BTreeMap<String, Entry<Value>>) -> HashMap<String, Value> {
    let mut packages = Vec::new();
    let mut packages_dev = Vec::new();
    for entry in merged.values() {
        if entry.identity.dev {
            packages_dev.push(entry.payload.clone());
        } else {
            packages.push(entry.payload.clone());
        }
    }
    update::locked_packages_by_name(&serde_json::json!({
        "packages": packages,
        "packages-dev": packages_dev,
    }))
}

/// Re-solves `allow_list`'s names and their locked dependency closure
/// (`UpdateAllowMode::WithTransitiveDeps` — see the `-W`/`-w` doc comment on
/// that enum; chapter 1's archaeology measured a cascade of median 0 but
/// max 14 *other* packages dragged along by a real resolution, so a mode
/// that excludes root-required names from the closure could hand back a
/// lock that doesn't satisfy its own manifest on the tail) against `root`,
/// leaving every other locked name exactly as `locked_by_name` states it.
/// `as_of` is `--as-of`'s parsed cutoff (epoch seconds UTC), threaded
/// straight to [`solver::solve_partial_update_as_of`]; `None` reproduces
/// [`solver::solve_partial_update`]'s own behaviour exactly (that function
/// is `solve_partial_update_as_of`'s own base case, not called here
/// separately, so there is only one code path to keep in sync with it).
/// `pub`, generic over [`Transport`]: `merge_composer_lock` always builds a
/// real `HttpTransport` repository, but a test can drive this directly
/// with a fixture-backed one instead (`tests/update.rs`'s own pattern).
#[expect(
    clippy::implicit_hasher,
    reason = "internal API, only ever called with the default hasher (mirrors \
              solver::solve_partial_update's own allowance)"
)]
pub async fn resolve_divergent_closure<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    locked_by_name: &HashMap<String, Value>,
    allow_list: &[String],
    as_of: Option<i64>,
) -> Result<solver::UpdateResult> {
    solver::solve_partial_update_as_of(
        repo,
        root,
        prefer_stable,
        false,
        locked_by_name,
        allow_list,
        UpdateAllowMode::WithTransitiveDeps,
        as_of,
    )
    .await
}

/// [`solver::UpdateResult`] -> the final `composer.lock` text, the same
/// `LockOptions` shape `update.rs`'s own full-update write uses.
fn write_resolved_lock(result: &solver::UpdateResult, composer_json: &[u8]) -> Result<String> {
    let options = lock_writer::LockOptions {
        minimum_stability: result.minimum_stability,
        stability_flags: &result.stability_flags,
        prefer_stable: result.prefer_stable,
        prefer_lowest: result.prefer_lowest,
        platform_reqs: &result.platform_reqs,
        platform_dev_reqs: &result.platform_dev_reqs,
        platform_overrides: &result.platform_overrides,
        aliases: &result.aliases,
    };
    lock_writer::write(&result.non_dev, Some(&result.dev), &options, composer_json)
}

/// The CLI path's re-solve attempt: builds the same fetcher/repository
/// sequence `update.rs`'s own `solve` does (`build_fetcher`,
/// `build_repository`, `default_cache_dir`), then
/// [`resolve_divergent_closure`] over it. Any failure here — no network, a
/// package no longer resolvable, a genuine constraint conflict in the
/// merged manifest — is the caller's cue to fall back to conflict markers,
/// so this never itself decides that markers are fine; it only reports why
/// the resolve didn't happen.
fn try_resolve_composer_lock(
    merged: &BTreeMap<String, Entry<Value>>,
    divergent: &BTreeSet<String>,
    composer_json: &[u8],
    project_dir: &Path,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<String> {
    let root: Value = serde_json::from_slice(composer_json).context("parsing composer.json")?;
    let locked_by_name = locked_by_name_from_merge(merged);
    let allow_list: Vec<String> = divergent.iter().cloned().collect();
    let prefer_stable = root
        .get("prefer-stable")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => update::default_cache_dir()?,
    };

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let fetcher = update::build_fetcher(project_dir, &root, offline)?;
        let metadata_ttl = update::metadata_ttl(None, offline);
        let repo = update::build_repository(&root, &cache_dir, &fetcher, metadata_ttl).await?;
        let resolved = resolve_divergent_closure(
            &repo,
            &root,
            prefer_stable,
            &locked_by_name,
            &allow_list,
            as_of,
        )
        .await;
        update::forget_repo(repo);
        write_resolved_lock(&resolved?, composer_json)
    })
}

/// One divergent name's marker block, `ours`' entry then `theirs`',
/// canonicalised through [`lock_writer::dump_package`] like every other
/// entry so a human resolving it by deleting one side is left with valid
/// JSON either way.
fn render_composer_entry(raw: &Value, trailing_comma: bool) -> Result<String> {
    let dumped = lock_writer::dump_package(raw)?;
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&dumped, &mut serializer)?;
    let text = String::from_utf8(buf)?;
    let mut out = text
        .lines()
        .map(|line| format!("        {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    if trailing_comma {
        out.push(',');
    }
    Ok(out)
}

fn render_composer_marker(ours: Option<&Value>, theirs: Option<&Value>) -> Result<String> {
    let mut lines = vec!["<<<<<<< ours".to_string()];
    if let Some(raw) = ours {
        lines.push(render_composer_entry(raw, true)?);
    }
    lines.push("=======".to_string());
    if let Some(raw) = theirs {
        lines.push(render_composer_entry(raw, true)?);
    }
    lines.push(">>>>>>> theirs".to_string());
    Ok(lines.join("\n"))
}

/// Rebuilds one `packages`/`packages-dev` array's body (everything between
/// its `[`/`]` lines) with the divergent names in `names` spliced in as
/// marker blocks, sorted alongside the clean entries by name.
fn render_array_body(
    clean: &[(&String, &Value)],
    names: &[&String],
    ours: &BTreeMap<String, Entry<Value>>,
    theirs: &BTreeMap<String, Entry<Value>>,
) -> Result<String> {
    enum Item<'a> {
        Clean(&'a Value),
        Divergent,
    }
    let mut items: Vec<(&String, Item)> = clean
        .iter()
        .map(|&(name, value)| (name, Item::Clean(value)))
        .chain(names.iter().map(|&name| (name, Item::Divergent)))
        .collect();
    items.sort_by_key(|(name, _)| (*name).clone());

    let mut chunks = Vec::with_capacity(items.len());
    for (name, item) in &items {
        let chunk = match item {
            Item::Clean(value) => render_composer_entry(value, true)?,
            Item::Divergent => render_composer_marker(
                ours.get(*name).map(|e| &e.payload),
                theirs.get(*name).map(|e| &e.payload),
            )?,
        };
        chunks.push(chunk);
    }
    // The array's own closing bracket follows, so the last entry must not
    // carry a trailing comma when it is an ordinary (non-marker) one.
    if let Some(last) = chunks.last_mut()
        && last.ends_with("},")
    {
        last.pop();
    }
    Ok(chunks.join("\n"))
}

/// Replaces the `"KEY": [ ... ]` array's body (`KEY` is `packages` or
/// `packages-dev`) in an already-rendered `composer.lock` text.
///
/// ponytail: only handles the array printed across multiple lines
/// (`serde_json`'s pretty printer inlines an empty array as `"KEY": [],`
/// on one line); a merge where every record of that dev-ness is divergent
/// leaves nothing clean to anchor on and this bails instead of guessing.
/// Upgrade path: special-case the single-line empty form if that turns up
/// on the corpus.
fn splice_array(lines: &mut Vec<String>, key: &str, body: &str) -> Result<()> {
    let open = format!("    \"{key}\": [");
    let start = lines
        .iter()
        .position(|line| line == &open)
        .with_context(|| format!("{key} array not found (or empty) in the merged composer.lock"))?;
    let close = (start + 1..lines.len())
        .find(|&i| lines[i] == "    ]," || lines[i] == "    ]")
        .with_context(|| format!("{key} array has no closing bracket"))?;
    let new_body: Vec<String> = body.lines().map(str::to_string).collect();
    lines.splice(start + 1..close, new_body);
    Ok(())
}

fn inject_composer_conflicts(
    doc: &str,
    merged: &BTreeMap<String, Entry<Value>>,
    divergent: &BTreeSet<String>,
    ours: &BTreeMap<String, Entry<Value>>,
    theirs: &BTreeMap<String, Entry<Value>>,
) -> Result<String> {
    let mut lines: Vec<String> = doc.lines().map(str::to_string).collect();

    for (key, dev) in [("packages", false), ("packages-dev", true)] {
        let names: Vec<&String> = divergent
            .iter()
            .filter(|name| {
                let record_dev = ours
                    .get(*name)
                    .or_else(|| theirs.get(*name))
                    .is_some_and(|e| e.identity.dev);
                record_dev == dev
            })
            .collect();
        if names.is_empty() {
            continue;
        }

        let clean: Vec<(&String, &Value)> = merged
            .iter()
            .filter(|(_, entry)| entry.identity.dev == dev)
            .map(|(name, entry)| (name, &entry.payload))
            .collect();

        let body = render_array_body(&clean, &names, ours, theirs)?;
        splice_array(&mut lines, key, &body)?;
    }

    Ok(lines.join("\n") + "\n")
}

fn viv_entries(records: Vec<native_lock::Record>) -> BTreeMap<String, Entry<native_lock::Record>> {
    records
        .into_iter()
        .map(|record| {
            (
                record.name.clone(),
                Entry {
                    identity: Identity {
                        version: record.version.clone(),
                        source_ref: record.source_ref.clone(),
                        dev: record.dev,
                    },
                    payload: record,
                },
            )
        })
        .collect()
}

fn render_viv_record(record: &native_lock::Record) -> Result<String> {
    Ok(native_lock::write_records(std::slice::from_ref(record))?
        .trim_end()
        .to_string())
}

fn render_viv_marker(
    ours: Option<&native_lock::Record>,
    theirs: Option<&native_lock::Record>,
) -> Result<String> {
    let mut lines = vec!["<<<<<<< ours".to_string()];
    if let Some(record) = ours {
        lines.push(render_viv_record(record)?);
    }
    lines.push("=======".to_string());
    if let Some(record) = theirs {
        lines.push(render_viv_record(record)?);
    }
    lines.push(">>>>>>> theirs".to_string());
    Ok(lines.join("\n"))
}

fn merge_viv_lock(base: &Path, ours: &Path, theirs: &Path) -> Result<u8> {
    let base_entries = viv_entries(native_lock::read(base)?);
    let ours_entries = viv_entries(native_lock::read(ours)?);
    let theirs_entries = viv_entries(native_lock::read(theirs)?);

    let (merged, divergent) = merge(&base_entries, &ours_entries, &theirs_entries);

    if divergent.is_empty() {
        let records: Vec<native_lock::Record> =
            merged.into_values().map(|entry| entry.payload).collect();
        fs_err::write(ours, native_lock::write_records(&records)?)?;
        return Ok(0);
    }

    let mut names: Vec<&String> = merged.keys().chain(divergent.iter()).collect();
    names.sort();

    let mut chunks = Vec::with_capacity(names.len());
    for name in names {
        let chunk = if divergent.contains(name) {
            render_viv_marker(
                ours_entries.get(name).map(|e| &e.payload),
                theirs_entries.get(name).map(|e| &e.payload),
            )?
        } else {
            render_viv_record(&merged[name].payload)?
        };
        chunks.push(chunk);
    }

    let mut text = chunks.join("\n\n");
    text.push('\n');
    fs_err::write(ours, text)?;
    Ok(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(version: &str, source_ref: Option<&str>, dev: bool) -> Entry<&'static str> {
        Entry {
            identity: Identity {
                version: version.to_string(),
                source_ref: source_ref.map(str::to_string),
                dev,
            },
            payload: "unused",
        }
    }

    fn map(pairs: &[(&str, Entry<&'static str>)]) -> BTreeMap<String, Entry<&'static str>> {
        pairs
            .iter()
            .map(|(name, entry)| ((*name).to_string(), entry.clone()))
            .collect()
    }

    /// The merge table `docs/research.md`/#275 specify, one row per case.
    #[test]
    fn merge_table() {
        struct Case {
            name: &'static str,
            base: Vec<(&'static str, Entry<&'static str>)>,
            ours: Vec<(&'static str, Entry<&'static str>)>,
            theirs: Vec<(&'static str, Entry<&'static str>)>,
            // Expected (name -> version) in the merged result.
            expect: Vec<(&'static str, &'static str)>,
            expect_divergent: Vec<&'static str>,
        }

        let v = |version: &'static str| entry(version, None, false);

        let cases = vec![
            Case {
                name: "unchanged on both sides",
                base: vec![("a", v("1.0"))],
                ours: vec![("a", v("1.0"))],
                theirs: vec![("a", v("1.0"))],
                expect: vec![("a", "1.0")],
                expect_divergent: vec![],
            },
            Case {
                name: "changed on ours only",
                base: vec![("a", v("1.0"))],
                ours: vec![("a", v("2.0"))],
                theirs: vec![("a", v("1.0"))],
                expect: vec![("a", "2.0")],
                expect_divergent: vec![],
            },
            Case {
                name: "changed on theirs only",
                base: vec![("a", v("1.0"))],
                ours: vec![("a", v("1.0"))],
                theirs: vec![("a", v("2.0"))],
                expect: vec![("a", "2.0")],
                expect_divergent: vec![],
            },
            Case {
                name: "added on ours only",
                base: vec![],
                ours: vec![("a", v("1.0"))],
                theirs: vec![],
                expect: vec![("a", "1.0")],
                expect_divergent: vec![],
            },
            Case {
                name: "removed on theirs only",
                base: vec![("a", v("1.0"))],
                ours: vec![("a", v("1.0"))],
                theirs: vec![],
                expect: vec![],
                expect_divergent: vec![],
            },
            Case {
                name: "changed on both sides to the same identity",
                base: vec![("a", v("1.0"))],
                ours: vec![("a", v("2.0"))],
                theirs: vec![("a", v("2.0"))],
                expect: vec![("a", "2.0")],
                expect_divergent: vec![],
            },
            Case {
                name: "changed on both sides to different identities",
                base: vec![("a", v("1.0"))],
                ours: vec![("a", v("2.0"))],
                theirs: vec![("a", v("3.0"))],
                expect: vec![],
                expect_divergent: vec!["a"],
            },
            Case {
                name: "added on both sides identically",
                base: vec![],
                ours: vec![("a", v("1.0"))],
                theirs: vec![("a", v("1.0"))],
                expect: vec![("a", "1.0")],
                expect_divergent: vec![],
            },
            Case {
                name: "added on both sides differently",
                base: vec![],
                ours: vec![("a", v("1.0"))],
                theirs: vec![("a", v("2.0"))],
                expect: vec![],
                expect_divergent: vec!["a"],
            },
            Case {
                name: "removed on one side, changed on the other",
                base: vec![("a", v("1.0"))],
                ours: vec![],
                theirs: vec![("a", v("2.0"))],
                expect: vec![],
                expect_divergent: vec!["a"],
            },
            Case {
                name: "dev flag flipped on one side only",
                base: vec![("a", entry("1.0", None, false))],
                ours: vec![("a", entry("1.0", None, true))],
                theirs: vec![("a", entry("1.0", None, false))],
                expect: vec![("a", "1.0")],
                expect_divergent: vec![],
            },
        ];

        for case in cases {
            let (merged, divergent) = merge(&map(&case.base), &map(&case.ours), &map(&case.theirs));
            let got: Vec<(&str, &str)> = merged
                .iter()
                .map(|(name, entry)| (name.as_str(), entry.identity.version.as_str()))
                .collect();
            assert_eq!(got, case.expect, "{}: merged records", case.name);
            let got_divergent: Vec<&str> = divergent.iter().map(String::as_str).collect();
            assert_eq!(
                got_divergent, case.expect_divergent,
                "{}: divergent names",
                case.name
            );
        }

        // The dev-flip case also needs the winning record to actually carry
        // the flipped flag, not just the version.
        let base = map(&[("a", entry("1.0", None, false))]);
        let ours = map(&[("a", entry("1.0", None, true))]);
        let theirs = map(&[("a", entry("1.0", None, false))]);
        let (merged, _) = merge(&base, &ours, &theirs);
        assert!(
            merged["a"].identity.dev,
            "dev flip on ours alone should win"
        );
    }

    #[test]
    fn has_conflict_markers_finds_a_line_initial_marker() {
        assert!(has_conflict_markers(b"a\n<<<<<<< HEAD\nb\n"));
        assert!(has_conflict_markers(b">>>>>>> branch\n"));
        assert!(!has_conflict_markers(b"{\"a\": 1}\n"));
    }
}
