//! `viv init` (#143): write a `composer.json` for a new project and stop.
//! No interactive question flow — `composer init` already covers that; this
//! is the fastest route from an empty directory to a file `viv install`
//! accepts.
//!
//! Defaults are inferred, never asked: `name` from `git config user.name`
//! (falling back to the OS user) and the project directory, `type`
//! `project`, `license` `MIT`, `autoload.psr-4` pointing at `src/` when that
//! directory exists, `config.sort-packages` on. Every flag overrides its
//! default. The file is built as a plain [`serde_json::Map`] and handed to
//! [`crate::normalize::maybe_normalize`] for key order and formatting, then
//! checked with [`crate::validate::run`] — the same two passes `viv add`/
//! `viv rm` and `viv validate` already run, so there is exactly one place
//! that decides what "normalized" and "valid" mean. `--require`/
//! `--require-dev` reuse `crate::require::synthesize_constraint` for a
//! bare package name and `crate::require::partial_update` for the
//! resolve/lock/install chain: the same functions `viv add` calls once it
//! has edited `composer.json`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::{Map, Value, json};

use crate::normalize;
use crate::require::{self, split_spec};
use crate::solver::pool_builder::UpdateAllowMode;
use crate::validate::{self, ValidateArgs};

/// `viv init` flags.
#[derive(Args, Debug, Clone)]
pub struct InitArgs {
    /// `vendor/package`; inferred from git/the OS user and the directory
    /// name when not given.
    #[arg(long)]
    pub name: Option<String>,
    /// Free-text project description; left out of the file entirely when
    /// not given.
    #[arg(long)]
    pub description: Option<String>,
    /// Package type. Defaults to `project`.
    #[arg(long = "type")]
    pub package_type: Option<String>,
    /// SPDX licence identifier. Defaults to `MIT`.
    #[arg(long)]
    pub license: Option<String>,
    /// Directory to autoload under `psr-4`; also forces the entry even when
    /// the directory doesn't exist yet (the default only adds one when
    /// `src/` is already there).
    #[arg(long = "autoload")]
    pub autoload: Option<String>,
    /// `vendor/package[:constraint]`, repeatable; a bare name gets a
    /// synthesised constraint, same as `viv add`.
    #[arg(long = "require")]
    pub require: Vec<String>,
    /// `vendor/package[:constraint]` for `require-dev`, repeatable.
    #[arg(long = "require-dev")]
    pub require_dev: Vec<String>,
    /// Write composer.json (and, with `--require`/`--require-dev`,
    /// composer.lock) but don't install.
    #[arg(long = "no-install")]
    pub no_install: bool,
    /// Overwrite an existing composer.json.
    #[arg(long)]
    pub force: bool,
    /// Project directory to write composer.json into.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
}

pub fn run(args: &InitArgs, cache_dir: Option<&std::path::Path>, offline: bool) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let composer_json_path = project_dir.join("composer.json");
    if composer_json_path.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to overwrite",
            composer_json_path.display()
        );
    }

    let name = args
        .name
        .clone()
        .unwrap_or_else(|| default_name(&project_dir));

    let mut root = Map::new();
    root.insert("name".to_string(), Value::String(name.clone()));
    if let Some(description) = &args.description {
        root.insert(
            "description".to_string(),
            Value::String(description.clone()),
        );
    }
    root.insert(
        "type".to_string(),
        Value::String(
            args.package_type
                .clone()
                .unwrap_or_else(|| "project".into()),
        ),
    );
    root.insert(
        "license".to_string(),
        Value::String(args.license.clone().unwrap_or_else(|| "MIT".into())),
    );
    if let Some((namespace_dir, dir)) = autoload_entry(args, &project_dir, &name) {
        root.insert(
            "autoload".to_string(),
            json!({"psr-4": {format!("{namespace_dir}\\"): format!("{dir}/")}}),
        );
    }
    root.insert("config".to_string(), json!({"sort-packages": true}));

    let mut allow_list = Vec::new();
    for (key, specs) in [
        ("require", &args.require),
        ("require-dev", &args.require_dev),
    ] {
        if specs.is_empty() {
            continue;
        }
        let mut links = Map::new();
        for spec in specs {
            let (raw_name, constraint) = split_spec(spec);
            let package_name = raw_name.to_ascii_lowercase();
            let constraint = match constraint {
                Some(constraint) => constraint.to_string(),
                None => require::synthesize_constraint(
                    &package_name,
                    &Value::Object(root.clone()),
                    &project_dir,
                    cache_dir,
                    offline,
                )?,
            };
            links.insert(package_name.clone(), Value::String(constraint));
            allow_list.push(package_name);
        }
        root.insert(key.to_string(), Value::Object(links));
    }

    // `--force` may overwrite a file that already exists (its indent
    // detected before this write replaces it); a brand new file has nothing
    // to detect from, so falls back to `normalize::detect_indent`'s own
    // four-space default (an empty string has no indented line either).
    let existing = fs_err::read_to_string(&composer_json_path).unwrap_or_default();
    let indent = normalize::detect_indent(&existing);

    fs_err::write(
        &composer_json_path,
        serde_json::to_vec(&Value::Object(root))?,
    )?;
    normalize::maybe_normalize(&composer_json_path, &indent)?;

    let status = validate::run(&ValidateArgs {
        file: Some(composer_json_path.clone()),
        no_check_all: false,
        check_lock: false,
        no_check_lock: false,
        no_check_publish: false,
        with_dependencies: false,
        strict: false,
    })?;
    if status >= 2 {
        bail!("{} failed validation", composer_json_path.display());
    }

    if allow_list.is_empty() {
        out(&format!("Wrote {}", composer_json_path.display()));
        out("Run `viv add <pkg>` to add a dependency.");
    } else {
        require::partial_update(
            &project_dir,
            cache_dir,
            offline,
            false,
            false,
            &allow_list,
            UpdateAllowMode::OnlyListed,
            false,
            false,
            args.no_install,
            false,
        )?;
        out(&format!("Wrote {}", composer_json_path.display()));
    }
    Ok(())
}

