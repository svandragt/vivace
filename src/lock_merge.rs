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
//! `viv.lock` inputs re-solve the same way (#295): a `native_lock::Record`
//! carries only `name`/`version`/`dist-url`/`dist-hash`/`source-ref`/`dev`/
//! `root-requirement` — no `require`, no `autoload`, none of what
//! [`solver::pool_builder::build_partial`] reads off a locked-out entry to
//! load it into the pool (`package_from_lock_entry`) or off
//! `locked_by_name`'s own `require` to expand a `WithTransitiveDeps`
//! closure. Rather than invent those fields, [`viv_lock_payloads`] sources
//! the non-divergent pinned set's payload from the sibling `composer.lock`
//! (#297 made one always sit beside `viv.lock`) keyed by name; the
//! divergent names themselves never need a payload, since rung 1's own
//! allow list re-solves them fresh regardless of what was locked. Missing
//! that sibling file falls back to markers, same as any other resolve
//! failure, but named up front rather than surfacing as an obscure lookup
//! error.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
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

/// `--max-scope` (#296): how far a divergent re-solve may escalate before
/// giving up on markers. Declaration order is rung order — `derive(Ord)`
/// makes `scope <= max_scope` the cap check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, clap::ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum Scope {
    /// Rung 1: the divergent names plus their own locked transitive
    /// closure, everything else pinned hard (chunk 2's original behaviour).
    Closure,
    /// Rung 2: rung 1 plus any pinned package that directly requires a
    /// name in the closure.
    Dependents,
    /// Rung 3: a full solve over the merged manifest with every
    /// non-divergent locked version passed as `--minimal-changes`'s
    /// `preferred` pin set. Never a plain update: nothing is pinned hard,
    /// but a satisfying solution that keeps a preferred version is always
    /// preferred over one that doesn't.
    Seeded,
}

impl Scope {
    fn name(self) -> &'static str {
        match self {
            Scope::Closure => "closure",
            Scope::Dependents => "dependents",
            Scope::Seeded => "seeded",
        }
    }

    fn rung(self) -> u8 {
        match self {
            Scope::Closure => 1,
            Scope::Dependents => 2,
            Scope::Seeded => 3,
        }
    }
}

/// One package outside the divergent set whose resolved identity differs
/// from both branches (`docs/research.md`/#296's "say what moved"):
/// normally empty for a rung-1 resolution, which only ever produces the
/// divergent names themselves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    pub name: String,
    /// Both branches' version when they agreed on one differing from the
    /// result, `"ours X, theirs Y"` when only one side had changed it
    /// (still not divergent — see `merge`'s own doc), or `"new"` when
    /// neither branch had this name locked at all.
    pub before: String,
    pub after: String,
}

/// A locked package's comparable identity for merge purposes
/// (`docs/research.md` chapter 1): two records with the same identity are
/// the same resolution, whatever else differs in how they're stored.
/// `pub`: [`escalate_resolve`]'s own `ours`/`theirs` parameters are keyed by
/// this, and a test builds them directly (`tests/lock_merge.rs`'s own
/// pattern, mirroring [`resolve_divergent_closure`]'s existing exposure).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub version: String,
    pub source_ref: Option<String>,
    pub dev: bool,
}

/// One side's view of a name: its identity, plus enough payload to write it
/// back out untouched when it wins (the full `composer.lock` entry, or a
/// parsed `viv.lock` `native_lock::Record`). `pub` for the same reason as
/// [`Identity`].
#[derive(Debug, Clone)]
pub struct Entry<T> {
    pub identity: Identity,
    pub payload: T,
}

