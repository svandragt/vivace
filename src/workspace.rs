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

use std::collections::HashSet;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use serde_json::Value;

use crate::lock::{self, Root, glob_match};
use crate::show::pad;

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

pub fn run(args: &WorkspaceArgs) -> Result<()> {
    match &args.command {
        WorkspaceCommand::List { project_dir } => run_list(project_dir),
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
