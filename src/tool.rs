//! `viv x`/`viv run`/`viv exec` (#85): npx-style one-off tool execution, a
//! script-name entry point into `scripts::Runner` beyond the four events
//! `install`/`dump-autoload` auto-dispatch, and a bare `vendor/bin` exec —
//! Composer's `run-script` and `exec` commands.
//!
//! `viv x` builds a synthetic single-package root `composer.json` under a
//! per-tool cache dir, resolves and installs it with the existing
//! `update::run`/`install::run` entry points (no new resolve/install path),
//! and execs the requested bin once that's done. A completion marker next to
//! the installed `vendor/` lets a second run of the same package/constraint
//! skip straight to the exec: no re-resolve, no re-install, no network.

use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::install::{self, InstallArgs};
use crate::link::LinkMode;
use crate::lock;
use crate::scripts;
use crate::store::{self, Store, hex};
use crate::update::{self, UpdateArgs};

/// `viv x` flags.
#[derive(Args, Debug, Clone)]
pub struct XArgs {
    /// `vendor/package` or `vendor/package:constraint`; omit with `--list`
    /// or `--uninstall`.
    pub package: Option<String>,
    /// Which bin to exec, when the package ships more than one.
    #[arg(long)]
    pub bin: Option<String>,
    /// Re-resolve and reinstall even if this constraint is already cached.
    #[arg(long)]
    pub refresh: bool,
    /// List every package cached by a previous `viv x` run.
    #[arg(long)]
    pub list: bool,
    /// Remove a cached tool env (every constraint cached for it).
    #[arg(long)]
    pub uninstall: Option<String>,
    /// Arguments passed through to the executed bin.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// `viv run` flags.
#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    /// Script name from the root `composer.json`'s `scripts` section; omit
    /// with `--list`.
    pub script: Option<String>,
    /// List every script declared in `scripts`, printing nothing else.
    #[arg(long)]
    pub list: bool,
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Arguments appended to the script's own command line.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// `viv exec` flags.
#[derive(Args, Debug, Clone)]
pub struct ExecArgs {
    /// `vendor/bin/<name>` to exec.
    pub bin: String,
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Arguments passed through to the executed bin.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub args: Vec<String>,
}

/// The marker written next to a tool env's `vendor/` once install finishes: a
/// sibling of the doc comment's own name in `store.rs`'s archive bucket
/// (`.ok`, next to not inside the dir), same shape here — a stat away from
/// deciding "already installed" without re-reading `vendor/composer/*`.
const TOOL_COMPLETE_MARKER: &str = ".viv-tool-complete";

pub fn run_x(args: &XArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => update::default_cache_dir()?,
    };

    if args.list {
        return list_tools(&cache_dir);
    }
    if let Some(spec) = &args.uninstall {
        return uninstall_tool(&cache_dir, spec);
    }

    let spec = args
        .package
        .as_deref()
        .context("viv x needs a package, e.g. `viv x phpunit/phpunit`")?;
    let (name, constraint) = split_spec(spec);
    let name = name.to_ascii_lowercase();
    let constraint = constraint.unwrap_or("*");
    let (vendor, short_name) = name
        .split_once('/')
        .with_context(|| format!("{name}: expected vendor/package"))?;

    let php_version = detect_php_version();
    let key_input = format!("{constraint}|php={php_version}");
    let key = hex(Sha256::digest(key_input.as_bytes()));

    let env_dir = {
        let store = Store::open(&cache_dir)?;
        store.tool_env_dir(vendor, short_name, &key)?
    };
    let marker = env_dir.join(TOOL_COMPLETE_MARKER);

    if args.refresh || !marker.is_file() {
        fs_err::create_dir_all(&env_dir)?;
        write_synthetic_root(&env_dir, &name, constraint)?;
        let update_args = UpdateArgs {
            packages: Vec::new(),
            with_dependencies: false,
            with_all_dependencies: false,
            minimal_changes: false,
            lock: false,
            no_dev: false,
            prefer_lowest: false,
            prefer_stable: false,
            dry_run: false,
            no_normalize: false,
            project_dir: env_dir.clone(),
            no_scripts: false,
            no_plugins: false,
            // `x` runs its own `install::run` right below with its own
            // `InstallArgs`; without this, `update::run`'s own new chaining
            // (#104) would install twice.
            no_install: true,
        };
        update::run(&update_args, Some(&cache_dir), offline)
            .with_context(|| format!("resolving {spec}"))?;
        let install_args = InstallArgs {
            no_dev: false,
            dry_run: false,
            link_mode: LinkMode::default(),
            adopt: false,
            project_dir: env_dir.clone(),
            optimize_autoloader: false,
            classmap_authoritative: false,
            apcu_autoloader: false,
            apcu_autoloader_prefix: None,
            no_scripts: false,
            // `install` never touches `composer.json` any more (#95); this
            // flag is a no-op kept only for old invocations, so leave it
            // unset rather than trip its deprecation warning on every run.
            no_normalize: false,
            no_plugins: false,
        };
        install::run(&install_args, Some(&cache_dir), offline)
            .with_context(|| format!("installing {spec}"))?;
        fs_err::write(&marker, b"")?;
    }

    let target = resolve_bin(&env_dir, &name, short_name, args.bin.as_deref())?;
    let error = std::process::Command::new(&target).args(&args.args).exec();
    Err(anyhow::Error::from(error).context(format!("executing {}", target.display())))
}

