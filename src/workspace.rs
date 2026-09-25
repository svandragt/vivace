//! `viv workspace list`: chapter 3's first step (#276) — discover workspace
//! members from `extra.viv.workspace.members` glob patterns in the root
//! `composer.json`, and report which other members each one requires by
//! name. Composer has no `extra.viv` key, so a project without
//! `extra.viv.workspace.members` never enters this module and compat mode
//! is untouched.
//!
//! Turning a member's requirement on another member into an actual install
//! (a symlink into the tree, the way pnpm's `workspace:*` protocol works)
//! needs the solver to treat that requirement as already satisfied from the
//! tree instead of a registry lookup — that's #277's one-solve work. This
//! module only discovers the edge and reports it in `list`; wiring it into
//! an install is out of scope here.
//!
//! `viv workspace init`/`viv workspace add` (#315) are a different, plain
//! Composer mechanism: chapter 3's aggregate root, a top-level
//! `composer.json` that requires every member through a `"path"`
//! repository (`#305`). `init` takes CLI glob patterns (not
//! `extra.viv.workspace.members`), writes one `path` repository per
//! pattern and one `require` line per package the patterns match, then
//! resolves and installs through the same `require::partial_update`
//! `viv add` uses. The file it writes is ordinary Composer input — no
//! `extra.viv` key — so `composer install` accepts it too.

