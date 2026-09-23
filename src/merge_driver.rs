//! Wires `viv lock merge` (`lock_merge.rs`, #275) into a git checkout so a
//! developer never has to configure it, or recover from it not being
//! configured, by hand.
//!
//! `wire` (#298) is `install`'s own every-run convenience: once
//! `.gitattributes` names `merge=viv`, it sets `merge.viv.driver` in the
//! clone's own `.git/config` so the *next* merge already runs through viv.
//! `resolve_composer_lock_conflict`/`resolve_viv_lock_conflict` (#299) are
//! `install`'s fallback for the merge that already happened without a
//! driver configured: git leaves the three-way merge in its index stages
//! (1 = base/`%O`, 2 = ours/`%A`, 3 = theirs/`%B`) whether or not a driver
//! ran, so `install`'s marker path can read them with `git show` and run
//! the same record merge and escalated re-solve `viv lock merge` would
//! have, then wire the clone so it doesn't happen a second time.

use std::path::Path;

use anyhow::{Context, Result};

use crate::lock_merge::{self, Scope};

/// The two `.gitattributes` lines that route `composer.lock`/`viv.lock`
/// merges through `viv lock merge` (`docs/research.md` chapter 1); `viv
/// init` (#298) and the git-index-stage fallback (#299) both write these,
/// never a bespoke line of their own.
pub const ATTRIBUTE_LINES: [&str; 2] = ["composer.lock merge=viv", "viv.lock merge=viv"];

/// #298: on every `install`, if this clone's `.gitattributes` names
/// `merge=viv` but its own `.git/config` has no `merge.viv.driver` yet, set
/// one. Convenience only — a fresh clone's first merge already goes
/// through viv without the developer running `git config` by hand — so any
/// failure (no `.gitattributes`, no git repository, git missing) is
/// silent, never an install error.
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

/// #299 step 5: once a conflicted lock has been resolved from the git
/// index stages, wires the clone so the same developer never repeats the
/// trip — a project that left markers definitionally has no `merge=viv`
/// attribute for [`wire`] to have already acted on, so this writes it
/// first (`composer.lock` always, `viv.lock` only when the project has
/// adopted it) and prints one line per file touched, then calls [`wire`]
/// to set the driver config now that the attribute is there.
pub fn wire_after_resolve(project_dir: &Path, viv_lock_present: bool) {
    let mut lines: Vec<&str> = vec![ATTRIBUTE_LINES[0]];
    if viv_lock_present {
        lines.push(ATTRIBUTE_LINES[1]);
    }
    if let Ok(added) = ensure_gitattributes(project_dir, &lines) {
        for line in added {
            stderr_line(&format!("viv: added `{line}` to .gitattributes"));
        }
    }
    wire(project_dir);
}

/// `git show :<n>:<path>` for stages 1 (base/`%O`), 2 (ours/`%A`) and 3
/// (theirs/`%B`) of `path` — the three-way merge git keeps in its index
/// while a conflicted merge is unresolved, whether or not a driver ran.
/// `None` when any stage is missing: `path` isn't mid-merge (or
/// `project_dir` isn't a git checkout at all), the caller's cue to leave
/// #274's existing message alone.
fn read_index_stages(project_dir: &Path, path: &str) -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let stage = |n: u8| -> Option<Vec<u8>> {
        let output = crate::vcs::git_command()
            .args(["show", &format!(":{n}:{path}")])
            .current_dir(project_dir)
            .output()
            .ok()?;
        output.status.success().then_some(output.stdout)
    };
    Some((stage(1)?, stage(2)?, stage(3)?))
}

/// #299: `install`'s fallback when `composer.lock` still carries conflict
/// markers and the developer has no merge driver configured. Reads the
/// three git index stages and runs the same merge and escalated re-solve
/// [`lock_merge::run`]'s own `viv lock merge` would, at its default
/// `--max-scope` (`Scope::Seeded`). `Ok(true)` once `lock_path` is
/// rewritten clean (the driver's own resolution lines already printed) and
/// `install` should re-read it; `Ok(false)` when `lock_path` isn't at a
/// conflicted merge stage (not a git checkout, or markers pasted in by
/// hand), leaving #274's message untouched. `Err` when the re-solve can't
/// settle the divergent names either: `lock_path` is left holding markers
/// for them, in #274's own shape, and the error carries #274's message
/// minus the `viv lock merge <base> <ours> <theirs>` advice — the
/// developer has no such files to run it on.
pub fn resolve_composer_lock_conflict(
    project_dir: &Path,
    lock_path: &Path,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<bool> {
    let Some((base, ours, theirs)) = read_index_stages(project_dir, "composer.lock") else {
        return Ok(false);
    };
    let (text, status) = lock_merge::merge_composer_lock_bytes(
        &base,
        &ours,
        &theirs,
        project_dir,
        false,
        None,
        cache_dir,
        offline,
        Scope::Seeded,
    )?;
    fs_err::write(lock_path, &text).with_context(|| format!("writing {}", lock_path.display()))?;
    if status == 0 {
        return Ok(true);
    }
    Err(anyhow::anyhow!(unsatisfiable_message(
        "composer.lock",
        lock_path,
        "\"name\": \"",
    )))
}

/// [`resolve_composer_lock_conflict`]'s `viv.lock` counterpart. A
/// successful re-solve also rewrites the sibling `composer.lock`, exactly
/// as `viv lock merge` does for this format (#295), so `install` must
/// re-read both.
pub fn resolve_viv_lock_conflict(
    project_dir: &Path,
    viv_lock_path: &Path,
    composer_lock_path: &Path,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<bool> {
    let Some((base, ours, theirs)) = read_index_stages(project_dir, "viv.lock") else {
        return Ok(false);
    };
    let (viv_text, composer_text, status) = lock_merge::merge_viv_lock_bytes(
        &base,
        &ours,
        &theirs,
        project_dir,
        false,
        None,
        cache_dir,
        offline,
        Scope::Seeded,
    )?;
    fs_err::write(viv_lock_path, &viv_text)
        .with_context(|| format!("writing {}", viv_lock_path.display()))?;
    if let Some(composer_text) = composer_text {
        fs_err::write(composer_lock_path, composer_text)
            .with_context(|| format!("writing {}", composer_lock_path.display()))?;
    }
    if status == 0 {
        return Ok(true);
    }
    Err(anyhow::anyhow!(unsatisfiable_message(
        "viv.lock",
        viv_lock_path,
        "name = \"",
    )))
}

/// #274's marker message, minus its `viv lock merge <base> <ours>
/// <theirs>` advice (#299): falls back to a plain notice on the
/// (unexpected) case that a file this function just wrote markers into
/// doesn't scan as having any.
fn unsatisfiable_message(label: &str, path: &Path, name_prefix: &str) -> String {
    crate::install::marker_conflict_message(label, path, name_prefix, false)
        .unwrap_or_else(|| format!("{label} has unresolved merge conflict markers"))
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr`
/// lint (`lock_merge.rs`'s/`update.rs`'s own `warn_out` does the same).
fn stderr_line(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}