/// The record merge, format-independent: for every name in the union of
/// `base`/`ours`/`theirs`, this is the plain three-way merge of a single
/// value (`Option<Identity>`, `None` meaning "absent/removed") applied
/// per-name — unchanged on both sides, changed on exactly one side
/// (including added or removed), changed on both sides identically, or
/// changed on both sides differently (divergent, returned by name so the
/// caller can render markers). A winning identity is never synthesised:
/// the record itself is reused from whichever of `ours`/`theirs`/`base`
/// (in that order) actually carries it. `pub`: a test builds `merged`/
/// `divergent` for [`escalate_resolve`] the same way `merge_composer_lock`
/// itself does, rather than duplicating this logic.
pub fn merge<T: Clone>(
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
    reason = "the three merge-driver paths plus --no-resolve/--as-of/--cache-dir/--offline/\
              --max-scope/--offline-rung"
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
    max_scope: Scope,
    offline_rung: bool,
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
            max_scope,
            offline_rung,
        ),
        // #314's rung 0 needs a divergent name's own `require`/`conflict`/
        // `replace`/`provide`, which a `viv.lock` record never carries (this
        // module's own doc comment on why); `--offline-rung` is a no-op here
        // rather than a refusal, the same tolerance `--max-scope` already
        // has for a flag that happens not to matter to this format.
        Format::VivLock => merge_viv_lock(
            base,
            ours,
            theirs,
            project_dir,
            no_resolve,
            as_of,
            cache_dir,
            offline,
            max_scope,
        ),
    }
}

