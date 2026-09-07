//! Composer script (event) dispatch: a port of `Composer\EventDispatcher`
//! for the four events `install`/`dump-autoload` care about
//! (`pre-install-cmd`, `pre-autoload-dump`, `post-autoload-dump`,
//! `post-install-cmd`; see `Composer\Script\ScriptEvents`). Only the root
//! package's `scripts` section is ever read — Composer never runs a
//! dependency's own scripts.
//!
//! What isn't ported: a `Class::method` static PHP callback needs
//! Composer's own PHP runtime bootstrapped (autoloader, `Event` object)
//! to call, which vivace has no way to do, so it's skipped with a warning
//! instead of run. A bound Symfony `Command` class listener isn't handled
//! either — narrower still, and not asked for.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// `@composer <subcommand>` names vivace actually implements; anything else
/// is a clear error naming the command, rather than silently doing nothing.
const COMPOSER_SUBCOMMANDS: &[&str] = &["install", "dump-autoload", "normalize"];

/// Backstop against a `@script` chain that never repeats an event name but
/// still never terminates. Composer instead only detects an exact repeat on
/// its own call stack (mirrored by `active` below), which catches the
/// common case; this cap only guards the uncommon one.
const MAX_DEPTH: usize = 64;

/// One `install`/`dump-autoload` run's script state, constructed once and
/// threaded through every `dispatch` call in the run: a `@putenv` in an
/// earlier event must still be visible in a later one, the same way
/// Composer's `putenv()` mutates the whole process's environment for the
/// rest of the run.
pub struct Runner {
    /// The root `composer.json`'s `scripts` object, or `Value::Null` when
    /// absent — re-read as a bare `Value` here because `scripts` has no
    /// place in `crate::lock::Root`, which only models what the rest of
    /// vivace needs.
    scripts: Value,
    project_dir: PathBuf,
    bin_dir: PathBuf,
    dev: bool,
    /// `false` when `--no-scripts` was passed, or `scripts` is absent or
    /// empty: `dispatch` then does no work at all, not even a listener
    /// lookup, so a project without scripts pays nothing for this module.
    enabled: bool,
    env: HashMap<String, Option<String>>,
    active: Vec<String>,
    /// Set for the duration of a [`Runner::run_named`] call (`viv run`'s
    /// `run-script <script> -- <args>`), appended (shell-escaped) to every
    /// shell command this invocation runs, including a `@chained` one:
    /// Composer's own `Event` carries `getArguments()` through recursive
    /// dispatch the same way. Empty for every event `install`/`dump-autoload`
    /// dispatch on their own, which never pass extra arguments.
    pass_args: Vec<String>,
}

impl Runner {
    /// `root`: the root `composer.json`, freshly parsed. `bin_dir`:
    /// `config.bin-dir`, relative to `project_dir`.
    pub fn new(
        root: &Value,
        project_dir: &Path,
        bin_dir: &str,
        dev: bool,
        no_scripts: bool,
    ) -> Self {
        let scripts = root.get("scripts").cloned().unwrap_or(Value::Null);
        let enabled = !no_scripts && scripts.as_object().is_some_and(|map| !map.is_empty());
        Runner {
            scripts,
            project_dir: project_dir.to_path_buf(),
            bin_dir: project_dir.join(bin_dir),
            dev,
            enabled,
            env: HashMap::new(),
            active: Vec::new(),
            pass_args: Vec::new(),
        }
    }

    /// Whether `dispatch` does anything at all: `false` under `--no-scripts`
    /// or an absent/empty root `scripts`, the two cases where a caller's own
    /// no-op fast path (skipping the whole run, not just script dispatch)
    /// stays safe to take.
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Run every listener bound to `event`, in declaration order.
    pub fn dispatch(&mut self, event: &str) -> Result<()> {
        if !self.enabled {
            return Ok(());
        }
        self.run_event(event)
    }

    /// `viv run <script> [args]`: Composer's `run-script <script> -- <args>`.
    /// Unlike `dispatch`, `script` need not be one of the four events
    /// `install`/`dump-autoload` auto-dispatch — any key under the root
    /// `scripts` object qualifies, matching Composer's own `run-script`
    /// command. `args` are appended to every shell command this script's
    /// chain runs.
    pub fn run_named(&mut self, script: &str, args: &[String]) -> Result<()> {
        if self.listeners(script).is_none() {
            bail!("Script \"{script}\" is not defined in this package");
        }
        self.pass_args = args.to_vec();
        let result = self.run_event(script);
        self.pass_args.clear();
        result
    }

    fn run_event(&mut self, event: &str) -> Result<()> {
        let Some(listeners) = self.listeners(event) else {
            return Ok(());
        };
        if self.active.iter().any(|e| e == event) {
            bail!("Circular call to script handler '{event}' detected");
        }
        if self.active.len() >= MAX_DEPTH {
            bail!("script event '{event}' recursed past depth {MAX_DEPTH}, probably a cycle");
        }
        self.active.push(event.to_string());
        let result = (|| {
            for listener in &listeners {
                self.run_listener(event, listener)?;
            }
            Ok(())
        })();
        self.active.pop();
        result
    }

