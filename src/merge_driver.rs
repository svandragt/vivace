//! Wires `viv lock merge` (`lock_merge.rs`, #275) into a git checkout so a
//! developer never has to configure it by hand.
//!
//! `wire` is `install`'s own every-run convenience: once `.gitattributes`
//! names `merge=viv`, it sets `merge.viv.driver` in the clone's own
//! `.git/config` so the *next* merge already runs through viv. `viv init`
//! is the other half (`init.rs`): it writes the `.gitattributes` line
//! itself, so the very first clone of a new project already has something
//! for `wire` to act on.

use std::path::Path;

use anyhow::{Context, Result};

/// The two `.gitattributes` lines that route `composer.lock`/`viv.lock`
/// merges through `viv lock merge` (`docs/research.md` chapter 1); `viv
/// init` is the only writer today.
pub const ATTRIBUTE_LINES: [&str; 2] = ["composer.lock merge=viv", "viv.lock merge=viv"];

/// On every `install`, if this clone's `.gitattributes` names `merge=viv`
/// but its own `.git/config` has no `merge.viv.driver` yet, set one.
/// Convenience only — a fresh clone's first merge already goes through viv
/// without the developer running `git config` by hand — so any failure (no
/// `.gitattributes`, no git repository, git missing) is silent, never an
/// install error.
pub fn wire(project_dir: &Path) {
    let attributes_path = project_dir.join(".gitattributes");
    if !attributes_path.is_file() {
        return;
    }
    let Ok(content) = fs_err::read_to_string(&attributes_path) else {
        return;
    };
    let wants_viv_driver = content
        .lines()
        .any(|line| line.split_whitespace().any(|attr| attr == "merge=viv"));
    if !wants_viv_driver {
        return;
    }

    let already_set = crate::vcs::git_command()
        .args(["config", "--local", "--get", "merge.viv.driver"])
        .current_dir(project_dir)
        .output();
    match already_set {
        Ok(output) if output.status.success() => return, // already configured
        Ok(_) => {}                                      // unset, fall through
        Err(_) => return,                                // no git, or spawn failure
    }

    let set = crate::vcs::git_command()
        .args([
            "config",
            "--local",
            "merge.viv.driver",
            "viv lock merge %O %A %B",
        ])
        .current_dir(project_dir)
        .output();
    if matches!(set, Ok(output) if output.status.success()) {
        stderr_line(
            "viv: set merge.viv.driver in this clone so git merges composer.lock through viv \
             lock merge",
        );
    }
}

/// Appends whichever of `lines` isn't already a line in
/// `project_dir/.gitattributes`, creating the file if it doesn't exist.
/// Returns the lines actually added, in the order given, so the caller can
/// report them in its own style (`viv init`'s stdout, `install`'s stderr).
pub fn ensure_gitattributes(project_dir: &Path, lines: &[&str]) -> Result<Vec<String>> {
    let path = project_dir.join(".gitattributes");
    let existing = fs_err::read_to_string(&path).unwrap_or_default();
    let existing_lines: std::collections::HashSet<&str> = existing.lines().collect();

    let mut added = Vec::new();
    let mut content = existing.clone();
    for &line in lines {
        if existing_lines.contains(line) {
            continue;
        }
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str(line);
        content.push('\n');
        added.push(line.to_string());
    }

    if !added.is_empty() {
        fs_err::write(&path, content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(added)
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr`
/// lint (`lock_merge.rs`'s/`update.rs`'s own `warn_out` does the same).
fn stderr_line(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}
