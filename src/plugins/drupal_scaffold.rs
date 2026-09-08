//! `drupal/core-composer-scaffold`'s `Handler::scaffold` (`POST_INSTALL_CMD`/
//! `POST_UPDATE_CMD`) and `Plugin::preAutoloadDump` (`PRE_AUTOLOAD_DUMP`):
//! copies scaffold files (Drupal's `index.php`, `.htaccess`, `settings.php`,
//! etc.) from every allowed package into the project, then writes
//! `vendor/drupal/DrupalInstalled.php` and points the root package's
//! classmap at it (plus, conditionally, a handful of framework classes) so
//! Composer's autoloader can find it without a `require`.
//!
//! Ported from `drupal/core-composer-scaffold` `11.x-dev` (`84d66ad`, fetched
//! 2026-09-08, issue #93): `Handler`, `AllowedPackages`, `ManageOptions`,
//! `ManageGitIgnore`, `Operations/*`, `GenerateAutoloadReferenceFile` and
//! `GenerateAutoloadRuntimeReferenceFile`. Runs from the same phase as
//! [`super::patches::apply`] (right after linking, while install directories
//! are known), since a scaffolded file can be produced from a package that
//! isn't `vendor/<name>` (`composer/installers` remaps `drupal/core` itself).
//!
//! ponytail: `ScaffoldOptions::symlink()` (a project can opt into symlinking
//! scaffold files instead of copying) isn't ported — every fixture and every
//! corpus project scaffolds via a plain copy; add a `symlink` field to
//! [`ResolvedOp::Replace`] and call `crate::link`'s relative-symlink helper
//! if a project setting `extra.drupal-scaffold.symlink: true` ever shows up.
//!
//! ponytail: `Handler::scaffold`'s `checkUnchanged`/`filterFiles` pass (skip
//! rewriting a file whose on-disk bytes already match) isn't ported: every
//! call site here runs once per fresh `vendor/`, so there's never a stale
//! scaffold file to compare against. Port it if `viv install` ever re-runs
//! this against an existing scaffold without reinstalling every package.
//!
//! ponytail: `Git::checkIgnore`/`checkTracked`/`isRepository` shell out to a
//! `git` subprocess per candidate file, same as the real plugin — cheap
//! relative to the file copies themselves, and reusing the actual `git`
//! binary avoids re-implementing gitignore pattern matching. Worth batching
//! into one `git check-ignore --stdin` call if a project with hundreds of
//! scaffold files ever makes this show up in a profile.

use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use crate::autoload::generator::find_shortest_path;
use crate::install::package_dir;
use crate::lock::{Package, Root};

/// `ScaffoldOptions::create`: the `extra.drupal-scaffold` section of one
/// package's (or the root's) `composer.json`.
struct ScaffoldOptions {
    allowed_packages: Vec<String>,
    locations: HashMap<String, String>,
    file_mapping: Map<String, Value>,
    gitignore: Option<bool>,
}

fn scaffold_options(extra: &Value) -> ScaffoldOptions {
    let opts = extra.get("drupal-scaffold");
    let allowed_packages = opts
        .and_then(|o| o.get("allowed-packages"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    let locations = opts
        .and_then(|o| o.get("locations"))
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_owned())))
                .collect()
        })
        .unwrap_or_default();
    let file_mapping = opts
        .and_then(|o| o.get("file-mapping"))
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let gitignore = opts
        .and_then(|o| o.get("gitignore"))
        .and_then(Value::as_bool);
    ScaffoldOptions {
        allowed_packages,
        locations,
        file_mapping,
        gitignore,
    }
}

/// A package (or the root project) allowed to scaffold files, in the order
/// [`allowed_entries`] resolved them — later entries win on a shared
/// destination.
enum Origin<'p> {
    Package(&'p Package),
    Root,
}

impl Origin<'_> {
    fn dir(&self, project_dir: &Path, vendor_dir: &Path) -> PathBuf {
        match self {
            Origin::Package(p) => package_dir(vendor_dir, project_dir, p),
            Origin::Root => project_dir.to_path_buf(),
        }
    }

    fn file_mapping(&self, root: &Root) -> Map<String, Value> {
        match self {
            Origin::Package(p) => {
                scaffold_options(p.raw.pointer("/extra").unwrap_or(&Value::Null)).file_mapping
            }
            Origin::Root => scaffold_options(&root.extra).file_mapping,
        }
    }
}

