//! Shared CLI test harness, uv's `TestContext` / `uv_snapshot!` pattern: an
//! isolated project + cache dir per test, and filters that turn temp paths
//! and timings into stable placeholders before snapshotting.
#![allow(dead_code, reason = "not every test binary uses every helper here")]

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;
use vivace::repository::Transport;

pub(crate) const FIXED_LAST_MODIFIED: &str = "Mon, 01 Jan 2024 00:00:00 GMT";

pub(crate) fn fixtures_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/packagist/repo.packagist.org")
}

/// Reads `lock`'s `packages`/`packages-dev` entries into a lowercased
/// name -> entry map, the shape `solve_partial_update` wants for its
/// already-locked packages, shared by `tests/require.rs`/`tests/update.rs`.
pub(crate) fn locked_by_name(lock: &Path) -> HashMap<String, serde_json::Value> {
    let lock: serde_json::Value = serde_json::from_slice(&fs_err::read(lock).unwrap()).unwrap();
    let mut by_name = HashMap::new();
    for key in ["packages", "packages-dev"] {
        for entry in lock[key].as_array().unwrap() {
            by_name.insert(
                entry["name"].as_str().unwrap().to_ascii_lowercase(),
                entry.clone(),
            );
        }
    }
    by_name
}

/// Fixture-replaying transport shared by `tests/require.rs`,
/// `tests/update.rs` and `tests/solver.rs`.
pub(crate) struct FixtureTransport {
    pub(crate) root: PathBuf,
}

impl Transport for &FixtureTransport {
    #[allow(clippy::unused_async_trait_impl)]
    async fn get(
        &self,
        url: &reqwest::Url,
        if_modified_since: Option<&str>,
    ) -> anyhow::Result<vivace::fetch::Conditional> {
        if if_modified_since == Some(FIXED_LAST_MODIFIED) {
            return Ok(vivace::fetch::Conditional::NotModified);
        }
        let path = self.root.join(url.path().trim_start_matches('/'));
        match fs_err::read(&path) {
            Ok(body) => Ok(vivace::fetch::Conditional::Fresh {
                body,
                last_modified: Some(FIXED_LAST_MODIFIED.to_string()),
            }),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                Ok(vivace::fetch::Conditional::NotFound)
            }
            Err(err) => Err(err.into()),
        }
    }
}

pub(crate) struct TestContext {
    pub(crate) project: TempDir,
    pub(crate) cache: TempDir,
}

impl TestContext {
    pub(crate) fn new() -> Self {
        TestContext {
            project: tempfile::tempdir().expect("creating a temp project dir"),
            cache: tempfile::tempdir().expect("creating a temp cache dir"),
        }
    }

    /// `viv` with `--cache-dir` pointed at this context's isolated cache and
    /// the working directory set to the project, so tests never touch a
    /// developer's real cache or shell directory.
    pub(crate) fn viv(&self) -> Command {
        let mut cmd = Command::new(cargo_bin("viv"));
        cmd.current_dir(self.project.path())
            .arg("--cache-dir")
            .arg(self.cache.path());
        cmd
    }

    /// `(pattern, replacement)` pairs for [`crate::viv_snapshot`]: this
    /// context's two temp paths, and any `12.34s` timing.
    ///
    /// Also filters each path's canonicalised form (e.g. macOS's
    /// `/private/var/...` for a raw `/var/...` `TempDir`), so a `viv` that
    /// canonicalises before printing still matches [PROJECT]/[CACHE].
    pub(crate) fn filters(&self) -> Vec<(String, String)> {
        let mut filters = vec![];
        filters.extend(path_filter(self.project.path(), "[PROJECT]"));
        filters.extend(path_filter(self.cache.path(), "[CACHE]"));
        filters.push((r"\d+\.\d+s".to_string(), "[TIME]".to_string()));
        // Longest pattern first: a canonicalised path (e.g. macOS's
        // `/private/var/...`) contains the raw path (`/var/...`) as a
        // substring, so applying the raw filter first would match inside
        // the canonical one and leave a stray `/private` prefix behind.
        filters.sort_by_key(|(pattern, _)| std::cmp::Reverse(pattern.len()));
        filters
    }
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}

/// `(pattern, replacement)` pairs for `path`, plus its canonicalised form
/// when that differs (a symlinked ancestor, e.g. macOS's `/private/var`
/// alias for `/var`).
fn path_filter(path: &std::path::Path, replacement: &str) -> Vec<(String, String)> {
    let mut filters = vec![(
        regex::escape(&path.display().to_string()),
        replacement.to_string(),
    )];
    if let Ok(canonical) = std::fs::canonicalize(path)
        && canonical != path
    {
        filters.push((
            regex::escape(&canonical.display().to_string()),
            replacement.to_string(),
        ));
    }
    filters
}

/// Run a built `viv` command and snapshot `success`/`exit_code`/`stdout`/
/// `stderr`, after applying `$ctx`'s path and timing filters.
#[macro_export]
macro_rules! viv_snapshot {
    ($ctx:expr, $cmd:expr) => {{
        let mut settings = insta::Settings::clone_current();
        for (pattern, replacement) in $ctx.filters() {
            settings.add_filter(&pattern, replacement);
        }
        settings.bind(|| {
            let mut cmd = $cmd;
            let output = cmd.output().expect("failed to run viv");
            let snapshot = format!(
                "success: {}\nexit_code: {}\n----- stdout -----\n{}\n----- stderr -----\n{}",
                output.status.success(),
                output.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
            insta::assert_snapshot!(snapshot);
        });
    }};
}

#[cfg(test)]
mod tests {
    use super::path_filter;

    /// Regression for #72: a symlinked ancestor (like macOS's real
    /// `/private/var` under `/var`) must yield a filter for both the raw
    /// and canonicalised path.
    #[test]
    fn path_filter_covers_a_symlinked_ancestor() {
        let real_root = tempfile::tempdir().expect("creating a real dir");
        let link_root = tempfile::tempdir().expect("creating a dir to hold the symlink");
        let link = link_root.path().join("alias");
        #[cfg(unix)]
        std::os::unix::fs::symlink(real_root.path(), &link).expect("creating a symlink");
        #[cfg(windows)]
        std::os::windows::fs::symlink_dir(real_root.path(), &link).expect("creating a symlink");

        let filters = path_filter(&link, "[PROJECT]");

        assert_eq!(
            filters.len(),
            2,
            "expected a filter for the raw and canonical path"
        );
        assert!(
            filters[0]
                .0
                .contains(&regex::escape(&link.display().to_string()))
        );
        let canonical = std::fs::canonicalize(&link).expect("canonicalizing the symlink");
        assert!(
            filters[1]
                .0
                .contains(&regex::escape(&canonical.display().to_string()))
        );

        // Applying the filters longest-first (as `TestContext::filters`
        // does) to the canonical path must consume it entirely, leaving no
        // `/private`-style prefix behind.
        let mut sorted = filters;
        sorted.sort_by_key(|(pattern, _)| std::cmp::Reverse(pattern.len()));
        let mut text = format!("{}/x", canonical.display());
        for (pattern, replacement) in &sorted {
            text = regex::Regex::new(pattern)
                .expect("valid filter regex")
                .replace_all(&text, replacement.as_str())
                .into_owned();
        }
        assert_eq!(text, "[PROJECT]/x");
    }
}
