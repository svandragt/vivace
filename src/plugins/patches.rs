//! `cweagans/composer-patches` 2.0.0 (docs/plugin-strategy.md's rule 3, #53):
//! apply patches from root `extra.patches` and the `patches-file` (default
//! `patches.json`) to the packages they target, and write `patches.lock.json`
//! the same way the real plugin does.
//!
//! 2.0.0 dropped the plain `patch` CLI patcher 1.x had: its only patchers are
//! `GitPatcher` (needs an existing `.git` in the install dir — never true for
//! a store-linked package) and `GitInitPatcher` (creates one with `git init`,
//! applies with `git apply --check` then `git apply`, then removes `.git`
//! again). `FreeformPatcher` needs a per-patch `extra.freeform` the fixture
//! never sets, so only the git path is ported here. The `Dependencies`
//! resolver (patches declared by a required package's own `composer.json`)
//! isn't ported either — out of scope for #53, which only asks for the root
//! and patches-file sources.
//!
//! Store files are read-only hardlinks shared with every other project using
//! the same package, so a patched package is relinked with
//! [`LinkMode::Copy`] first — the same escape hatch `--link-mode copy`
//! already offers, just scoped to the one package being patched.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::link::{LinkMode, link_tree};
use crate::lock::{Package, Root};
use crate::store::{Store, hex};

use super::{Adapter, Ctx};

pub(super) struct Patches;

impl Adapter for Patches {
    fn plugin_names(&self) -> &'static [&'static str] {
        &["cweagans/composer-patches"]
    }

    fn upstream_version(&self) -> &'static str {
        "2.0.0"
    }

    fn state_fingerprint(&self, root: &Root, project_dir: &Path) -> Result<Option<String>> {
        fingerprint(root, project_dir).map(Some)
    }

    fn post_link(
        &self,
        ctx: &Ctx<'_>,
        newly_linked: &[Package],
        kept: &[&Package],
        store: &Store,
    ) -> Result<()> {
        apply(
            ctx.root,
            ctx.project_dir,
            ctx.vendor_dir,
            newly_linked,
            kept,
            store,
        )
    }
}

/// A patch, in the plugin's own `Patch::jsonSerialize` field order — matters
/// for the byte-identical `patches.lock.json` the e2e test diffs against.
#[derive(Debug, Clone, Serialize)]
struct Patch {
    package: String,
    description: String,
    url: String,
    sha256: Option<String>,
    depth: Option<i64>,
    extra: Value,
}

/// A package name paired with the patches resolved for it, in resolution order.
type Group<T> = Vec<(String, Vec<T>)>;

/// `extra.composer-patches`' own config keys the plugin's `ConfigurablePlugin`
/// trait reads; `disable-*`/`ignore-dependency-patches` are irrelevant here
/// since resolvers/patchers/downloaders aren't pluggable in vivace.
struct Config {
    patches_file: String,
    package_depths: HashMap<String, i64>,
    default_patch_depth: i64,
}

fn config(root: &Root) -> Config {
    let cfg = root.extra.get("composer-patches");
    let patches_file = cfg
        .and_then(|c| c.get("patches-file"))
        .and_then(Value::as_str)
        .unwrap_or("patches.json")
        .to_string();
    let package_depths = cfg
        .and_then(|c| c.get("package-depths"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_i64().map(|d| (k.clone(), d)))
                .collect()
        })
        .unwrap_or_default();
    let default_patch_depth = cfg
        .and_then(|c| c.get("default-patch-depth"))
        .and_then(Value::as_i64)
        .unwrap_or(1);
    Config {
        patches_file,
        package_depths,
        default_patch_depth,
    }
}

/// `Util::getDefaultPackagePatchDepth`: the plugin's own hardcoded per-package
/// depth override, ported verbatim (only one entry as of 2.0.0).
fn built_in_depth(package: &str) -> Option<i64> {
    (package == "drupal/core").then_some(2)
}