/// `AllowedPackages::getAllowedPackages`: `drupal/legacy-scaffold-assets` and
/// `drupal/core` implicitly first, then the root's own `allowed-packages` in
/// order, each recursively expanded via its own `allowed-packages`
/// (preorder DFS, first occurrence wins), then the root itself last if it
/// declares any `file-mapping` of its own — highest priority, since it's
/// applied last. A name with no installed package (never required, or an
/// optional scaffold source the project doesn't pull in) is silently
/// dropped, same as Composer's own `findPackage` returning nothing.
fn allowed_entries<'p>(
    root: &Root,
    packages_by_name: &HashMap<&'p str, &'p Package>,
) -> Vec<Origin<'p>> {
    let root_opts = scaffold_options(&root.extra);
    let mut top_level = vec![
        "drupal/legacy-scaffold-assets".to_string(),
        "drupal/core".to_string(),
    ];
    top_level.extend(root_opts.allowed_packages.iter().cloned());

    let mut order: Vec<&'p str> = Vec::new();
    let mut seen: HashSet<&'p str> = HashSet::new();
    recurse_allowed(&top_level, packages_by_name, &mut order, &mut seen);

    let mut entries: Vec<Origin<'p>> = order
        .into_iter()
        .map(|name| Origin::Package(packages_by_name[name]))
        .collect();

    if !root_opts.file_mapping.is_empty() {
        entries.push(Origin::Root);
    }
    entries
}

fn recurse_allowed<'p>(
    names: &[String],
    packages_by_name: &HashMap<&'p str, &'p Package>,
    order: &mut Vec<&'p str>,
    seen: &mut HashSet<&'p str>,
) {
    for name in names {
        let Some((&key, &package)) = packages_by_name.get_key_value(name.as_str()) else {
            continue;
        };
        if !seen.insert(key) {
            continue;
        }
        order.push(key);
        let opts = scaffold_options(package.raw.pointer("/extra").unwrap_or(&Value::Null));
        recurse_allowed(&opts.allowed_packages, packages_by_name, order, seen);
    }
}

/// One file-mapping entry, normalized (`OperationData::normalizeScaffoldMetadata`)
/// but not yet resolved against whatever else already claimed this
/// destination.
enum PendingOp {
    Replace {
        source: PathBuf,
        overwrite: bool,
    },
    Append {
        prepend: Option<PathBuf>,
        append: Option<PathBuf>,
        default: Option<PathBuf>,
        force_append: bool,
    },
}

/// `OperationData`'s normalization plus `OperationFactory::create`: `None`
/// means a `SkipOp`, which never writes a file and never overrides a
/// previous package's op at the same destination (`build_op`'s caller drops
/// the map entry entirely instead).
fn build_op(package_dir: &Path, dest: &str, raw: &Value) -> Result<Option<PendingOp>> {
    if let Value::Bool(allowed) = raw {
        if *allowed {
            bail!("File mapping {dest} cannot be given the value 'true'.");
        }
        return Ok(None);
    }
    let obj: Map<String, Value> = match raw {
        Value::String(s) => {
            let mut m = Map::new();
            m.insert("path".to_string(), Value::String(s.clone()));
            m
        }
        Value::Object(m) => m.clone(),
        _ => bail!("File mapping {dest} cannot be empty."),
    };
    let mode = obj.get("mode").and_then(Value::as_str).map_or_else(
        || {
            if obj.contains_key("append") || obj.contains_key("prepend") {
                "append".to_string()
            } else {
                "replace".to_string()
            }
        },
        str::to_owned,
    );
    match mode.as_str() {
        "skip" => Ok(None),
        "replace" => {
            let path = obj.get("path").and_then(Value::as_str).with_context(|| {
                format!("{dest}: 'path' component required for 'replace' operations")
            })?;
            let source = package_dir.join(path);
            if !source.is_file() {
                bail!("Scaffold file {path} not found in package.");
            }
            let overwrite = obj
                .get("overwrite")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            Ok(Some(PendingOp::Replace { source, overwrite }))
        }
        "append" => {
            let resolve = |key: &str| {
                obj.get(key)
                    .and_then(Value::as_str)
                    .map(|p| package_dir.join(p))
            };
            let prepend = resolve("prepend");
            let append = resolve("append");
            let default = resolve("default");
            let has_content = |p: &Option<PathBuf>| {
                p.as_ref()
                    .and_then(|p| p.metadata().ok())
                    .is_some_and(|m| m.len() > 0)
            };
            if !has_content(&prepend) && !has_content(&append) {
                // `OperationFactory::createAppendOp`: nothing to add, so this
                // never touches the destination at all.
                return Ok(None);
            }
            let force_append = default.is_some()
                || obj
                    .get("force-append")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            Ok(Some(PendingOp::Append {
                prepend,
                append,
                default,
                force_append,
            }))
        }
        other => bail!("Unknown scaffold operation mode {other}."),
    }
}