/// The default `psr-4` entry: `--autoload <dir>` always sets one, forcing
/// it even over a directory that doesn't exist yet; otherwise `src/` only
/// when it's already there. Returns `(namespace, dir)`.
fn autoload_entry(
    args: &InitArgs,
    project_dir: &std::path::Path,
    name: &str,
) -> Option<(String, String)> {
    let dir = match &args.autoload {
        Some(dir) => dir.clone(),
        None if project_dir.join("src").is_dir() => "src".to_string(),
        None => return None,
    };
    let project_half = name.split('/').next_back().unwrap_or(name);
    Some((studly(project_half), dir))
}

/// `git config user.name`, falling back to `project_dir`'s directory name,
/// both slugged: `<vendor>/<project>`.
fn default_name(project_dir: &std::path::Path) -> String {
    let vendor = slug(&git_user_name().unwrap_or_else(os_user_name));
    let vendor = if vendor.is_empty() {
        "user".to_string()
    } else {
        vendor
    };
    let project = project_dir
        .file_name()
        .and_then(|name| name.to_str())
        .map(slug)
        .filter(|slugged| !slugged.is_empty())
        .unwrap_or_else(|| "project".to_string());
    format!("{vendor}/{project}")
}

fn git_user_name() -> Option<String> {
    let output = std::process::Command::new("git")
        .args(["config", "user.name"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!name.is_empty()).then_some(name)
}

fn os_user_name() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("LOGNAME"))
        .unwrap_or_default()
}

/// Lowercase, non-alphanumeric runs collapsed to a single `-`, trimmed.
fn slug(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut last_dash = false;
    for c in input.to_ascii_lowercase().chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
            last_dash = false;
        } else if !last_dash {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

/// `-`/`_`-delimited parts, each capitalised: `my-app` -> `MyApp`.
fn studly(input: &str) -> String {
    input
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

/// `viv new <dir>` (#139): the same no-prompt defaults `run` writes for a
/// bare `viv init`, into a directory `new` has already created — no flags of
/// its own, since `new`'s own directory-name-only shape has nowhere to put
/// `viv init`'s `--require`/`--license`/etc.
pub(crate) fn write_defaults(
    project_dir: &Path,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<()> {
    run(
        &InitArgs {
            name: None,
            description: None,
            package_type: None,
            license: None,
            autoload: None,
            require: Vec::new(),
            require_dev: Vec::new(),
            no_install: false,
            force: false,
            project_dir: project_dir.to_path_buf(),
        },
        cache_dir,
        offline,
    )
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_collapses_and_trims() {
        assert_eq!(slug("Sander van Dragt"), "sander-van-dragt");
        assert_eq!(slug("--Weird__Name--"), "weird-name");
        assert_eq!(slug(""), "");
    }

    #[test]
    fn studly_capitalises_each_part() {
        assert_eq!(studly("my-app"), "MyApp");
        assert_eq!(studly("my_app"), "MyApp");
        assert_eq!(studly("app"), "App");
    }
}