use std::collections::{BTreeMap, HashSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde_json::{Map, Value};

use crate::lock::{self, Root, glob_match};
use crate::require;
use crate::show::pad;
use crate::solver::pool_builder::UpdateAllowMode;

/// `viv workspace` flags: which workspace operation to run.
#[derive(Args, Debug, Clone)]
pub struct WorkspaceArgs {
    #[command(subcommand)]
    pub command: WorkspaceCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum WorkspaceCommand {
    /// List the workspace's members: name, version, path (relative to the
    /// root) and which other members each one requires.
    List {
        /// Project directory holding the root composer.json.
        #[arg(short = 'd', long = "project-dir", default_value = ".")]
        project_dir: PathBuf,
    },
    /// Write a top-level composer.json (the aggregate root) from member
    /// glob patterns, then resolve and install (#315). No patterns prints
    /// every composer.json under the project directory instead, grouped by
    /// the directory a pattern for it would target, and writes nothing.
    Init(WorkspaceInitArgs),
    /// Add one more member to an existing aggregate root and resolve again
    /// (#315).
    Add(WorkspaceAddArgs),
}

/// `viv workspace init` flags.
#[derive(Args, Debug, Clone)]
pub struct WorkspaceInitArgs {
    /// Path patterns, e.g. `plugins/* themes/*`; a bare `*` matches every
    /// directory one level below it. Omit to print the discovery listing
    /// instead of writing anything.
    pub patterns: Vec<String>,
    /// Project directory to write composer.json into.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Overwrite an existing composer.json.
    #[arg(long)]
    pub force: bool,
    /// Write composer.json and composer.lock but don't install.
    #[arg(long = "no-install")]
    pub no_install: bool,
}

/// `viv workspace add` flags.
#[derive(Args, Debug, Clone)]
pub struct WorkspaceAddArgs {
    /// Directory holding the new member's composer.json, e.g. `plugins/new-one`.
    pub path: String,
    /// Project directory holding the aggregate root's composer.json.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Write composer.json and composer.lock but don't install.
    #[arg(long = "no-install")]
    pub no_install: bool,
}

/// One discovered workspace member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub version: Option<String>,
    /// Relative to the workspace root, `/`-separated.
    pub path: String,
    /// Other members' names this member's `require` names; linking that
    /// into an install from the tree rather than a registry is #277.
    pub requires: Vec<String>,
}

pub fn run(args: &WorkspaceArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    match &args.command {
        WorkspaceCommand::List { project_dir } => run_list(project_dir),
        WorkspaceCommand::Init(init_args) => run_init(init_args, cache_dir, offline),
        WorkspaceCommand::Add(add_args) => run_add(add_args, cache_dir, offline),
    }
}

fn run_list(project_dir: &Path) -> Result<()> {
    let members = discover(project_dir)?.unwrap_or_default();
    if members.is_empty() {
        out("No workspace members configured (extra.viv.workspace.members).");
        return Ok(());
    }
    print_members(&members);
    Ok(())
}

/// `viv workspace init <pattern>...`: every directory a pattern matches
/// that holds a `composer.json` with a `name` becomes one `require` line;
/// every pattern becomes one `"path"` repository, in argument order, so
/// each is queried (and, being canonical by default, satisfies the names
/// it provides without ever falling through to `packagist.org`) before the
/// implicit default repository is. `--force` mirrors `viv init`'s own
/// overwrite guard, since this is the same "write composer.json and stop
/// if it's already there" shape.
fn run_init(args: &WorkspaceInitArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    if args.patterns.is_empty() {
        return print_scan(&project_dir);
    }

    let composer_json_path = project_dir.join("composer.json");
    if composer_json_path.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to overwrite",
            composer_json_path.display()
        );
    }
    // Only read for `minimum-stability`/`prefer-stable`: a `--force`
    // rewrite otherwise starts fresh, the same "no autodiscovery merge"
    // shape `run_list`'s own `discover` avoids (`docs/research.md` chapter
    // 3's "proposing dry run instead of autodiscovery that writes").
    let existing: Option<Value> = composer_json_path
        .is_file()
        .then(|| -> Result<Value> {
            Ok(serde_json::from_slice(&fs_err::read(&composer_json_path)?)?)
        })
        .transpose()?;

    let mut names: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for pattern in &args.patterns {
        for dir in resolve_glob(&project_dir, pattern)? {
            let member_composer_json = dir.join("composer.json");
            if !member_composer_json.is_file() {
                continue;
            }
            let member = lock::read_root(&member_composer_json)
                .with_context(|| format!("{}: workspace member", member_composer_json.display()))?;
            if let Some(name) = member.name
                && seen.insert(name.clone())
            {
                names.push(name);
            }
        }
    }

    let mut root = Map::new();
    root.insert(
        "name".to_string(),
        Value::String(crate::init::default_name(&project_dir)),
    );
    let specs: Vec<String> = args
        .patterns
        .iter()
        .map(|pattern| format!("path:{pattern}"))
        .collect();
    require::append_repositories(&mut root, &specs)?;
    if !names.is_empty() {
        let mut require_section = Map::new();
        for name in &names {
            require_section.insert(name.clone(), Value::String("*".to_string()));
        }
        root.insert("require".to_string(), Value::Object(require_section));
    }
    root.insert(
        "minimum-stability".to_string(),
        existing
            .as_ref()
            .and_then(|e| e.get("minimum-stability"))
            .cloned()
            .unwrap_or_else(|| Value::String("dev".to_string())),
    );
    root.insert(
        "prefer-stable".to_string(),
        existing
            .as_ref()
            .and_then(|e| e.get("prefer-stable"))
            .cloned()
            .unwrap_or(Value::Bool(true)),
    );

    let existing_contents = fs_err::read_to_string(&composer_json_path).unwrap_or_default();
    let indent = crate::normalize::detect_indent(&existing_contents);
    require::write_composer_json(&composer_json_path, &Value::Object(root))?;
    crate::normalize::maybe_normalize(&composer_json_path, &indent)?;
    out(&format!("Wrote {}", composer_json_path.display()));

    require::partial_update(
        &project_dir,
        cache_dir,
        offline,
        false,
        false,
        &names,
        UpdateAllowMode::OnlyListed,
        false,
        false,
        args.no_install,
        false,
        &[],
        false,
        std::time::Duration::ZERO,
    )
}

/// `viv workspace add <path>`: read `path`'s own `composer.json` for its
/// `name`, add one `require` line for it to the existing aggregate root
/// and resolve again — the same `require::add_link` plus
/// `require::partial_update` `viv add` itself runs, just seeded from a
/// directory instead of a `vendor/package[:constraint]` spec. Assumes
/// `path` is already covered by one of the root's `"path"` repository
/// patterns; if it isn't, the resolve below fails the way any other
/// unresolvable requirement does.
fn run_add(args: &WorkspaceAddArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&composer_json_path).context("reading composer.json")?;
    let mut root: Value = serde_json::from_str(&original).context("parsing composer.json")?;
    if !root.is_object() {
        bail!("composer.json must be a JSON object");
    }

    let member_composer_json = project_dir.join(&args.path).join("composer.json");
    let member = lock::read_root(&member_composer_json)
        .with_context(|| format!("{}: workspace member", member_composer_json.display()))?;
    let name = member
        .name
        .with_context(|| format!("{}: has no \"name\"", member_composer_json.display()))?;

    require::add_link(&mut root, "require", &name, "*")?;

    let indent = crate::normalize::detect_indent(&original);
    require::write_composer_json(&composer_json_path, &root)?;
    crate::normalize::maybe_normalize(&composer_json_path, &indent)?;

    require::partial_update(
        &project_dir,
        cache_dir,
        offline,
        false,
        false,
        &[name],
        UpdateAllowMode::OnlyListed,
        false,
        false,
        args.no_install,
        false,
        &[],
        false,
        std::time::Duration::ZERO,
    )
}