/// `pub`: a test builds `ours`/`theirs` for [`escalate_resolve`] straight
/// off a fixture `composer.lock`'s own [`lock::read_lock`], the same way
/// `merge_composer_lock` itself does.
pub fn composer_entries(lock: &Lock) -> BTreeMap<String, Entry<Value>> {
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

/// The record merge and re-solve, purely on the three inputs' bytes: no
/// path but `project_dir`'s own `composer.json`/repositories touches disk
/// (#299). [`merge_composer_lock`] is this with a real `%O %A %B` triple
/// read off disk; `install`'s git-index-stage fallback (`merge_driver.rs`)
/// calls this directly with `git show :1:`/`:2:`/`:3:` output, which has no
/// path of its own to write a temporary file for. Returns the resulting
/// `composer.lock` text and the same 0/1 status [`run`] reports; the
/// caller decides where that text goes (`ours`, for the CLI driver; the
/// real `composer.lock`, for `install`'s fallback).
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors run's own --no-resolve/--as-of/--cache-dir/--offline/--max-scope/\
              --offline-rung, plus the three inputs"
)]
pub(crate) fn merge_composer_lock_bytes(
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
    project_dir: &Path,
    no_resolve: bool,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    offline: bool,
    max_scope: Scope,
    offline_rung: bool,
) -> Result<(String, u8)> {
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

    let base_lock = lock::parse_lock(&String::from_utf8_lossy(base), Path::new("base"))
        .context("reading base")?;
    let ours_lock = lock::parse_lock(&String::from_utf8_lossy(ours), Path::new("ours"))
        .context("reading ours")?;
    let theirs_lock = lock::parse_lock(&String::from_utf8_lossy(theirs), Path::new("theirs"))
        .context("reading theirs")?;

    let base_entries = composer_entries(&base_lock);
    let ours_entries = composer_entries(&ours_lock);
    let theirs_entries = composer_entries(&theirs_lock);

    let (merged, divergent) = merge(&base_entries, &ours_entries, &theirs_entries);

    if !divergent.is_empty() && !no_resolve {
        match try_resolve_composer_lock(
            &merged,
            &divergent,
            &ours_entries,
            &theirs_entries,
            &composer_json,
            project_dir,
            as_of,
            cache_dir,
            offline,
            max_scope,
            offline_rung,
        ) {
            Ok((text, rung, name, moved)) => {
                print_resolution(&divergent, rung, name, &moved);
                return Ok((text, 0));
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

    let ours_value: Value = serde_json::from_slice(ours).context("re-parsing ours as JSON")?;
    let aggregates = LockAggregates::from_lock_value(&ours_value);
    let dev_present = ours_value.get("packages-dev").is_some_and(|v| !v.is_null());
    let dev_opt = dev_present.then_some(dev.as_slice());

    let doc = lock_writer::write(&non_dev, dev_opt, &aggregates.as_options(), &composer_json)?;

    if divergent.is_empty() {
        return Ok((doc, 0));
    }

    let patched =
        inject_composer_conflicts(&doc, &merged, &divergent, &ours_entries, &theirs_entries)?;
    Ok((patched, 1))
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors run's own --no-resolve/--as-of/--cache-dir/--offline/--max-scope/\
              --offline-rung, plus the three paths"
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
    max_scope: Scope,
    offline_rung: bool,
) -> Result<u8> {
    let base_bytes = fs_err::read(base).with_context(|| format!("reading {}", base.display()))?;
    let ours_bytes = fs_err::read(ours).with_context(|| format!("reading {}", ours.display()))?;
    let theirs_bytes =
        fs_err::read(theirs).with_context(|| format!("reading {}", theirs.display()))?;
    let (text, status) = merge_composer_lock_bytes(
        &base_bytes,
        &ours_bytes,
        &theirs_bytes,
        project_dir,
        no_resolve,
        as_of,
        cache_dir,
        offline,
        max_scope,
        offline_rung,
    )?;
    fs_err::write(ours, text)?;
    Ok(status)
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

/// Rung 2's allow-list addition (#296): every pinned (non-divergent, not
/// already in `closure`) name whose own locked `require` names something in
/// `closure` — read straight off the record already in hand, never
/// re-fetched. `closure` itself is [`solver::pool_builder::expand_allow_list`]'s
/// own computation (`UpdateAllowMode::WithTransitiveDeps`) over
/// `divergent`, recomputed here rather than threaded out of rung 1's own
/// solve, since it is cheap (no network) and keeps this function free of a
/// second return value on [`resolve_divergent_closure`].
fn direct_dependents(
    merged: &BTreeMap<String, Entry<Value>>,
    root: &Value,
    divergent: &[String],
) -> BTreeSet<String> {
    let locked_requires: HashMap<String, Vec<String>> = merged
        .iter()
        .map(|(name, entry)| {
            let requires = entry
                .payload
                .get("require")
                .and_then(Value::as_object)
                .map(|m| m.keys().map(|k| k.to_ascii_lowercase()).collect())
                .unwrap_or_default();
            (name.to_ascii_lowercase(), requires)
        })
        .collect();
    let root_require_names: HashSet<String> = root
        .get("require")
        .and_then(Value::as_object)
        .into_iter()
        .flat_map(|m| m.keys())
        .chain(
            root.get("require-dev")
                .and_then(Value::as_object)
                .into_iter()
                .flat_map(|m| m.keys()),
        )
        .map(|k| k.to_ascii_lowercase())
        .collect();
    let allow_list: Vec<String> = divergent.iter().map(|n| n.to_ascii_lowercase()).collect();
    let closure = solver::pool_builder::expand_allow_list(
        &allow_list,
        &locked_requires,
        &root_require_names,
        UpdateAllowMode::WithTransitiveDeps,
    );

    merged
        .iter()
        .filter(|(name, _)| !closure.contains(&name.to_ascii_lowercase()))
        .filter(|(_, entry)| {
            entry
                .payload
                .get("require")
                .and_then(Value::as_object)
                .is_some_and(|requires| {
                    requires
                        .keys()
                        .any(|k| closure.contains(&k.to_ascii_lowercase()))
                })
        })
        .map(|(name, _)| name.clone())
        .collect()
}

/// Every package outside `divergent` whose resolved identity in `result`
/// differs from both `ours` and `theirs` (#296's "say what moved"): a
/// plain rung-1 resolution never produces one of these, since nothing
/// outside the divergent names is ever unlocked at that rung.
fn moved_packages(
    result: &solver::UpdateResult,
    divergent: &BTreeSet<String>,
    ours: &BTreeMap<String, Entry<Value>>,
    theirs: &BTreeMap<String, Entry<Value>>,
) -> Vec<Moved> {
    let mut moved = Vec::new();
    for resolved in result.non_dev.iter().chain(result.dev.iter()) {
        if divergent.contains(&resolved.name) {
            continue;
        }
        let o = ours
            .get(&resolved.name)
            .map(|e| e.identity.version.as_str());
        let t = theirs
            .get(&resolved.name)
            .map(|e| e.identity.version.as_str());
        if o == Some(resolved.pretty_version.as_str())
            || t == Some(resolved.pretty_version.as_str())
        {
            continue;
        }
        let before = match (o, t) {
            (Some(a), Some(b)) if a == b => a.to_string(),
            (Some(a), Some(b)) => format!("ours {a}, theirs {b}"),
            (Some(a), None) => a.to_string(),
            (None, Some(b)) => b.to_string(),
            (None, None) => "new".to_string(),
        };
        moved.push(Moved {
            name: resolved.name.clone(),
            before,
            after: resolved.pretty_version.clone(),
        });
    }
    moved.sort_by(|a, b| a.name.cmp(&b.name));
    moved
}

/// One escalation's outcome: the resolved lock, the rung that finished it,
/// and every package `moved_packages` found outside the divergent set.
pub struct EscalatedResolve {
    pub result: solver::UpdateResult,
    pub scope: Scope,
    pub moved: Vec<Moved>,
}

/// #296: rung 1 ([`resolve_divergent_closure`]), then rung 2 (rung 1's
/// allow list plus `direct_dependents`), then rung 3 (a full solve,
/// [`solver::solve_update_seeded`], with every non-divergent locked
/// version as `preferred` — never a plain update, since `preferred` is
/// non-empty and every locked name is in it). Stops and returns the
/// failure as soon as `max_scope` caps it short of finishing; the caller
/// turns that into markers, same as any other resolve failure.
/// `pub`, generic over [`Transport`] like [`resolve_divergent_closure`]:
/// the CLI path always builds a real `HttpTransport`, a test can drive
/// this directly against a fixture-backed one.
#[expect(
    clippy::too_many_arguments,
    reason = "the merge's own pinned/ours/theirs maps, the resolve inputs (root/prefer_stable/\
              as_of/cache_dir), and the --max-scope cap"
)]
pub async fn escalate_resolve<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    merged: &BTreeMap<String, Entry<Value>>,
    divergent: &BTreeSet<String>,
    ours: &BTreeMap<String, Entry<Value>>,
    theirs: &BTreeMap<String, Entry<Value>>,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    max_scope: Scope,
) -> Result<EscalatedResolve> {
    let locked_by_name = locked_by_name_from_merge(merged);
    let divergent_list: Vec<String> = divergent.iter().cloned().collect();

    let (result, scope) = match resolve_divergent_closure(
        repo,
        root,
        prefer_stable,
        &locked_by_name,
        &divergent_list,
        as_of,
    )
    .await
    {
        Ok(result) => (result, Scope::Closure),
        Err(rung1_err) if max_scope == Scope::Closure => {
            return Err(
                rung1_err.context(format!("capped at --max-scope={}", Scope::Closure.name()))
            );
        }
        Err(_rung1_err) => {
            let dependents = direct_dependents(merged, root, &divergent_list);
            let mut allow_list = divergent_list.clone();
            allow_list.extend(dependents);
            match resolve_divergent_closure(
                repo,
                root,
                prefer_stable,
                &locked_by_name,
                &allow_list,
                as_of,
            )
            .await
            {
                Ok(result) => (result, Scope::Dependents),
                Err(rung2_err) if max_scope == Scope::Dependents => {
                    return Err(rung2_err.context(format!(
                        "capped at --max-scope={}",
                        Scope::Dependents.name()
                    )));
                }
                Err(_) => {
                    let preferred = update::preferred_versions(&locked_by_name, &[])?;
                    let seed: Vec<String> = locked_by_name.keys().cloned().collect();
                    match solver::solve_update_seeded::<T, crate::audit::NoAdvisories>(
                        repo,
                        root,
                        prefer_stable,
                        false,
                        &seed,
                        preferred,
                        None,
                        cache_dir,
                        &crate::autoload::platform::IgnorePlatform::None,
                    )
                    .await
                    {
                        Ok(result) => (result, Scope::Seeded),
                        Err(err) => {
                            let reason = if err
                                .downcast_ref::<solver::problem::SolverError>()
                                .is_some()
                            {
                                "the merged composer.json is unsatisfiable even at rung 3 \
                                 (seeded): this is a manifest problem, not a lock one"
                            } else {
                                "re-solving at rung 3 (seeded) did not finish; the registry may \
                                 be unreachable (cached metadata was tried first via \
                                 --cache-dir)"
                            };
                            return Err(err.context(reason));
                        }
                    }
                }
            }
        }
    };

    let moved = moved_packages(&result, divergent, ours, theirs);
    Ok(EscalatedResolve {
        result,
        scope,
        moved,
    })
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
/// `build_repository`, `default_cache_dir`), then [`escalate_resolve`] over
/// it. Any failure here — no network, a package no longer resolvable, a
/// genuine constraint conflict in the merged manifest even at
/// `--max-scope`'s cap — is the caller's cue to fall back to conflict
/// markers, so this never itself decides that markers are fine; it only
/// reports why the resolve didn't happen.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors merge_composer_lock's own --as-of/--cache-dir/--offline/--max-scope/\
              --offline-rung, plus the ours/theirs maps escalate_resolve's moved-package report \
              needs"
)]
fn try_resolve_composer_lock(
    merged: &BTreeMap<String, Entry<Value>>,
    divergent: &BTreeSet<String>,
    ours: &BTreeMap<String, Entry<Value>>,
    theirs: &BTreeMap<String, Entry<Value>>,
    composer_json: &[u8],
    project_dir: &Path,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    offline: bool,
    max_scope: Scope,
    offline_rung: bool,
) -> Result<(String, u8, &'static str, Vec<Moved>)> {
    let root: Value = serde_json::from_slice(composer_json).context("parsing composer.json")?;
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
        let repo = update::build_repository(project_dir, &root, &cache_dir, &fetcher, metadata_ttl)
            .await?;
        let resolved = escalate_then_offline_pin(
            &repo,
            &root,
            prefer_stable,
            merged,
            divergent,
            ours,
            theirs,
            as_of,
            Some(&cache_dir),
            max_scope,
            offline_rung,
        )
        .await;
        update::forget_repo(repo);
        let (result, rung, name, moved) = resolved?;
        let text = write_resolved_lock(&result, composer_json)?;
        Ok((text, rung, name, moved))
    })
}

