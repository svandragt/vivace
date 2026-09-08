//! `viv diagnose`: an environment and configuration report to paste into a
//! bug report, the way `composer diagnose` checks connectivity, auth, cache
//! and PHP. This is not byte-compatible with Composer's own `diagnose` and
//! does not claim to be — it's vivace's own read-only survey of the things
//! that make `viv install` behave differently machine to machine. No
//! network calls; nothing on disk is written or created.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use clap::Args;
use serde_json::Value;

use crate::auth::composer_home;
use crate::lock::{self, Lock, Root};
use crate::plugins;
use crate::solver::platform::platform_packages;
use crate::store::Store;

#[derive(Args)]
pub struct DiagnoseArgs {
    /// Project directory holding `composer.json`/`composer.lock`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Print every registered adapter's plugin name(s) and the upstream
    /// version it was ported from, one line per name, in registration order,
    /// and nothing else. Feeds #127 part 3's drift workflow
    /// (`.github/workflows/adapter-drift.yml`), not for interactive use.
    #[arg(long, hide = true)]
    pub adapters: bool,
}

pub fn run(args: &DiagnoseArgs, cache_dir: Option<&Path>) -> Result<()> {
    if args.adapters {
        diagnose_adapters();
        return Ok(());
    }

    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;

    out(&format!(
        "viv: {} ({}-{})",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        std::env::consts::OS
    ));
    diagnose_cache(cache_dir)?;
    diagnose_composer_home_and_auth(&project_dir);
    diagnose_tools();
    diagnose_project(&project_dir)?;
    Ok(())
}

fn diagnose_cache(cache_dir: Option<&Path>) -> Result<()> {
    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => match crate::update::default_cache_dir() {
            Ok(dir) => dir,
            Err(err) => {
                out(&format!("cache-dir: not resolved ({err})"));
                return Ok(());
            }
        },
    };
    out(&format!("cache-dir: {}", cache_dir.display()));
    if !cache_dir.is_dir() {
        // `Store::open` would create the directory; a report must not.
        out("cache-size: not created yet");
        return Ok(());
    }
    let size = Store::open(&cache_dir)?.size()?;
    out(&format!(
        "cache-size: {} bytes, {} archives, {} pointers",
        size.archive_bytes, size.archives, size.pointers
    ));
    Ok(())
}

fn diagnose_composer_home_and_auth(project_dir: &Path) {
    let home = composer_home();
    out(&format!(
        "composer-home: {}",
        home.as_deref()
            .map_or("not resolved".to_string(), |p| p.display().to_string())
    ));

    if let Some(home) = &home {
        report_auth_source("auth.json (composer home)", &home.join("auth.json"));
    }
    report_auth_source("auth.json (project)", &project_dir.join("auth.json"));
    if let Ok(raw) = std::env::var("COMPOSER_AUTH") {
        let hosts = serde_json::from_str(&raw)
            .ok()
            .map(|value| auth_hosts(&value))
            .unwrap_or_default();
        out(&format!("COMPOSER_AUTH: {}", format_hosts(&hosts)));
    }
}

/// Print one `auth` line for `path` if it exists, listing the host names it
/// holds under any credential category — never a credential value.
fn report_auth_source(label: &str, path: &Path) {
    let Ok(content) = fs_err::read_to_string(path) else {
        return;
    };
    let hosts = serde_json::from_str(&content)
        .ok()
        .map(|value| auth_hosts(&value))
        .unwrap_or_default();
    out(&format!("{label}: {}", format_hosts(&hosts)));
}

fn format_hosts(hosts: &[String]) -> String {
    if hosts.is_empty() {
        "(none)".to_string()
    } else {
        hosts.join(", ")
    }
}

/// Every host name keyed under `auth.json`'s credential categories
/// (`github-oauth`, `http-basic`, `bearer`, `gitlab-oauth`, `gitlab-token`,
/// `bitbucket-oauth`, `custom-headers`), sorted and deduplicated. Only the
/// keys are read; values (tokens, passwords) never reach the report.
fn auth_hosts(value: &Value) -> Vec<String> {
    const CATEGORIES: &[&str] = &[
        "github-oauth",
        "http-basic",
        "bearer",
        "gitlab-oauth",
        "gitlab-token",
        "bitbucket-oauth",
        "custom-headers",
    ];
    let mut hosts: Vec<String> = CATEGORIES
        .iter()
        .filter_map(|category| value.get(category))
        .filter_map(Value::as_object)
        .flat_map(|map| map.keys().cloned())
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

fn diagnose_tools() {
    out(&format!(
        "php: {}",
        tool_version("php", &["-r", "echo PHP_VERSION;"])
    ));
    out(&format!("git: {}", tool_version("git", &["--version"])));
    out(&format!(
        "composer: {}",
        tool_version("composer", &["--version", "--no-ansi"])
    ));
}

/// Runs `command args` and returns its trimmed stdout, or `"not found"` when
/// the command can't be run or exits non-zero (no PHP/git/Composer on
/// `PATH`, same as the rest of vivace's own detection probes in
/// `solver::pool_builder`/`tool.rs`).
fn tool_version(command: &str, args: &[&str]) -> String {
    Command::new(command)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "not found".to_string())
}

fn diagnose_project(project_dir: &Path) -> Result<()> {
    let composer_json_path = project_dir.join("composer.json");
    let lock_path = project_dir.join("composer.lock");
    if !composer_json_path.is_file() || !lock_path.is_file() {
        out("project: none");
        return Ok(());
    }
    out(&format!("project: {}", project_dir.display()));

    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root = lock::parse_root(&composer_json).context("parsing composer.json")?;
    let root_json: Value =
        serde_json::from_slice(&composer_json).context("parsing composer.json")?;
    let lock = lock::read_lock(&lock_path)?;

    diagnose_platform(&root_json)?;
    diagnose_plugins(&lock, &root);
    Ok(())
}

fn diagnose_platform(root_json: &Value) -> Result<()> {
    let overrides = root_json
        .pointer("/config/platform")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    for package in platform_packages(&overrides)? {
        out(&format!(
            "platform: {} {}",
            package.name, package.pretty_version
        ));
    }
    Ok(())
}

/// The decision `viv install` would make for each `composer-plugin` in the
/// lock, resolved one package at a time via [`plugins::resolve`] (the same
/// entry point `install`/`update` use) so this never re-derives the
/// native-adapter/inert/refuse rules itself.
fn diagnose_plugins(lock: &Lock, root: &Root) {
    for package in &lock.packages {
        if package.r#type != "composer-plugin" {
            continue;
        }
        let decision = if root.config.allow_plugins.is_enabled(&package.name) {
            let scratch = Lock {
                content_hash: None,
                packages: vec![package.clone()],
                aliases: Vec::new(),
            };
            match plugins::resolve(&scratch, root, false) {
                Ok(_) => "handled (native adapter or no-op)".to_string(),
                Err(err) => format!("refused: {err:#}"),
            }
        } else {
            "disabled (config.allow-plugins)".to_string()
        };
        out(&format!("plugins: {}: {decision}", package.name));
    }
}

/// `viv diagnose --adapters`: `<plugin name>\t<upstream version>`, one line
/// per name (the `WordPress` core installer pair has two), in
/// `NATIVE_ADAPTERS` registration order, nothing else on stdout.
fn diagnose_adapters() {
    for adapter in plugins::all_adapters() {
        for name in adapter.plugin_names() {
            out(&format!("{name}\t{}", adapter.upstream_version()));
        }
    }
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}