    /// A single listener is a bare string; more than one is an array in
    /// declaration order. Anything else in the `scripts` map (a malformed
    /// entry) has no listeners.
    fn listeners(&self, event: &str) -> Option<Vec<String>> {
        match self.scripts.get(event)? {
            Value::String(single) => Some(vec![single.clone()]),
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect(),
            ),
            _ => None,
        }
    }

    fn run_listener(&mut self, event: &str, listener: &str) -> Result<()> {
        if let Some(rest) = listener.strip_prefix("@putenv ") {
            match rest.split_once('=') {
                Some((key, value)) => {
                    self.env.insert(key.to_string(), Some(value.to_string()));
                }
                None => {
                    self.env.insert(rest.to_string(), None);
                }
            }
            return Ok(());
        }
        if let Some(rest) = listener.strip_prefix("@php ") {
            return self.run_shell(event, listener, &format!("php {rest}"));
        }
        if let Some(rest) = listener.strip_prefix("@composer ") {
            return self.run_composer(event, listener, rest);
        }
        if let Some(other_script) = listener.strip_prefix('@') {
            return self.run_event(other_script);
        }
        // Composer's `isPhpScript`: no space, and a `::` static call.
        if !listener.contains(' ') && listener.contains("::") {
            tracing::warn!(
                "{event}: skipping `{listener}`, a static PHP callback vivace has no Composer \
                 runtime to bootstrap and call"
            );
            return Ok(());
        }
        self.run_shell(event, listener, listener)
    }

    /// `@composer <subcommand> ...`: re-exec vivace's own binary instead of
    /// resolving Composer's PHP entrypoint, for the subcommands vivace
    /// implements.
    fn run_composer(&self, event: &str, listener: &str, rest: &str) -> Result<()> {
        let subcommand = rest
            .split_whitespace()
            .next()
            .with_context(|| format!("{event}: `{listener}` names no composer subcommand"))?;
        if !COMPOSER_SUBCOMMANDS.contains(&subcommand) {
            bail!(
                "{event}: `{listener}` calls `composer {subcommand}`, which viv does not \
                 implement"
            );
        }
        let exe = std::env::current_exe().context("locating vivace's own binary for @composer")?;
        let mut command = Command::new(exe);
        command
            .args(rest.split_whitespace())
            .args(&self.pass_args)
            .current_dir(&self.project_dir);
        self.apply_env(&mut command);
        let status = command
            .status()
            .with_context(|| format!("{event}: running `{listener}`"))?;
        Self::check_status(event, listener, status)
    }

    /// Everything Composer runs through its `ProcessExecutor`: `@php`
    /// listeners (already rewritten to `php <args>`) and every plain shell
    /// command, both via `sh -c`, matching Composer's own shell-backed
    /// execution (no Windows `cmd.exe` path — vivace is Linux-only).
    fn run_shell(&self, event: &str, listener: &str, command_line: &str) -> Result<()> {
        let mut line = command_line.to_string();
        for arg in &self.pass_args {
            line.push(' ');
            line.push_str(&Self::shell_escape(arg));
        }
        let mut command = Command::new("sh");
        command.arg("-c").arg(&line).current_dir(&self.project_dir);
        self.apply_env(&mut command);
        let status = command
            .status()
            .with_context(|| format!("{event}: running `{listener}`"))?;
        Self::check_status(event, listener, status)
    }

    /// `ProcessExecutor::escapeArgument` on non-Windows: single-quote,
    /// doubling any embedded quote. Duplicated from `src/bin.rs`'s own copy
    /// (private there, and small enough not to widen for one more caller).
    fn shell_escape(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }

    fn check_status(event: &str, listener: &str, status: ExitStatus) -> Result<()> {
        if status.success() {
            return Ok(());
        }
        let code = status.code().map_or_else(
            || "no exit code (killed by a signal)".to_string(),
            |c| format!("exit code {c}"),
        );
        bail!("{event}: `{listener}` exited with {code}");
    }

    /// `COMPOSER_DEV_MODE`, `bin_dir` prepended to `PATH` when it exists
    /// (Composer's `ensureBinDirIsInPath`), then this run's `@putenv`
    /// overlay layered on top so a script's own `@putenv` wins. Everything
    /// else is `Command`'s default: inherited as-is, which already covers
    /// `COMPOSER` (only ever set by the user).
    fn apply_env(&self, command: &mut Command) {
        command.env("COMPOSER_DEV_MODE", if self.dev { "1" } else { "0" });
        if self.bin_dir.is_dir() {
            let bin_dir = self.bin_dir.display().to_string();
            let path = std::env::var("PATH").unwrap_or_default();
            if path.split(':').next() != Some(bin_dir.as_str()) {
                command.env("PATH", format!("{bin_dir}:{path}"));
            }
        }
        for (key, value) in &self.env {
            match value {
                Some(value) => {
                    command.env(key, value);
                }
                None => {
                    command.env_remove(key);
                }
            }
        }
    }
}