/// #296's "say what moved": the rung reached and the divergent names it
/// resolved, then one line per [`Moved`] package. A rung-1 resolution
/// prints nothing beyond the first line, matching `docs/research.md`'s own
/// wording. `rung`/`name` rather than a [`Scope`] directly: rung 0
/// (`--offline-rung`, #314) never escalates and so has no `Scope` variant
/// of its own, and every other caller already has a `Scope` in hand to
/// pass as `scope.rung()`/`scope.name()`.
fn print_resolution(divergent: &BTreeSet<String>, rung: u8, name: &str, moved: &[Moved]) {
    let names: Vec<&str> = divergent.iter().map(String::as_str).collect();
    warn_out(&format!(
        "viv lock merge: resolved via rung {rung} ({name}): {}",
        names.join(", ")
    ));
    for m in moved {
        warn_out(&format!(
            "viv lock merge: {} moved outside the divergent set: {} -> {}",
            m.name, m.before, m.after
        ));
    }
}

/// The offline pin check (#314), `--offline-rung`: tried only once the
/// registry escalation (rungs 1-3) has already failed
/// ([`escalate_then_offline_pin`] is the caller that enforces this
/// ordering). For every divergent name, one parent's own pinned record —
/// `ours` first, then `theirs` — stands in for a fresh fetch, and the
/// whole candidate set is checked with [`solver::solve_partial_update`]'s
/// own empty-allow-list path: [`solver::pool_builder::build_partial_seeded`]
/// loads every locked-out name — which, with nothing left in the allow
/// list, is now everyone — straight from its own lock entry
/// (`package_from_lock_entry`, `install.rs`'s #300 check reuses the same
/// primitive for the same reason), so the real rule generator and CDCL
/// solver run over a pool built without a single fetch. This is that
/// machinery, not a second constraint checker: a candidate that conflicts
/// with a sibling's `require`, or that the merged `composer.json`'s own
/// `require`/`require-dev`/`conflict` rules out, fails the solve the same
/// way a live re-solve would.
///
/// Whichever side is tried, every divergent name takes that side's own
/// pin in one attempt — never a per-name mix of `ours` and `theirs` — so
/// there are at most two attempts regardless of how many names diverged,
/// matching how rungs 1-3 already treat the divergent set as one unit
/// rather than escalating name by name. `None` when a name has no entry
/// on the side being tried (removed on that side) or neither attempt
/// solves; the caller reads that as "the conflict stands, fall back to
/// markers".
///
/// `pub`, generic over [`Transport`] like [`escalate_resolve`]: reuses
/// whichever `repo` the failed escalation already built (never a fresh
/// one of its own) — the empty allow list is what keeps this fetch-free,
/// not the transport underneath it, so a test can drive this against a
/// real fixture-backed repo that just finished failing rungs 1-3 and
/// prove, by counting calls, that this step adds none.
pub async fn try_offline_rung<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    merged: &BTreeMap<String, Entry<Value>>,
    divergent: &BTreeSet<String>,
    ours: &BTreeMap<String, Entry<Value>>,
    theirs: &BTreeMap<String, Entry<Value>>,
) -> Option<solver::UpdateResult> {
    for (primary, fallback) in [(ours, theirs), (theirs, ours)] {
        let mut locked_by_name = locked_by_name_from_merge(merged);
        let mut complete = true;
        for name in divergent {
            let Some(entry) = primary.get(name).or_else(|| fallback.get(name)) else {
                complete = false;
                break;
            };
            locked_by_name.insert(name.clone(), entry.payload.clone());
        }
        if !complete {
            continue;
        }
        if let Ok(result) = solver::solve_partial_update(
            repo,
            root,
            prefer_stable,
            false,
            &locked_by_name,
            &[],
            UpdateAllowMode::OnlyListed,
        )
        .await
        {
            return Some(result);
        }
    }
    None
}