/// A file-mapping entry resolved against whatever else already claimed this
/// destination, ready to be written to disk.
enum ResolvedOp {
    Replace {
        source: PathBuf,
        overwrite: bool,
    },
    Append {
        prepend: Option<PathBuf>,
        append: Option<PathBuf>,
        default: Option<PathBuf>,
        original: Option<Vec<u8>>,
        managed: bool,
    },
}

struct Resolved {
    dest_full: PathBuf,
    op: ResolvedOp,
}

/// `AbstractOperation::contents()`/`generateContents()`: the exact bytes this
/// op would write, used to capture a previous package's rendered contents
/// when a later package appends to the same destination.
fn render(op: &ResolvedOp) -> Result<Vec<u8>> {
    match op {
        ResolvedOp::Replace { source, .. } => Ok(fs_err::read(source)?),
        ResolvedOp::Append {
            prepend,
            append,
            default,
            original,
            ..
        } => append_contents(
            prepend.as_ref(),
            append.as_ref(),
            default.as_ref(),
            original.as_ref(),
        ),
    }
}

/// `AppendOp::generateContents`: prepend data, then the original contents (or
/// the default data if the original is empty), then append data.
fn append_contents(
    prepend: Option<&PathBuf>,
    append: Option<&PathBuf>,
    default: Option<&PathBuf>,
    original: Option<&Vec<u8>>,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    if let Some(path) = prepend {
        out.extend(fs_err::read(path)?);
        out.push(b'\n');
    }
    let mut middle = original.cloned().unwrap_or_default();
    if middle.is_empty()
        && let Some(path) = default
    {
        middle = fs_err::read(path)?;
    }
    out.extend(middle);
    if let Some(path) = append {
        out.push(b'\n');
        out.extend(fs_err::read(path)?);
    }
    Ok(out)
}

/// `[web-root]`/`[project-root]` token replacement
/// (`ScaffoldFilePath::destinationPath`'s `Interpolator`, narrowed to the two
/// tokens vivace ever needs to resolve).
fn interpolate(dest: &str, web_root: &Path, project_root: &Path) -> PathBuf {
    let s = dest
        .replace("[web-root]", &web_root.to_string_lossy())
        .replace("[project-root]", &project_root.to_string_lossy());
    PathBuf::from(s)
}

/// `ManageOptions::ensureLocations`: a declared location (default `.`),
/// created if missing, resolved to an absolute path.
fn resolve_location(project_dir: &Path, declared: Option<&String>) -> Result<PathBuf> {
    let rel = declared.map_or(".", String::as_str);
    let dir = project_dir.join(rel);
    fs_err::create_dir_all(&dir)?;
    Ok(fs_err::canonicalize(&dir)?)
}

fn git_is_repository(dir: &Path) -> bool {
    Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn git_check_ignore(dir: &Path, path: &Path) -> bool {
    Command::new("git")
        .arg("check-ignore")
        .arg(path)
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success())
}

fn git_check_tracked(dir: &Path, path: &Path) -> bool {
    Command::new("git")
        .args(["ls-files", "--error-unmatch"])
        .arg(path)
        .current_dir(dir)
        .output()
        .is_ok_and(|o| o.status.success())
}