/// `ResolverBase::findPatchesInJson`: a package's patches are either the
/// expanded format (a JSON array of `{description,url,sha256,depth,extra}`
/// objects) or the compact format (a JSON object of `description -> url`).
fn parse_group(defs: &Map<String, Value>, provenance: &str) -> Group<Patch> {
    let mut out = Vec::new();
    for (package, value) in defs {
        let mut patches = Vec::new();
        match value {
            Value::Array(items) => {
                for item in items {
                    let obj = item.as_object();
                    let description = obj
                        .and_then(|o| o.get("description"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let url = obj
                        .and_then(|o| o.get("url"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let sha256 = obj
                        .and_then(|o| o.get("sha256"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let depth = obj.and_then(|o| o.get("depth")).and_then(Value::as_i64);
                    let mut extra = obj
                        .and_then(|o| o.get("extra"))
                        .cloned()
                        .unwrap_or_else(|| Value::Object(Map::new()));
                    if let Some(map) = extra.as_object_mut() {
                        map.insert(
                            "provenance".to_string(),
                            Value::String(provenance.to_string()),
                        );
                    }
                    patches.push(Patch {
                        package: package.clone(),
                        description,
                        url,
                        sha256,
                        depth,
                        extra,
                    });
                }
            }
            Value::Object(map) => {
                for (description, url) in map {
                    let mut extra = Map::new();
                    extra.insert(
                        "provenance".to_string(),
                        Value::String(provenance.to_string()),
                    );
                    patches.push(Patch {
                        package: package.clone(),
                        description: description.clone(),
                        url: url.as_str().unwrap_or_default().to_string(),
                        sha256: None,
                        depth: None,
                        extra: Value::Object(extra),
                    });
                }
            }
            _ => {}
        }
        out.push((package.clone(), patches));
    }
    out
}

/// `PatchCollection::addPatch`: the first patch added for a (url, sha256)
/// pair wins; a later one is silently dropped, scoped to patches already
/// collected for the same package.
fn merge(into: &mut Group<Patch>, group: Group<Patch>) {
    for (package, new_patches) in group {
        let index = into
            .iter()
            .position(|(name, _)| *name == package)
            .unwrap_or_else(|| {
                into.push((package, Vec::new()));
                into.len() - 1
            });
        let existing = &mut into[index].1;
        for patch in new_patches {
            let duplicate = existing.iter().any(|p| {
                p.url == patch.url || (patch.sha256.is_some() && p.sha256 == patch.sha256)
            });
            if !duplicate {
                existing.push(patch);
            }
        }
    }
}

/// `Resolver::loadFromResolvers`, restricted to the `RootComposer` and
/// `PatchesFile` resolvers (see the module doc for why `Dependencies` isn't
/// ported).
fn resolve(root: &Root, project_dir: &Path) -> Result<(Config, Group<Patch>)> {
    let cfg = config(root);
    let mut collection = Vec::new();

    if let Some(defs) = root.extra.get("patches").and_then(Value::as_object) {
        merge(&mut collection, parse_group(defs, "root"));
    }

    let patches_file_path = project_dir.join(&cfg.patches_file);
    if patches_file_path.is_file() {
        let bytes = fs_err::read(&patches_file_path)?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("{}: invalid JSON", patches_file_path.display()))?;
        let defs = value
            .get("patches")
            .and_then(Value::as_object)
            .with_context(|| format!("{}: no patches found", patches_file_path.display()))?;
        let provenance = format!("patches-file:{}", cfg.patches_file);
        merge(&mut collection, parse_group(defs, &provenance));
    }

    Ok((cfg, collection))
}

/// A cheap fingerprint of the currently-resolved patch definitions (root
/// `extra.patches` plus the patches file), for `install::State`'s no-op fast
/// path (`src/install.rs`): unlike `write_lock`'s `_hash`, this never reads a
/// patch's own bytes (no local file I/O or network fetch per patch beyond
/// the patches file itself), so it's cheap enough to compute on every run.
pub(super) fn fingerprint(root: &Root, project_dir: &Path) -> Result<String> {
    let (_, collection) = resolve(root, project_dir)?;
    let mut patches_by_package = Map::new();
    for (package, patches) in &collection {
        let values: Vec<Value> = patches
            .iter()
            .map(serde_json::to_value)
            .collect::<serde_json::Result<_>>()?;
        patches_by_package.insert(package.clone(), Value::Array(values));
    }
    let compact = serde_json::to_string(&Value::Object(patches_by_package))?;
    Ok(hex(Sha256::digest(compact.as_bytes())))
}

/// A patch with its depth resolved and its content already read, ready to
/// hand to `git apply`.
struct Ready {
    meta: Patch,
    bytes: Vec<u8>,
}

/// `Patches::guessDepth`, `Downloader::downloadPatch`'s sha256 verification
/// and, implicitly, its actual download: reads every patch's bytes (local or
/// remote), fills in `sha256` if the definition didn't set one (or checks it
/// matches, `HashMismatchException`'s equivalent), and resolves `depth`.
fn ready_patches(
    project_dir: &Path,
    cfg: &Config,
    collection: Group<Patch>,
) -> Result<Group<Ready>> {
    let mut ready = Vec::new();
    for (package, patches) in collection {
        let mut list = Vec::new();
        for mut patch in patches {
            let bytes = read_patch(project_dir, &patch.url)
                .with_context(|| format!("{}: downloading patch {}", patch.package, patch.url))?;
            let digest = hex(Sha256::digest(&bytes));
            match &patch.sha256 {
                Some(expected) if *expected != digest => bail!(
                    "{}: sha256 mismatch for patch {} (expected {expected}, got {digest})",
                    patch.package,
                    patch.url
                ),
                _ => patch.sha256 = Some(digest),
            }
            patch.depth = Some(
                patch
                    .depth
                    .or_else(|| cfg.package_depths.get(&package).copied())
                    .or_else(|| built_in_depth(&package))
                    .unwrap_or(cfg.default_patch_depth),
            );
            list.push(Ready { meta: patch, bytes });
        }
        ready.push((package, list));
    }
    Ok(ready)
}

/// A patch's `url`: a project-root-relative local path (the fixture, and
/// Composer's own recommendation), or a URL fetched over HTTP(S). Vivace's
/// own `fetch::Fetcher` is shaped around a package's dist, not an arbitrary
/// URL, so a patch download gets its own small client here instead.
fn read_patch(project_dir: &Path, url: &str) -> Result<Vec<u8>> {
    if url.contains("://") {
        let runtime = tokio::runtime::Runtime::new()?;
        runtime.block_on(async {
            let response = reqwest::get(url).await?.error_for_status()?;
            Ok(response.bytes().await?.to_vec())
        })
    } else {
        fs_err::read(project_dir.join(url)).map_err(Into::into)
    }
}

/// `Locker::getCollectionHash`/`setLockData`: writes `patches.lock.json` and
/// returns its `_hash`. The hash is `sha256(json_encode(['patches' => ...],
/// 0))` — PHP's default `json_encode` escapes `/` and non-ASCII, which is why
/// the hash input isn't just `serde_json::to_string`; the file itself is
/// written with Composer's `JsonFile::write` defaults instead (4-space
/// indent, unescaped `/` and Unicode), which the real plugin's own bytes
/// (captured while writing the fixture) confirm is what's on disk.
fn write_lock(lock_path: &Path, patches_by_package: &Map<String, Value>) -> Result<String> {
    let mut hash_root = Map::new();
    hash_root.insert(
        "patches".to_string(),
        Value::Object(patches_by_package.clone()),
    );
    let compact = serde_json::to_string(&Value::Object(hash_root))?.replace('/', "\\/");
    let hash = hex(Sha256::digest(compact.as_bytes()));

    let mut file_root = Map::new();
    file_root.insert("_hash".to_string(), Value::String(hash.clone()));
    file_root.insert(
        "patches".to_string(),
        Value::Object(patches_by_package.clone()),
    );

    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&Value::Object(file_root), &mut ser)?;
    buf.push(b'\n');
    fs_err::write(lock_path, &buf)?;
    Ok(hash)
}

fn previous_hash(lock_path: &Path) -> Option<String> {
    let bytes = fs_err::read(lock_path).ok()?;
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    value.get("_hash")?.as_str().map(str::to_string)
}

/// `GitPatcher`/`GitInitPatcher::apply`: a store-linked package is never a
/// git repo, so this always takes the `GitInitPatcher` branch — `git init`,
/// dry-run `git apply --check`, then the real `git apply`, then remove the
/// temporary `.git` regardless of outcome. `FreeformPatcher` isn't ported
/// (see the module doc), so a check or apply failure is `Patcher::applyPatch`
/// returning `false` for every patcher, i.e. the plugin's own exception.
fn apply_git(install_path: &Path, patch: &Ready) -> Result<()> {
    if Command::new("git").arg("--version").output().is_err() {
        bail!("No patchers available.");
    }
    let depth = patch.meta.depth.unwrap_or(1);
    let temp = tempfile::NamedTempFile::new()?;
    fs_err::write(temp.path(), &patch.bytes)?;

    let init = Command::new("git")
        .arg("-C")
        .arg(install_path)
        .arg("init")
        .output()
        .context("running git init")?;
    let mut applied = init.status.success();

    if applied {
        let check = Command::new("git")
            .arg("-C")
            .arg(install_path)
            .args(["apply", "--check", "--verbose", &format!("-p{depth}")])
            .arg(temp.path())
            .output()
            .context("running git apply --check")?;
        // "Git will indicate success but silently skip patches in some
        // scenarios" (cweagans/composer-patches#165) — same stderr sniff.
        applied = check.status.success()
            && !String::from_utf8_lossy(&check.stderr).starts_with("Skipped");
    }

    if applied {
        let apply = Command::new("git")
            .arg("-C")
            .arg(install_path)
            .args(["apply", &format!("-p{depth}"), "--verbose"])
            .arg(temp.path())
            .output()
            .context("running git apply")?;
        applied = apply.status.success();
    }

    // GitInitPatcher removes its temporary repo whether or not the patch
    // applied; best-effort, since a failure here shouldn't hide the real
    // error below.
    let _ = fs_err::remove_dir_all(install_path.join(".git"));

    if !applied {
        bail!(
            "No available patcher was able to apply patch {} to {}",
            patch.meta.url,
            patch.meta.package
        );
    }
    Ok(())
}

/// The full `cweagans/composer-patches` cycle for one `viv install`: resolve
/// patches, write `patches.lock.json`, and (re)patch every package whose
/// patch set is new to it — either just linked this run, or one whose
/// resolved patches changed since `patches.lock.json` was last written (the
/// lock changes with `composer.json`/the patches file, not with
/// `composer.lock`, so the ordinary plan diff can't see it).
pub(super) fn apply(
    root: &Root,
    project_dir: &Path,
    vendor_dir: &Path,
    newly_linked: &[Package],
    kept: &[&Package],
    store: &Store,
) -> Result<()> {
    let (cfg, collection) = resolve(root, project_dir)?;
    if collection.is_empty() {
        return Ok(());
    }
    let lock_path = project_dir.join("patches.lock.json");
    let previous = previous_hash(&lock_path);

    let ready = ready_patches(project_dir, &cfg, collection)?;

    let mut patches_by_package = Map::new();
    for (package, patches) in &ready {
        let values: Vec<Value> = patches
            .iter()
            .map(|p| serde_json::to_value(&p.meta))
            .collect::<serde_json::Result<_>>()?;
        patches_by_package.insert(package.clone(), Value::Array(values));
    }
    let new_hash = write_lock(&lock_path, &patches_by_package)?;
    let patch_set_changed = previous.as_deref() != Some(new_hash.as_str());

    for (package, patches) in &ready {
        if patches.is_empty() {
            continue;
        }
        let freshly_linked = newly_linked.iter().any(|p| &p.name == package);
        if !freshly_linked && !patch_set_changed {
            // Already patched by a previous run and nothing about the patch
            // set has changed since: nothing to do.
            continue;
        }
        let Some(target) = newly_linked
            .iter()
            .chain(kept.iter().copied())
            .find(|p| &p.name == package)
        else {
            // Patches name a package that isn't actually locked/selected;
            // Composer's own `patchPackage` never fires for it either.
            continue;
        };
        let Some(store_dir) = store.lookup(target) else {
            // ponytail: only archive/store-backed packages are patched (the
            // fixture's case); a `path`/git-source/preferred-source package
            // is already an independent copy or the user's own checkout, so
            // patching it isn't wired up here — upgrade if #53 needs it.
            continue;
        };
        let install_path = crate::install::package_dir(vendor_dir, project_dir, target);
        // Break the store hardlink for this package only, same as
        // `--link-mode copy` does for a whole install.
        link_tree(&store_dir, &install_path, LinkMode::Copy)?;
        for patch in patches {
            apply_git(&install_path, patch)?;
        }
    }
    Ok(())
}