/// `viv workspace init` with no patterns: a dry-run listing of every
/// `composer.json` under `project_dir`, grouped by the directory a
/// `<group>/*` pattern would target, so a person can copy the pattern they
/// want into the real command. Skips `vendor/`/`node_modules/` so an
/// already-installed tree's own manifests don't drown the project's real
/// members. Never writes a file.
fn print_scan(project_dir: &Path) -> Result<()> {
    let mut found = Vec::new();
    scan_composer_json(project_dir, project_dir, &mut found)?;
    found.sort();

    if found.is_empty() {
        out("No composer.json found below the current directory.");
        return Ok(());
    }

    // ponytail: a depth-1 match (`themes/composer.json`, no further `/`)
    // and the project's own root `composer.json` both group under `.`
    // (parent-of-parent and "no parent at all" collapse to the same key);
    // fine for a listing whose job is a copyable pattern, not a precise
    // depth tree. Split them if that conflation ever confuses someone.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in found {
        let group = match path.rsplit_once('/') {
            Some((parent, _)) => parent.to_string(),
            None => ".".to_string(),
        };
        groups.entry(group).or_default().push(path);
    }

    let mut stdout = std::io::stdout().lock();
    for (group, members) in &groups {
        let _ = writeln!(stdout, "{group} ({})", members.len());
        for member in members {
            let _ = writeln!(stdout, "  {member}");
        }
    }
    Ok(())
}

/// Recursively collect every directory under `current` that holds a
/// `composer.json`, as `/`-separated paths relative to `root_dir` (`.` for
/// `root_dir` itself), skipping `vendor`/`node_modules` subtrees entirely.
fn scan_composer_json(root_dir: &Path, current: &Path, found: &mut Vec<String>) -> Result<()> {
    if current.join("composer.json").is_file() {
        found.push(display_relative(root_dir, current));
    }
    let mut entries = Vec::new();
    for entry in fs_err::read_dir(current)? {
        entries.push(entry?);
    }
    entries.sort_by_key(fs_err::DirEntry::file_name);
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if name == "vendor" || name == "node_modules" {
            continue;
        }
        scan_composer_json(root_dir, &entry.path(), found)?;
    }
    Ok(())
}

/// Read `extra.viv.workspace.members` from `project_dir`'s root
/// `composer.json` and resolve it to [`Member`]s. `Ok(None)` when the key
/// is absent: the caller takes the pre-workspace path unchanged.
pub fn discover(project_dir: &Path) -> Result<Option<Vec<Member>>> {
    let root_dir = fs_err::canonicalize(project_dir)
        .with_context(|| format!("{}: project directory", project_dir.display()))?;
    let root = lock::read_root(&root_dir.join("composer.json"))?;
    discover_from_root(&root_dir, &root)
}

