//! Shared CLI test harness, uv's `TestContext` / `uv_snapshot!` pattern: an
//! isolated project + cache dir per test, and filters that turn temp paths
//! and timings into stable placeholders before snapshotting.
#![allow(dead_code, reason = "not every test binary uses every helper here")]

use assert_cmd::Command;
use assert_cmd::cargo::cargo_bin;
use tempfile::TempDir;

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
    pub(crate) fn filters(&self) -> Vec<(String, String)> {
        vec![
            (
                regex::escape(&self.project.path().display().to_string()),
                "[PROJECT]".to_string(),
            ),
            (
                regex::escape(&self.cache.path().display().to_string()),
                "[CACHE]".to_string(),
            ),
            (r"\d+\.\d+s".to_string(), "[TIME]".to_string()),
        ]
    }
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
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