/// `ManageGitIgnore::manageIgnored`/`addToGitIgnore`: adds every managed,
/// untracked, not-already-ignored scaffold result to the `.gitignore` in its
/// own directory, sorted, one entry per line.
fn manage_gitignore(
    project_dir: &Path,
    root_opts: &ScaffoldOptions,
    candidates: &[(PathBuf, bool)],
) -> Result<()> {
    let enabled = match root_opts.gitignore {
        Some(v) => v,
        None => {
            git_is_repository(project_dir) && git_check_ignore(project_dir, Path::new("vendor"))
        }
    };
    if !enabled {
        return Ok(());
    }

    let mut by_dir: HashMap<PathBuf, Vec<String>> = HashMap::new();
    for (path, managed) in candidates {
        if !managed {
            continue;
        }
        if git_check_ignore(project_dir, path) {
            continue;
        }
        if git_check_tracked(project_dir, path) {
            continue;
        }
        let dir = fs_err::canonicalize(path.parent().expect("scaffold destination has a parent"))?;
        let name = path
            .file_name()
            .expect("scaffold destination has a file name");
        by_dir
            .entry(dir)
            .or_default()
            .push(format!("/{}", name.to_string_lossy()));
    }
    for (dir, mut entries) in by_dir {
        entries.sort();
        let path = dir.join(".gitignore");
        let mut contents = if path.is_file() {
            fs_err::read_to_string(&path)?
        } else {
            String::new()
        };
        if !contents.is_empty() && !contents.ends_with('\n') {
            contents.push('\n');
        }
        contents.push_str(&entries.join("\n"));
        fs_err::write(&path, contents)?;
    }
    Ok(())
}

const AUTOLOAD_HEADER: &str = "\
/**
 * @file
 * Includes the autoloader created by Composer.
 *
 * This file was generated by drupal-scaffold.
 *
 * @see composer.json
 * @see index.php
 * @see core/install.php
 * @see core/rebuild.php
 */

";

const AUTOLOAD_RUNTIME_HEADER: &str = "\
/**
 * @file
 * Includes the autoload_runtime created by the Symfony Runtime component.
 *
 * This file was generated by drupal-scaffold.
 *
 * @see composer.json
 * @see index.php
 * @see core/install.php
 * @see core/rebuild.php
 */

use Drupal\\Core\\Runtime\\DrupalRuntime;

// By default, the symfony/runtime component would load SymfonyRuntime as its
// runtime. However, Drupal's Kernel has a lot of runtime components that it
// expects to be prepared. Thus, we default Drupal applications to DrupalRuntime
// instead to make this easily accessible.
$_ENV['APP_RUNTIME'] ??= $_SERVER['APP_RUNTIME'] ?? DrupalRuntime::class;
";