fn discover_from_root(root_dir: &Path, root: &Root) -> Result<Option<Vec<Member>>> {
    let Some(patterns) = root
        .extra
        .pointer("/viv/workspace/members")
        .and_then(Value::as_array)
    else {
        return Ok(None);
    };
    let patterns: Vec<&str> = patterns.iter().filter_map(Value::as_str).collect();

    // (canonical member dir, the pattern that first matched it) in match
    // order, so a duplicate or nested refusal can name both sides.
    let mut resolved: Vec<(PathBuf, &str)> = Vec::new();
    for pattern in &patterns {
        for dir in resolve_glob(root_dir, pattern)? {
            let canonical = fs_err::canonicalize(&dir)
                .with_context(|| format!("{}: workspace member", dir.display()))?;
            if !canonical.starts_with(root_dir) {
                bail!(
                    "workspace member {} (matched by \"{pattern}\") is outside the workspace \
                     root {}",
                    display_relative(root_dir, &canonical),
                    root_dir.display()
                );
            }
            if !canonical.join("composer.json").is_file() {
                bail!(
                    "workspace member {} (matched by \"{pattern}\") has no composer.json",
                    display_relative(root_dir, &canonical)
                );
            }
            if let Some((_, first_pattern)) = resolved.iter().find(|(path, _)| *path == canonical) {
                bail!(
                    "workspace member {} is matched twice, by \"{first_pattern}\" and \
                     \"{pattern}\"",
                    display_relative(root_dir, &canonical)
                );
            }
            resolved.push((canonical, pattern));
        }
    }

    for (path, _) in &resolved {
        for (other, _) in &resolved {
            if path != other && path.starts_with(other) {
                bail!(
                    "workspace member {} is nested inside member {}",
                    display_relative(root_dir, path),
                    display_relative(root_dir, other)
                );
            }
        }
    }

    let mut members: Vec<(Root, &Path)> = Vec::new();
    for (path, _) in &resolved {
        let member_root = lock::read_root(&path.join("composer.json"))
            .with_context(|| format!("{}: workspace member", display_relative(root_dir, path)))?;
        members.push((member_root, path.as_path()));
    }

    let names: HashSet<String> = members
        .iter()
        .filter_map(|(member_root, _)| member_root.name.clone())
        .collect();

    let mut result: Vec<Member> = members
        .iter()
        .map(|(member_root, path)| {
            let requires = member_root
                .require
                .keys()
                .filter(|name| names.contains(*name))
                .cloned()
                .collect();
            Member {
                name: member_root
                    .name
                    .clone()
                    .unwrap_or_else(|| display_relative(root_dir, path)),
                version: member_root.version.clone(),
                path: display_relative(root_dir, path),
                requires,
            }
        })
        .collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(Some(result))
}

/// Resolve one glob pattern to the directories it matches under
/// `root_dir`, in directory-listing order. Only a bare `*` wildcard per
/// path segment is supported (the same shape `glob_match` matches package
/// names against for `config.allow-plugins`/`preferred-install`), no `**`;
/// a literal segment (including `..`) is passed through untouched, so the
/// outside-root check in [`discover_from_root`] can catch and name it
/// rather than this silently clamping it to the root.
fn resolve_glob(root_dir: &Path, pattern: &str) -> Result<Vec<PathBuf>> {
    let segments: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    expand(root_dir, &segments)
}

fn expand(current: &Path, segments: &[&str]) -> Result<Vec<PathBuf>> {
    let Some((segment, rest)) = segments.split_first() else {
        return Ok(if current.is_dir() {
            vec![current.to_path_buf()]
        } else {
            Vec::new()
        });
    };
    if !segment.contains('*') {
        return expand(&current.join(segment), rest);
    }
    if !current.is_dir() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs_err::read_dir(current)? {
        entries.push(entry?);
    }
    entries.sort_by_key(fs_err::DirEntry::file_name);

    let mut matches = Vec::new();
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name();
        if glob_match(segment, &name.to_string_lossy()) {
            matches.extend(expand(&entry.path(), rest)?);
        }
    }
    Ok(matches)
}