pub fn run_run(args: &RunArgs) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let composer_json_value: Value =
        serde_json::from_slice(&composer_json).context("parsing composer.json")?;

    if args.list || args.script.is_none() {
        list_scripts(&composer_json_value);
        return Ok(());
    }
    let script = args.script.as_deref().expect("checked above");

    let root = lock::parse_root(&composer_json).context("parsing composer.json")?;
    let mut runner = scripts::Runner::new(
        &composer_json_value,
        &project_dir,
        &root.config.bin_dir(),
        true,
        false,
    );
    runner.run_named(script, &args.args)
}

pub fn run_exec(args: &ExecArgs) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root = lock::parse_root(&composer_json).context("parsing composer.json")?;
    let bin_dir = project_dir.join(root.config.bin_dir());
    let target = bin_dir.join(&args.bin);
    if !target.is_file() {
        bail!(
            "{}: not found; run `viv install` first, or check the bin name",
            target.display()
        );
    }

    let path = std::env::var("PATH").unwrap_or_default();
    let new_path = format!("{}:{path}", bin_dir.display());
    let error = std::process::Command::new(&target)
        .args(&args.args)
        .env("PATH", new_path)
        .exec();
    Err(anyhow::Error::from(error).context(format!("executing {}", target.display())))
}

/// `vendor/package` or `vendor/package:constraint` (duplicated from
/// `require.rs`'s own private `split_spec`: small, and that file isn't this
/// lane's to widen).
fn split_spec(spec: &str) -> (&str, Option<&str>) {
    match spec.split_once(':') {
        Some((name, constraint)) => (name, Some(constraint)),
        None => (spec, None),
    }
}

/// The running `php` interpreter's own version, folded into a tool env's
/// cache key so a different interpreter (or none installed) never reuses
/// another one's resolve/install. `"unknown"` when `php` cannot be run at
/// all, same fallback shape as `solver::pool_builder`'s own PHP detection
/// (not reused directly: that one is private to the solver, and both copies
/// are a few lines).
fn detect_php_version() -> String {
    std::process::Command::new("php")
        .args(["-r", "echo PHP_VERSION;"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .filter(|version| !version.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn write_synthetic_root(env_dir: &Path, name: &str, constraint: &str) -> Result<()> {
    let body = serde_json::json!({ "require": { name: constraint } });
    let text = format!("{}\n", serde_json::to_string_pretty(&body)?);
    fs_err::write(env_dir.join("composer.json"), text)?;
    Ok(())
}

/// `--bin`'s default: the bin whose basename equals the package's own short
/// name, else the package's only bin, else an error listing every candidate
/// — candidates are the requested package's own `bin` entries (read from the
/// just-installed `vendor/composer/installed.json`), not every bin any
/// transitive dependency happens to ship into the same `vendor/bin`.
fn resolve_bin(
    env_dir: &Path,
    name: &str,
    short_name: &str,
    requested: Option<&str>,
) -> Result<PathBuf> {
    let installed_path = env_dir.join("vendor/composer/installed.json");
    let installed: Value = serde_json::from_slice(
        &fs_err::read(&installed_path)
            .with_context(|| format!("reading {}", installed_path.display()))?,
    )?;
    let package = installed
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
        .with_context(|| format!("{name}: not found in {}", installed_path.display()))?;
    let bins: Vec<String> = package
        .get("bin")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    if bins.is_empty() {
        bail!("{name} declares no `bin` entries to run");
    }

    let bin_dir = env_dir.join("vendor/bin");
    let chosen = if let Some(requested) = requested {
        if !bins.iter().any(|b| b == requested) {
            bail!(
                "{name}: no bin named `{requested}`; candidates: {}",
                bins.join(", ")
            );
        }
        requested.to_string()
    } else if bins.iter().any(|b| b == short_name) {
        short_name.to_string()
    } else if let [only] = bins.as_slice() {
        only.clone()
    } else {
        bail!(
            "{name} ships more than one bin ({}); pass --bin to choose",
            bins.join(", ")
        );
    };
    Ok(bin_dir.join(chosen))
}

fn list_scripts(composer_json: &Value) {
    let Some(scripts) = composer_json.get("scripts").and_then(Value::as_object) else {
        return;
    };
    for name in scripts.keys() {
        out(name);
    }
}

fn list_tools(cache_dir: &Path) -> Result<()> {
    let tools_dir = cache_dir.join(store::TOOLS_BUCKET);
    if !tools_dir.is_dir() {
        return Ok(());
    }
    for vendor_entry in fs_err::read_dir(&tools_dir)? {
        let vendor_entry = vendor_entry?;
        if !vendor_entry.file_type()?.is_dir() {
            continue;
        }
        let vendor = vendor_entry.file_name().to_string_lossy().into_owned();
        for name_entry in fs_err::read_dir(vendor_entry.path())? {
            let name_entry = name_entry?;
            if !name_entry.file_type()?.is_dir() {
                continue;
            }
            let name = name_entry.file_name().to_string_lossy().into_owned();
            let cached = fs_err::read_dir(name_entry.path())?
                .filter_map(std::result::Result::ok)
                .any(|entry| entry.path().join(TOOL_COMPLETE_MARKER).is_file());
            if cached {
                out(&format!("{vendor}/{name}"));
            }
        }
    }
    Ok(())
}

fn uninstall_tool(cache_dir: &Path, spec: &str) -> Result<()> {
    let (name, _constraint) = split_spec(spec);
    let (vendor, short_name) = name
        .split_once('/')
        .with_context(|| format!("{name}: expected vendor/package"))?;
    let dir = cache_dir
        .join(store::TOOLS_BUCKET)
        .join(vendor)
        .join(short_name);
    if !dir.is_dir() {
        bail!("{spec}: no cached tool env at {}", dir.display());
    }
    fs_err::remove_dir_all(&dir)?;
    out(&format!("Removed {spec} ({})", dir.display()));
    Ok(())
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}