/// `Handler::scaffold`: copy every allowed package's scaffold files into the
/// project, write the `[web-root]/autoload.php`/`autoload_runtime.php`
/// shims, then manage `.gitignore`. Runs from `install::run`, right after
/// linking (`docs/plugin-strategy.md`'s adapter-phase rule; same call site
/// as [`super::patches::apply`]).
pub(super) fn apply(
    root: &Root,
    project_dir: &Path,
    vendor_dir: &Path,
    packages: &[&Package],
) -> Result<()> {
    let packages_by_name: HashMap<&str, &Package> =
        packages.iter().map(|p| (p.name.as_str(), *p)).collect();
    let entries = allowed_entries(root, &packages_by_name);
    if entries.is_empty() {
        return Ok(());
    }

    let root_opts = scaffold_options(&root.extra);
    let web_root = resolve_location(project_dir, root_opts.locations.get("web-root"))?;
    let project_root = resolve_location(project_dir, root_opts.locations.get("project-root"))?;

    let mut map: HashMap<String, Resolved> = HashMap::new();
    for entry in &entries {
        let dir = entry.dir(project_dir, vendor_dir);
        let mapping = entry.file_mapping(root);
        for (dest, raw) in &mapping {
            let dest_full = interpolate(dest, &web_root, &project_root);
            let Some(pending) = build_op(&dir, dest, raw)? else {
                // An explicit `false` (skip) or a no-op append cancels
                // whatever an earlier package scaffolded at this path.
                map.remove(dest);
                continue;
            };
            let op = match pending {
                PendingOp::Replace { source, overwrite } => {
                    ResolvedOp::Replace { source, overwrite }
                }
                PendingOp::Append {
                    prepend,
                    append,
                    default,
                    force_append,
                } => {
                    if let Some(previous) = map.get(dest) {
                        ResolvedOp::Append {
                            prepend,
                            append,
                            default,
                            original: Some(render(&previous.op)?),
                            managed: true,
                        }
                    } else if !force_append {
                        continue;
                    } else if !dest_full.is_file() {
                        if default.is_some() {
                            ResolvedOp::Append {
                                prepend,
                                append,
                                default,
                                original: None,
                                managed: false,
                            }
                        } else {
                            continue;
                        }
                    } else {
                        let existing = fs_err::read(&dest_full)?;
                        let has_data = |p: &Option<PathBuf>| {
                            p.as_ref()
                                .and_then(|p| fs_err::read(p).ok())
                                .is_some_and(|data| contains(&existing, &data))
                        };
                        if has_data(&append) || has_data(&prepend) {
                            continue;
                        }
                        ResolvedOp::Append {
                            prepend,
                            append,
                            default,
                            original: Some(existing),
                            managed: false,
                        }
                    }
                }
            };
            map.insert(dest.clone(), Resolved { dest_full, op });
        }
    }

    let mut gitignore_candidates: Vec<(PathBuf, bool)> = Vec::new();
    for resolved in map.values() {
        match &resolved.op {
            ResolvedOp::Replace { source, overwrite } => {
                if *overwrite || !resolved.dest_full.exists() {
                    if let Some(parent) = resolved.dest_full.parent() {
                        fs_err::create_dir_all(parent)?;
                    }
                    let _ = fs_err::remove_file(&resolved.dest_full);
                    fs_err::copy(source, &resolved.dest_full)?;
                }
                gitignore_candidates.push((resolved.dest_full.clone(), *overwrite));
            }
            ResolvedOp::Append {
                prepend,
                append,
                default,
                original,
                managed,
            } => {
                let content = append_contents(
                    prepend.as_ref(),
                    append.as_ref(),
                    default.as_ref(),
                    original.as_ref(),
                )?;
                fs_err::write(&resolved.dest_full, content)?;
                gitignore_candidates.push((resolved.dest_full.clone(), *managed));
            }
        }
    }

    // `GenerateAutoloadReferenceFile`/`GenerateAutoloadRuntimeReferenceFile`:
    // a fixed autoload/autoload_runtime shim in the web root, regenerated
    // unless it's already there and committed to git.
    for (name, header, target) in [
        (
            "autoload.php",
            AUTOLOAD_HEADER,
            vendor_dir.join("autoload.php"),
        ),
        (
            "autoload_runtime.php",
            AUTOLOAD_RUNTIME_HEADER,
            vendor_dir.join("autoload_runtime.php"),
        ),
    ] {
        let dest = web_root.join(name);
        if dest.is_file() && git_check_tracked(project_dir, &dest) {
            continue;
        }
        let rel = find_shortest_path(&dest.to_string_lossy(), &target.to_string_lossy(), false);
        let rel = rel.strip_prefix("./").unwrap_or(&rel);
        fs_err::write(
            &dest,
            format!("<?php\n\n{header}return require __DIR__ . '/{rel}';\n"),
        )?;
        gitignore_candidates.push((dest, true));
    }

    manage_gitignore(project_dir, &root_opts, &gitignore_candidates)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// `Plugin::preAutoloadDump`: writes `vendor/drupal/DrupalInstalled.php` and
/// returns the classmap entries to add to the root package's autoload
/// (this file, plus a handful of framework classes conditional on their
/// package being installed) — [`super::Plugins::apply_pre_autoload_dump`]'s
/// caller merges these into `Input.root.autoload.classmap` before the
/// classmap scan, the same seam that already resolves any other absolute
/// classmap path.
pub(super) fn pre_autoload_dump(
    root: &Root,
    project_dir: &Path,
    vendor_dir: &Path,
    packages: &[&Package],
) -> Result<Vec<String>> {
    let installed = |name: &str| packages.iter().any(|p| p.name == name);
    let mut classmap = Vec::new();
    if installed("symfony/http-foundation") {
        for file in [
            "FileBag",
            "HeaderBag",
            "HeaderUtils",
            "ParameterBag",
            "Request",
            "RequestStack",
            "ServerBag",
        ] {
            classmap.push(path_string(
                &vendor_dir.join(format!("symfony/http-foundation/{file}.php")),
            ));
        }
    }
    if installed("symfony/http-kernel") {
        for file in ["HttpKernel", "HttpKernelInterface", "TerminableInterface"] {
            classmap.push(path_string(
                &vendor_dir.join(format!("symfony/http-kernel/{file}.php")),
            ));
        }
    }
    if installed("symfony/dependency-injection") {
        classmap.push(path_string(
            &vendor_dir.join("symfony/dependency-injection/ContainerInterface.php"),
        ));
    }
    if installed("psr/container") {
        classmap.push(path_string(
            &vendor_dir.join("psr/container/src/ContainerInterface.php"),
        ));
    }

    let drupal_dir = vendor_dir.join("drupal");
    fs_err::create_dir_all(&drupal_dir)?;
    let hash = version_hash(root, project_dir, packages)?;
    let installed_path = drupal_dir.join("DrupalInstalled.php");
    fs_err::write(
        &installed_path,
        format!(
            "<?php\n\nnamespace Drupal;\n\n\
             /**\n\
             \x20* A class containing information determined during composer installation.\n\
             \x20*\n\
             \x20* This file is generated automatically by the\n\
             \x20* drupal/core-composer-scaffold Composer plugin, and should not be\n\
             \x20* edited.\n\
             \x20*\n\
             \x20* @see \\Drupal\\Composer\\Plugin\\Scaffold\\Plugin::preAutoloadDump()\n\
             \x20*/\n\
             class DrupalInstalled {{\n\n\
             \x20\x20/**\n\
             \x20\x20\x20* A hash of all the installed packages and their versions.\n\
             \x20\x20\x20*/\n\
             \x20\x20public const string VERSIONS_HASH = '{hash}';\n\n\
             }}\n"
        ),
    )?;
    classmap.push(path_string(&installed_path));
    Ok(classmap)
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// `DrupalInstalledTemplate::getCode`'s version hash: `xxh3` of every
/// installed package's `getUniqueName()-getSourceReference()` (sorted by
/// unique name), plus the root package's own, `|`-joined. `packages` is
/// whatever the local repository holds for this run — metapackages
/// included, since Composer's own repository carries them too.
fn version_hash(root: &Root, project_dir: &Path, packages: &[&Package]) -> Result<String> {
    let mut entries: Vec<(String, Option<String>)> = Vec::with_capacity(packages.len());
    for package in packages {
        let version = crate::version::normalize(&package.version)
            .with_context(|| format!("{}: normalising version", package.name))?;
        let reference = package.source.as_ref().and_then(|s| s.reference.clone());
        entries.push((format!("{}-{version}", package.name), reference));
    }
    entries.sort_by(|a, b| a.0.cmp(&b.0));

    let mut versions = String::new();
    for (unique_name, reference) in &entries {
        versions.push_str(unique_name);
        versions.push('-');
        if let Some(reference) = reference {
            versions.push_str(reference);
        }
        versions.push('|');
    }

    let root_name = root.name.clone().unwrap_or_else(|| "__root__".to_string());
    // Same guard as `installed_php`'s own root version: an explicit
    // `version` in composer.json always wins, and skips the git guess (and
    // the reference that comes with it) entirely.
    let root_guess = root
        .version
        .is_none()
        .then(|| crate::vcs::guess_root_version(project_dir))
        .flatten();
    let root_pretty = root
        .version
        .clone()
        .or_else(|| root_guess.as_ref().map(|v| v.pretty_version.clone()))
        .unwrap_or_else(|| "1.0.0+no-version-set".to_string());
    let root_version = crate::version::normalize(&root_pretty).context("root version")?;
    let _ = write!(versions, "{root_name}-{root_version}");
    versions.push('-');
    if let Some(guess) = &root_guess {
        versions.push_str(&guess.reference);
    }

    Ok(format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(versions.as_bytes())
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::{read_lock, read_root};

    fn copy_tree(from: &Path, to: &Path) {
        fs_err::create_dir_all(to).unwrap();
        for entry in fs_err::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                fs_err::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn append_contents_prepends_and_appends_around_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let prepend = dir.path().join("prepend");
        let append = dir.path().join("append");
        fs_err::write(&prepend, "before").unwrap();
        fs_err::write(&append, "after").unwrap();
        let middle = b"middle".to_vec();
        let got = append_contents(Some(&prepend), Some(&append), None, Some(&middle)).unwrap();
        assert_eq!(got, b"before\nmiddle\nafter");
    }

    #[test]
    fn append_contents_falls_back_to_default_when_original_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let default = dir.path().join("default");
        fs_err::write(&default, "fallback").unwrap();
        let got = append_contents(None, None, Some(&default), None).unwrap();
        assert_eq!(got, b"fallback");
    }

    #[test]
    fn build_op_treats_false_as_skip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            build_op(dir.path(), "dest", &Value::Bool(false))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn build_op_rejects_true() {
        let dir = tempfile::tempdir().unwrap();
        assert!(build_op(dir.path(), "dest", &Value::Bool(true)).is_err());
    }

    #[test]
    fn build_op_a_bare_string_is_a_replace_with_default_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::write(dir.path().join("src.txt"), "hi").unwrap();
        let op = build_op(dir.path(), "dest", &Value::String("src.txt".to_string()))
            .unwrap()
            .unwrap();
        assert!(matches!(
            op,
            PendingOp::Replace {
                overwrite: true,
                ..
            }
        ));
    }

    #[test]
    fn build_op_append_without_content_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let raw = serde_json::json!({"append": "missing.txt"});
        assert!(build_op(dir.path(), "dest", &raw).unwrap().is_none());
    }

    /// #93's fixture, byte-diffed against real `drupal/core-composer-scaffold`
    /// 11.4.6's own output (`tests/fixtures/plugins/drupal/expected`):
    /// allowed-packages ordering (a later package's op wins), every op
    /// (replace/append/prepend/skip), `overwrite: false` against a
    /// pre-existing file, the `.gitignore`/autoload shims and the
    /// `DrupalInstalled.php` classmap addition.
    #[test]
    fn drupal_fixture_matches_composers_golden() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/drupal");
        let root = read_root(&dir.join("composer.json")).unwrap();
        let lock = read_lock(&dir.join("composer.lock")).unwrap();
        let packages: Vec<&Package> = lock.packages(true).collect();

        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        copy_tree(&dir.join("seed"), project_dir.path());
        // `link_archives`' own equivalent for the two `path` packages: a
        // plain copy stands in for Composer's symlink, since `apply` only
        // ever reads from a package's install dir.
        copy_tree(
            &dir.join("packages/base"),
            &vendor_dir.join("acme/drupal-scaffold-base"),
        );
        copy_tree(
            &dir.join("packages/override"),
            &vendor_dir.join("acme/drupal-scaffold-override"),
        );

        apply(&root, project_dir.path(), &vendor_dir, &packages).unwrap();
        let classmap =
            pre_autoload_dump(&root, project_dir.path(), &vendor_dir, &packages).unwrap();
        assert!(
            classmap
                .iter()
                .any(|p| p.ends_with("drupal/DrupalInstalled.php"))
        );

        let expected = dir.join("expected");
        for rel in [
            ".editorconfig",
            "web/robots.txt",
            "web/index.php",
            "web/.htaccess",
            "web/router.php",
            "web/settings-extra.php",
            "web/sites/default/default.settings.php",
            "web/.gitignore",
            "web/sites/default/.gitignore",
            "web/autoload.php",
            "web/autoload_runtime.php",
        ] {
            let want = fs_err::read_to_string(expected.join(rel)).unwrap();
            let got = fs_err::read_to_string(project_dir.path().join(rel)).unwrap();
            assert_eq!(got, want, "{rel} differs");
        }
        let want =
            fs_err::read_to_string(expected.join("vendor/drupal/DrupalInstalled.php")).unwrap();
        let got = fs_err::read_to_string(vendor_dir.join("drupal/DrupalInstalled.php")).unwrap();
        assert_eq!(got, want, "vendor/drupal/DrupalInstalled.php differs");
    }
}