/// `path` relative to `root_dir`, `/`-separated regardless of platform, for
/// error messages and the `list` table's own path column. Falls back to
/// `path`'s own display when it isn't under `root_dir` (the outside-root
/// refusal names the offending path this way).
fn display_relative(root_dir: &Path, path: &Path) -> String {
    match path.strip_prefix(root_dir) {
        Ok(relative) => relative
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

fn print_members(members: &[Member]) {
    let name_len = members
        .iter()
        .map(|m| m.name.chars().count())
        .max()
        .unwrap_or(0);
    let version_len = members
        .iter()
        .map(|m| m.version.as_deref().unwrap_or("-").chars().count())
        .max()
        .unwrap_or(0);
    let path_len = members
        .iter()
        .map(|m| m.path.chars().count())
        .max()
        .unwrap_or(0);

    let mut stdout = std::io::stdout().lock();
    for member in members {
        let version = member.version.as_deref().unwrap_or("-");
        let requires = if member.requires.is_empty() {
            "-".to_string()
        } else {
            member.requires.join(", ")
        };
        let _ = writeln!(
            stdout,
            "{} {} {} {requires}",
            pad(&member.name, name_len),
            pad(version, version_len),
            pad(&member.path, path_len),
        );
    }
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_json(path: &Path, value: &Value) {
        fs_err::create_dir_all(path.parent().unwrap()).unwrap();
        fs_err::write(path, serde_json::to_vec(value).unwrap()).unwrap();
    }

    fn member_json(name: &str, requires: &[&str]) -> Value {
        let require: serde_json::Map<String, Value> = requires
            .iter()
            .map(|r| ((*r).to_string(), Value::String("*".to_string())))
            .collect();
        serde_json::json!({"name": name, "version": "1.0.0", "require": require})
    }

    #[test]
    fn absent_workspace_key_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("composer.json"),
            &serde_json::json!({"name": "acme/root"}),
        );

        assert!(discover(dir.path()).unwrap().is_none());
    }

    #[test]
    fn discovers_members_and_the_inter_member_edge() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("composer.json"),
            &serde_json::json!({
                "name": "acme/root",
                "extra": {"viv": {"workspace": {"members": ["packages/*"]}}}
            }),
        );
        write_json(
            &dir.path().join("packages/foo/composer.json"),
            &member_json("acme/foo", &[]),
        );
        write_json(
            &dir.path().join("packages/bar/composer.json"),
            &member_json("acme/bar", &["acme/foo"]),
        );

        let members = discover(dir.path()).unwrap().unwrap();

        assert_eq!(members.len(), 2);
        let bar = members.iter().find(|m| m.name == "acme/bar").unwrap();
        assert_eq!(bar.requires, vec!["acme/foo".to_string()]);
        assert_eq!(bar.path, "packages/bar");
        let foo = members.iter().find(|m| m.name == "acme/foo").unwrap();
        assert!(foo.requires.is_empty());
    }

    #[test]
    fn refuses_a_member_with_no_composer_json() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("composer.json"),
            &serde_json::json!({
                "name": "acme/root",
                "extra": {"viv": {"workspace": {"members": ["packages/*"]}}}
            }),
        );
        fs_err::create_dir_all(dir.path().join("packages/empty")).unwrap();

        let err = discover(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("packages/empty") && err.to_string().contains("composer.json"),
            "{err}"
        );
    }

    #[test]
    fn refuses_the_same_member_matched_twice() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("composer.json"),
            &serde_json::json!({
                "name": "acme/root",
                "extra": {"viv": {"workspace": {"members": ["packages/*", "packages/foo"]}}}
            }),
        );
        write_json(
            &dir.path().join("packages/foo/composer.json"),
            &member_json("acme/foo", &[]),
        );

        let err = discover(dir.path()).unwrap_err();
        assert!(
            err.to_string().contains("packages/foo") && err.to_string().contains("matched twice"),
            "{err}"
        );
    }

    #[test]
    fn refuses_a_member_nested_inside_another() {
        let dir = tempfile::tempdir().unwrap();
        write_json(
            &dir.path().join("composer.json"),
            &serde_json::json!({
                "name": "acme/root",
                "extra": {"viv": {"workspace": {"members": ["packages/foo", "packages/foo/nested"]}}}
            }),
        );
        write_json(
            &dir.path().join("packages/foo/composer.json"),
            &member_json("acme/foo", &[]),
        );
        write_json(
            &dir.path().join("packages/foo/nested/composer.json"),
            &member_json("acme/nested", &[]),
        );

        let err = discover(dir.path()).unwrap_err();
        assert!(err.to_string().contains("nested inside member"), "{err}");
    }

    #[test]
    fn refuses_a_member_outside_the_root() {
        let workspace = tempfile::tempdir().unwrap();
        let root_dir = workspace.path().join("root");
        let outside_dir = workspace.path().join("outside");
        write_json(
            &root_dir.join("composer.json"),
            &serde_json::json!({
                "name": "acme/root",
                "extra": {"viv": {"workspace": {"members": ["../outside"]}}}
            }),
        );
        write_json(
            &outside_dir.join("composer.json"),
            &member_json("acme/outside", &[]),
        );

        let err = discover(&root_dir).unwrap_err();
        assert!(
            err.to_string().contains("outside the workspace root"),
            "{err}"
        );
    }
}
