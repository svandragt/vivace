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
use crate::php;
use crate::scripts;
use crate::store::{self, Store, hex};
use crate::update::{self, UpdateArgs};

/// `viv x` flags.
#[derive(Args, Debug, Clone)]
pub struct XArgs {
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
    /// `vendor/package` or `vendor/package:constraint`, followed by
    /// arguments passed through to the executed bin; omit with `--list` or
    /// `--uninstall`. viv's own flags (`--bin`, `--refresh`, `-d`,
    /// `--cache-dir`, `-v`, `--offline`) only apply before the package name
    /// — `viv x phpunit/phpunit --filter Foo` passes `--filter Foo` to
    /// `phpunit`, not to `viv`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
}

/// `viv run` flags.
#[derive(Args, Debug, Clone)]
pub struct RunArgs {
    /// List every script declared in `scripts`.
    #[arg(long)]
    pub list: bool,
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// A script name from the root `composer.json`'s `scripts` section, a
    /// `vendor/bin` binary, or a PATH command, followed by its own
    /// arguments; omit with `--list`. viv's own flags (`-d`, `--cache-dir`,
    /// `-v`, `--offline`) only apply before this positional — after it,
    /// `viv run -d ../app phpunit --filter Foo` passes `--filter Foo` to
    /// phpunit untouched.
    #[arg(
        required_unless_present = "list",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    pub command: Vec<String>,
}

/// `viv exec` flags.
#[derive(Args, Debug, Clone)]
pub struct ExecArgs {
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// `vendor/bin/<name>` to exec, followed by its own arguments. viv's
    /// own flags (`-d`, `--cache-dir`, `-v`, `--offline`) only apply before
    /// this positional.
    #[arg(required = true, trailing_var_arg = true, allow_hyphen_values = true)]
    pub command: Vec<String>,
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
        .command
        .first()
        .map(String::as_str)
        .context("viv x needs a package, e.g. `viv x phpunit/phpunit`")?;
    let pass_args = pass_through(&args.command);
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
            lock: None,
            no_dev: false,
            prefer_lowest: false,
            prefer_stable: false,
            dry_run: false,
            no_normalize: false,
            bump_after_update: None,
            project_dir: env_dir.clone(),
            no_scripts: false,
            no_plugins: false,
            // `x` runs its own `install::run` right below with its own
            // `InstallArgs`; without this, `update::run`'s own new chaining
            // (#104) would install twice.
            no_install: true,
            ignore_platform_reqs: false,
            ignore_platform_req: Vec::new(),
            no_blocking: false,
            no_security_blocking: false,
            metadata_ttl: None,
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
            ignore_platform_reqs: false,
            ignore_platform_req: Vec::new(),
            no_scripts: false,
            // `install` never touches `composer.json` any more (#95); this
            // flag is a no-op kept only for old invocations, so leave it
            // unset rather than trip its deprecation warning on every run.
            no_normalize: false,
            no_plugins: false,
            no_progress: false,
            no_interaction: false,
            prefer_dist: false,
            no_suggest: false,
        };
        install::run(&install_args, Some(&cache_dir), offline)
            .with_context(|| format!("installing {spec}"))?;
        fs_err::write(&marker, b"")?;
    }

    let target = resolve_bin(&env_dir, &name, short_name, args.bin.as_deref())?;
    let error = std::process::Command::new(&target).args(pass_args).exec();
    Err(anyhow::Error::from(error).context(format!("executing {}", target.display())))
}