/// The offline pin's own rung tag (#314): never a [`Scope`] variant,
/// since it never escalates further itself and only ever runs once every
/// `Scope` rung `max_scope` allowed has already failed — one further rung
/// past the highest of the three, not a fourth choice among them.
const OFFLINE_PIN_RUNG: u8 = 4;
const OFFLINE_PIN_NAME: &str = "offline_pin";

/// Escalate first ([`escalate_resolve`]); only on failure, and only when
/// `offline_rung` asks for it, try the offline pin
/// ([`try_offline_rung`]) as a last resort before conflict markers.
/// Reusing the very `repo` the failed escalation just finished with,
/// rather than building a second one: [`try_offline_rung`]'s own
/// empty-allow-list call is what keeps it fetch-free (`build_partial_seeded`'s
/// "everyone locked out" path), not which `Transport` sits underneath, so
/// there is nothing a second `Repository` would buy here.
///
/// This ordering is why the offline pin can never disagree with a
/// registry answer that also finished: by the time it runs, this same
/// `repo` has already tried and failed to produce one at every rung
/// `max_scope` allowed. `docs/research.md`'s own measurement is what
/// found the *previous* order (offline first) disagreed with the registry
/// on two-thirds of the merges where both could run at all; moving it
/// here is that finding acted on, not merely recorded.
///
/// `pub`, generic over [`Transport`]: `tests/lock_merge.rs` drives this
/// with a fixture-backed repo whose escalation genuinely fails (a
/// requirement the fixture's own registry data cannot satisfy at any
/// rung), then a call-counting wrapper proves the offline branch adds no
/// further fetch, and a separate case proves this never even reaches the
/// offline branch when escalation succeeds.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors escalate_resolve's own arguments plus the offline_rung switch"
)]
pub async fn escalate_then_offline_pin<T: Transport>(
    repo: &Repository<T>,
    root: &Value,
    prefer_stable: bool,
    merged: &BTreeMap<String, Entry<Value>>,
    divergent: &BTreeSet<String>,
    ours: &BTreeMap<String, Entry<Value>>,
    theirs: &BTreeMap<String, Entry<Value>>,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    max_scope: Scope,
    offline_rung: bool,
) -> Result<(solver::UpdateResult, u8, &'static str, Vec<Moved>)> {
    match escalate_resolve(
        repo,
        root,
        prefer_stable,
        merged,
        divergent,
        ours,
        theirs,
        as_of,
        cache_dir,
        max_scope,
    )
    .await
    {
        Ok(resolved) => Ok((
            resolved.result,
            resolved.scope.rung(),
            resolved.scope.name(),
            resolved.moved,
        )),
        Err(escalate_err) => {
            if !offline_rung {
                return Err(escalate_err);
            }
            match try_offline_rung(repo, root, prefer_stable, merged, divergent, ours, theirs).await
            {
                Some(result) => {
                    let moved = moved_packages(&result, divergent, ours, theirs);
                    Ok((result, OFFLINE_PIN_RUNG, OFFLINE_PIN_NAME, moved))
                }
                None => Err(escalate_err),
            }
        }
    }
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

/// #295: `viv.lock`'s non-divergent pinned set has no `require` of its own
/// (see the module doc), so its resolve payload — everything
/// [`solver::pool_builder::build_partial`] needs to pin a name hard — comes
/// from the sibling `composer.lock`'s own entry for that name instead,
/// matched by name and never re-fetched. `pub`: a test builds `merged`'s
/// identities directly and checks this against a hand-built `composer.lock`
/// `Value`, the same way [`composer_entries`] is exposed for
/// [`escalate_resolve`]'s own tests. A divergent name is deliberately never
/// looked up here: rung 1's own allow list re-solves it fresh regardless of
/// what either branch had locked, so the only names that need a payload are
/// the ones staying pinned.
pub fn viv_lock_payloads(
    merged: &BTreeMap<String, Identity>,
    composer_lock: &Value,
) -> Result<BTreeMap<String, Entry<Value>>> {
    let payloads = update::locked_packages_by_name(composer_lock);
    merged
        .iter()
        .map(|(name, identity)| {
            let payload = payloads
                .get(&name.to_ascii_lowercase())
                .cloned()
                .with_context(|| {
                    format!(
                        "composer.lock has no entry for {name}, needed to re-solve viv.lock's \
                         pinned set"
                    )
                })?;
            Ok((
                name.clone(),
                Entry {
                    identity: identity.clone(),
                    payload,
                },
            ))
        })
        .collect()
}

/// `ours`/`theirs` only ever feed [`moved_packages`]' identity comparison
/// inside [`escalate_resolve`], never a payload, so a `viv.lock` record's
/// own (unused) fields stand in as `Value::Null` rather than sourcing a
/// second payload these two maps never read.
fn to_value_entries(
    entries: &BTreeMap<String, Entry<native_lock::Record>>,
) -> BTreeMap<String, Entry<Value>> {
    entries
        .iter()
        .map(|(name, entry)| {
            (
                name.clone(),
                Entry {
                    identity: entry.identity.clone(),
                    payload: Value::Null,
                },
            )
        })
        .collect()
}

/// The `viv.lock` counterpart to [`try_resolve_composer_lock`]: assembles
/// [`escalate_resolve`]'s `Value` payloads via [`viv_lock_payloads`], then
/// writes back both `viv.lock` (via [`native_lock::write`]) and the sibling
/// `composer.lock` (via [`write_resolved_lock`], the same writer the
/// composer.lock path itself uses) so the pair stays in step for #297's own
/// `reconcile`.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors try_resolve_composer_lock's own arguments, plus the composer.lock sibling \
              path this format's re-solve reads its pinned set's requires from"
)]
fn try_resolve_viv_lock(
    merged: &BTreeMap<String, Entry<native_lock::Record>>,
    divergent: &BTreeSet<String>,
    ours: &BTreeMap<String, Entry<native_lock::Record>>,
    theirs: &BTreeMap<String, Entry<native_lock::Record>>,
    project_dir: &Path,
    composer_lock_path: &Path,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    offline: bool,
    max_scope: Scope,
) -> Result<(String, String, Scope, Vec<Moved>)> {
    let composer_lock: Value = serde_json::from_slice(&fs_err::read(composer_lock_path)?)
        .with_context(|| format!("parsing {}", composer_lock_path.display()))?;
    let merged_identities: BTreeMap<String, Identity> = merged
        .iter()
        .map(|(name, entry)| (name.clone(), entry.identity.clone()))
        .collect();
    let merged_value = viv_lock_payloads(&merged_identities, &composer_lock)?;
    let ours_value = to_value_entries(ours);
    let theirs_value = to_value_entries(theirs);

    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path)
        .with_context(|| format!("reading {}", composer_json_path.display()))?;
    let root: Value = serde_json::from_slice(&composer_json).context("parsing composer.json")?;
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
    let resolved = runtime.block_on(async {
        let fetcher = update::build_fetcher(project_dir, &root, offline)?;
        let metadata_ttl = update::metadata_ttl(None, offline);
        let repo = update::build_repository(project_dir, &root, &cache_dir, &fetcher, metadata_ttl)
            .await?;
        let resolved = escalate_resolve(
            &repo,
            &root,
            prefer_stable,
            &merged_value,
            divergent,
            &ours_value,
            &theirs_value,
            as_of,
            Some(&cache_dir),
            max_scope,
        )
        .await;
        update::forget_repo(repo);
        resolved
    })?;

    let viv_text = native_lock::write(&resolved.result.non_dev, &resolved.result.dev, &root)?;
    let composer_text = write_resolved_lock(&resolved.result, &composer_json)?;
    Ok((viv_text, composer_text, resolved.scope, resolved.moved))
}