pub fn run_run(args: &RunArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let composer_json_value: Value =
        serde_json::from_slice(&composer_json).context("parsing composer.json")?;

    if args.list {
        list_scripts(&composer_json_value);
        return Ok(());
    }
    // clap's `required_unless_present = "list"` on `command` guarantees this.
    let script = args
        .command
        .first()
        .expect("command required without --list");
    let script_args = pass_through(&args.command);

    let root = lock::parse_root(&composer_json).context("parsing composer.json")?;
    let bin_dir = project_dir.join(root.config.bin_dir());
    let php_dir = php::project_php_dir(&project_dir, cache_dir, offline)?;

    let mut runner = scripts::Runner::new(
        &composer_json_value,
        &project_dir,
        &root.config.bin_dir(),
        true,
        false,
    )
    .with_php_dir(php_dir.clone());
    if runner.has_script(script) {
        return runner.run_named(script, script_args);
    }

    // Not a declared script (#338): fall back to `vendor/bin/<script>`,
    // then to `<script>` resolved on the same composed PATH a matched bin
    // would run with — Composer's `run-script` has no such fallback, but
    // `viv run` is meant as one entry point for "run this", scripts, bins
    // and PATH tools alike.
    let bin_path = bin_dir.join(script);
    if bin_path.is_file() {
        return exec_with_path(&bin_path, script_args, php_dir.as_deref(), &bin_dir);
    }
    let composed_path = php::compose_path(php_dir.as_deref(), &bin_dir);
    if let Some(found) = find_on_path(script, &composed_path) {
        return exec_with_path(&found, script_args, php_dir.as_deref(), &bin_dir);
    }

    bail!("\"{script}\": not a script in composer.json, not in vendor/bin, and not on PATH");
}

pub fn run_exec(args: &ExecArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path).context("reading composer.json")?;
    let root = lock::parse_root(&composer_json).context("parsing composer.json")?;
    let bin_dir = project_dir.join(root.config.bin_dir());
    // clap's `required = true` on `command` guarantees a first element.
    let bin = args.command.first().expect("command required");
    let target = bin_dir.join(bin);
    if !target.is_file() {
        bail!(
            "{}: not found; run `viv install` first, or check the bin name",
            target.display()
        );
    }

    let php_dir = php::project_php_dir(&project_dir, cache_dir, offline)?;
    exec_with_path(
        &target,
        pass_through(&args.command),
        php_dir.as_deref(),
        &bin_dir,
    )
}

/// The arguments after the tool's name, with one leading `--` dropped: the
/// `composer run-script test -- --filter X` idiom, which Symfony strips and
/// which the single trailing positional (#338) would otherwise hand to the
/// tool as a literal.
fn pass_through(command: &[String]) -> &[String] {
    match command.get(1..) {
        Some([first, rest @ ..]) if first == "--" => rest,
        Some(rest) => rest,
        None => &[],
    }
}

/// `exec()`'s a `target` binary with `args`, its `PATH` composed the same
/// way for every `viv run`/`viv exec` execution (`php::compose_path`) — the
/// one place that never returns on success, shared so a resolved
/// `vendor/bin`/PATH target in `run_run` runs identically to `run_exec`'s.
fn exec_with_path(
    target: &Path,
    args: &[String],
    php_dir: Option<&Path>,
    bin_dir: &Path,
) -> Result<()> {
    let new_path = php::compose_path(php_dir, bin_dir);
    let error = std::process::Command::new(target)
        .args(args)
        .env("PATH", new_path)
        .exec();
    Err(anyhow::Error::from(error).context(format!("executing {}", target.display())))
}

/// `name` resolved by scanning `composed_path` (`:`-separated, like `PATH`)
/// for an executable file — std has no `which`. Mirrors a shell's own PATH
/// search (first match wins), restricted to regular files with an execute
/// bit set so a directory or a non-executable file of the same name is
/// skipped rather than handed to `exec()` to fail on.
fn find_on_path(name: &str, composed_path: &str) -> Option<PathBuf> {
    std::env::split_paths(composed_path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    fs_err::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
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

/// `--list`'s own body (#235): says so explicitly when there are no scripts
/// declared, rather than printing nothing, so an empty project can't be
/// mistaken for a command that ran and found nothing to report.
fn list_scripts(composer_json: &Value) {
    let scripts = composer_json.get("scripts").and_then(Value::as_object);
    match scripts {
        Some(scripts) if !scripts.is_empty() => {
            for name in scripts.keys() {
                out(name);
            }
        }
        _ => out("No scripts declared in composer.json"),
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