/// [`merge_composer_lock_bytes`]'s `viv.lock` counterpart (#299): the
/// three inputs' bytes in, `(viv.lock text, composer.lock text when the
/// re-solve also rewrote the sibling, status)` out, nothing touched on
/// disk but `project_dir`'s own `composer.json` and (read-only, via
/// [`try_resolve_viv_lock`]) sibling `composer.lock`.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors merge_composer_lock_bytes's own --as-of/--cache-dir/--offline/--max-scope, \
              plus the three inputs"
)]
pub(crate) fn merge_viv_lock_bytes(
    base: &[u8],
    ours: &[u8],
    theirs: &[u8],
    project_dir: &Path,
    no_resolve: bool,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    offline: bool,
    max_scope: Scope,
) -> Result<(String, Option<String>, u8)> {
    let base_entries = viv_entries(native_lock::parse(&String::from_utf8_lossy(base))?);
    let ours_entries = viv_entries(native_lock::parse(&String::from_utf8_lossy(ours))?);
    let theirs_entries = viv_entries(native_lock::parse(&String::from_utf8_lossy(theirs))?);

    let (merged, divergent) = merge(&base_entries, &ours_entries, &theirs_entries);

    if divergent.is_empty() {
        let records: Vec<native_lock::Record> =
            merged.into_values().map(|entry| entry.payload).collect();
        return Ok((native_lock::write_records(&records)?, None, 0));
    }

    if !no_resolve {
        let composer_lock_path = project_dir.join("composer.lock");
        if composer_lock_path.exists() {
            match try_resolve_viv_lock(
                &merged,
                &divergent,
                &ours_entries,
                &theirs_entries,
                project_dir,
                &composer_lock_path,
                as_of,
                cache_dir,
                offline,
                max_scope,
            ) {
                Ok((viv_text, composer_text, scope, moved)) => {
                    warn_out(&format!(
                        "viv lock merge: also re-wrote {} so it stays a companion to viv.lock",
                        composer_lock_path.display()
                    ));
                    print_resolution(&divergent, scope.rung(), scope.name(), &moved);
                    return Ok((viv_text, Some(composer_text), 0));
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
        } else {
            warn_out(&format!(
                "viv lock merge: no sibling {} to read the pinned set's requires from; falling \
                 back to conflict markers",
                composer_lock_path.display()
            ));
        }
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
    Ok((text, None, 1))
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors merge_composer_lock's own --as-of/--cache-dir/--offline/--max-scope, plus \
              the three merge-driver paths"
)]
fn merge_viv_lock(
    base: &Path,
    ours: &Path,
    theirs: &Path,
    project_dir: &Path,
    no_resolve: bool,
    as_of: Option<i64>,
    cache_dir: Option<&Path>,
    offline: bool,
    max_scope: Scope,
) -> Result<u8> {
    let base_bytes = fs_err::read(base).with_context(|| format!("reading {}", base.display()))?;
    let ours_bytes = fs_err::read(ours).with_context(|| format!("reading {}", ours.display()))?;
    let theirs_bytes =
        fs_err::read(theirs).with_context(|| format!("reading {}", theirs.display()))?;
    let (viv_text, composer_text, status) = merge_viv_lock_bytes(
        &base_bytes,
        &ours_bytes,
        &theirs_bytes,
        project_dir,
        no_resolve,
        as_of,
        cache_dir,
        offline,
        max_scope,
    )?;
    fs_err::write(ours, viv_text)?;
    if let Some(composer_text) = composer_text {
        fs_err::write(project_dir.join("composer.lock"), composer_text)?;
    }
    Ok(status)
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
