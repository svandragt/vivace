//! `viv new`/`viv create-project` (#139): start a project in a directory
//! that doesn't exist yet — `viv init` (#143) is the same thing for the
//! directory you're already in.
//!
//! A bare directory name (no `/`) runs `init::write_defaults` inside it,
//! same as `viv init`. A `vendor/package[:constraint]` spec resolves that
//! package's best version (default: newest stable), downloads its dist
//! through the store exactly [`crate::install`] does for a locked package,
//! unpacks it as the project skeleton with [`link::link_tree`], drops any
//! `.git`/`.hg` it shipped, then chains into [`update::run`]/[`install::run`]
//! the same way a normal project resolves and installs, running the
//! skeleton's `post-root-package-install`/`post-create-project-cmd` scripts
//! around that (`ScriptEvents`, unless `--no-scripts`).
//!
//! Version selection duplicates `require.rs`'s own private
//! `version_selector::pick_best` ranking rather than widening that module:
//! `new` needs the resolved [`PackageVersion`] itself (for its `dist`), not
//! just a synthesised constraint string, and needs to honour `--repository`,
//! which `require::synthesize_constraint` doesn't.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::{Map, Value, json};

use crate::auth::Auth;
use crate::fetch::{Downloaded, Fetcher};
use crate::init;
use crate::install::{self, InstallArgs};
use crate::link::{self, LinkMode};
use crate::lock;
use crate::repository::{DevAcceptance, HttpTransport, PackageVersion, Repository, Transport};
use crate::require;
use crate::scripts;
use crate::semver;
use crate::store::Store;
use crate::update::{self, UpdateArgs};

/// `viv new`/`viv create-project` flags.
#[derive(Args, Debug, Clone)]
pub struct NewArgs {
    /// A bare directory name for an empty project, or
    /// `vendor/package[:constraint]` for a skeleton (constraint defaults to
    /// the newest stable version).
    pub target: String,
    /// Directory to create the skeleton in (default: the package's short
    /// name); rejected alongside a bare directory `target`.
    pub dir: Option<String>,
    /// `composer create-project vendor/package dir constraint`'s own third
    /// positional, accepted only for that compatibility: prefer
    /// `vendor/package:constraint`. An error alongside a colon constraint.
    pub constraint: Option<String>,
    /// Resolve without `require-dev`; `install` (unless `--no-install`)
    /// skips dev packages too, matching `viv update`/`viv install`'s own
    /// `--no-dev`.
    #[arg(long = "no-dev")]
    pub no_dev: bool,
    /// Unpack the skeleton and write composer.lock, but don't install.
    #[arg(long = "no-install")]
    pub no_install: bool,
    /// Skip `post-root-package-install`/`post-create-project-cmd` and every
    /// other root `scripts` listener the install half would run.
    #[arg(long = "no-scripts")]
    pub no_scripts: bool,
    /// Extra `composer`-type repository to resolve the skeleton from
    /// (Private Packagist, Satis), same priority as `composer.json`'s own
    /// `repositories`.
    #[arg(long)]
    pub repository: Option<String>,
}

pub fn run(args: &NewArgs, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let (raw_name, colon_constraint) = require::split_spec(&args.target);
    if !raw_name.contains('/') {
        if args.dir.is_some() || args.constraint.is_some() {
            bail!(
                "{:?} is a plain directory name; a second or third argument only applies to a \
                 vendor/package skeleton",
                args.target
            );
        }
        return run_empty(&args.target, cache_dir, offline);
    }

    let constraint = match (colon_constraint, &args.constraint) {
        (Some(_), Some(_)) => bail!(
            "{:?} already names a constraint after the colon; drop the third argument",
            args.target
        ),
        (Some(constraint), None) => Some(constraint.to_string()),
        (None, constraint) => constraint.clone(),
    };
    run_from_package(
        raw_name,
        args.dir.as_deref(),
        constraint.as_deref(),
        args,
        cache_dir,
        offline,
    )
}

fn run_empty(dir_name: &str, cache_dir: Option<&Path>, offline: bool) -> Result<()> {
    let dir = PathBuf::from(dir_name);
    create_target_dir(&dir)?;
    init::write_defaults(&dir, cache_dir, offline)?;
    out(&format!(
        "Run `cd {dir_name} && viv add <package>` to add a dependency."
    ));
    Ok(())
}

fn run_from_package(
    name: &str,
    dir: Option<&str>,
    constraint: Option<&str>,
    args: &NewArgs,
    cache_dir: Option<&Path>,
    offline: bool,
) -> Result<()> {
    let name = name.to_ascii_lowercase();
    let (_vendor, short_name) = name
        .split_once('/')
        .with_context(|| format!("{name}: expected vendor/package"))?;
    let dir_name = dir.map_or_else(|| short_name.to_string(), str::to_string);
    let dir = PathBuf::from(&dir_name);
    create_target_dir(&dir)?;

    let cache_dir = match cache_dir {
        Some(dir) => dir.to_path_buf(),
        None => update::default_cache_dir()?,
    };
    let auth = Auth::load(&dir)?;
    let fetcher = Fetcher::new(auth)?.offline(offline);
    let transport = HttpTransport { fetcher: &fetcher };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    let repo_root = match &args.repository {
        Some(url) => json!({"repositories": {"viv-new": {"type": "composer", "url": url}}}),
        None => json!({}),
    };
    let repo = runtime.block_on(Repository::from_composer_json(
        &repo_root, &cache_dir, transport,
    ))?;

    let constraint_text = constraint.unwrap_or("*");
    let version = runtime.block_on(resolve_version(&repo, &name, constraint_text))?;

    let store = Store::open(&cache_dir)?;
    let package = to_lock_package(&version)?;
    package.validate_dist()?;
    let temp_dir = store.temp_dir()?;
    let downloaded = runtime
        .block_on(fetcher.fetch(&package, &temp_dir))
        .with_context(|| format!("{name}: fetching dist"))?;
    let extracted = match downloaded {
        Downloaded::Bytes(bytes) => store.add_zip(&package, &bytes),
        Downloaded::File(path) => store.add_archive_from_file(&package, &path),
    }
    .with_context(|| format!("{name}: extracting dist"))?;

    link::link_tree(&extracted, &dir, LinkMode::Copy)
        .with_context(|| format!("unpacking {name} into {}", dir.display()))?;
    for vcs_dir in [".git", ".hg"] {
        let path = dir.join(vcs_dir);
        if path.exists() {
            fs_err::remove_dir_all(&path)?;
        }
    }

    let composer_json_path = dir.join("composer.json");
    let composer_json = fs_err::read(&composer_json_path)
        .with_context(|| format!("{name}: skeleton has no composer.json"))?;
    let composer_json_value: Value =
        serde_json::from_slice(&composer_json).context("parsing the skeleton's composer.json")?;
    let bin_dir = update::bin_dir(&composer_json_value);
    let mut scripts = scripts::Runner::new(
        &composer_json_value,
        &dir,
        &bin_dir,
        !args.no_dev,
        args.no_scripts,
    );
    scripts.dispatch("post-root-package-install")?;

    // A skeleton that ships no lock needs one written before it can install
    // (`update::run` does both, chaining into `install::run` itself unless
    // `--no-install`); one that already ships a lock only needs the install
    // half, and nothing at all under `--no-install`.
    if !dir.join("composer.lock").exists() {
        update::run(
            &UpdateArgs {
                packages: Vec::new(),
                with_dependencies: false,
                with_all_dependencies: false,
                minimal_changes: false,
                lock: false,
                no_dev: args.no_dev,
                prefer_lowest: false,
                prefer_stable: false,
                dry_run: false,
                no_normalize: false,
                project_dir: dir.clone(),
                no_scripts: args.no_scripts,
                no_plugins: false,
                no_install: args.no_install,
                no_blocking: false,
                no_security_blocking: false,
            },
            Some(&cache_dir),
            offline,
        )?;
    } else if !args.no_install {
        install::run(
            &InstallArgs {
                no_dev: args.no_dev,
                dry_run: false,
                link_mode: LinkMode::default(),
                adopt: false,
                project_dir: dir.clone(),
                optimize_autoloader: false,
                classmap_authoritative: false,
                apcu_autoloader: false,
                apcu_autoloader_prefix: None,
                no_scripts: args.no_scripts,
                no_normalize: false,
                no_plugins: false,
                no_progress: false,
            },
            Some(&cache_dir),
            offline,
        )?;
    }
    if !args.no_install {
        scripts.dispatch("post-create-project-cmd")?;
    }

    out(&format!(
        "Created {name} in {}",
        fs_err::canonicalize(&dir)?.display()
    ));
    out(&format!(
        "Run `cd {dir_name} && viv add <package>` to add a dependency."
    ));
    Ok(())
}

/// Creates `dir` (parents included) if it doesn't exist yet, or checks it's
/// already empty: shared by the empty-project path (which populates it
/// itself) and the skeleton path (which hands it to
/// [`link::link_tree`], which recreates it atomically).
fn create_target_dir(dir: &Path) -> Result<()> {
    match fs_err::read_dir(dir) {
        Ok(mut entries) => {
            if entries.next().is_some() {
                bail!("{}: already exists and is not empty", dir.display());
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            fs_err::create_dir_all(dir)?;
        }
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

/// `VersionSelector::findBestCandidate`, restricted to versions matching
/// `constraint_text` (default `"*"`, everything): rank by
/// `require.rs`'s own stability preference (stable wins, else the least bad
/// stability available), then by version, highest first. Duplicated rather
/// than reused: `require.rs`'s copy is `pub(super)` to that module's own
/// `version_selector`, and only ever resolves against `PACKAGIST_URL`, not
/// `--repository`.
async fn resolve_version<T: Transport>(
    repo: &Repository<T>,
    name: &str,
    constraint_text: &str,
) -> Result<PackageVersion> {
    let constraint = semver::parse_constraint(constraint_text)
        .with_context(|| format!("{constraint_text:?}: not a valid constraint"))?;
    let dev = if constraint_text.starts_with("dev-") || constraint_text.ends_with("-dev") {
        DevAcceptance::Both
    } else {
        DevAcceptance::NonDevOnly
    };
    let versions = repo.load_package(name, dev).await?;
    let preferred_rank = stability_rank("stable");
    let mut best: Option<(PackageVersion, semver::NormalizedVersion, &'static str)> = None;
    for pv in versions {
        let normalized = semver::normalize(&pv.version_normalized)?;
        if !constraint.matches(&normalized) {
            continue;
        }
        let stability = semver::stability(normalized.as_str());
        let rank = stability_rank(stability);
        let candidate_is_worse_than_preferred = preferred_rank < rank;
        let better = match &best {
            None => true,
            Some((_, best_version, best_stability)) => {
                let best_rank = stability_rank(best_stability);
                let best_is_worse_than_preferred = preferred_rank < best_rank;
                if candidate_is_worse_than_preferred && !best_is_worse_than_preferred {
                    false
                } else if !candidate_is_worse_than_preferred && best_is_worse_than_preferred {
                    true
                } else {
                    semver::compare(&normalized, best_version) == std::cmp::Ordering::Greater
                }
            }
        };
        if better {
            best = Some((pv, normalized, stability));
        }
    }
    best.map(|(pv, ..)| pv)
        .with_context(|| format!("{name}: no version matches {constraint_text:?}"))
}

fn stability_rank(stability: &str) -> u8 {
    match stability {
        "stable" => 0,
        "RC" => 5,
        "beta" => 10,
        "alpha" => 15,
        // "dev" and anything unrecognised both rank last.
        _ => 20,
    }
}

/// A [`PackageVersion`]'s fields the store/fetcher need to download and
/// extract its dist, wrapped as a [`lock::Package`] since that's what both
/// take — everything else is a placeholder, since `new` never plans or
/// installs this package into `vendor/` (the skeleton it unpacks into `dir`
/// is the project root, not a dependency of one).
fn to_lock_package(version: &PackageVersion) -> Result<lock::Package> {
    let dist = version
        .dist
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .with_context(|| format!("{}: malformed dist entry", version.name))?;
    let source = version
        .source
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .with_context(|| format!("{}: malformed source entry", version.name))?;
    Ok(lock::Package {
        name: version.name.to_ascii_lowercase(),
        version: version.version.clone(),
        dist,
        source,
        transport_options: lock::TransportOptions::default(),
        autoload: None,
        require: Map::default(),
        provide: Map::default(),
        replace: Map::default(),
        r#type: "library".to_string(),
        target_dir: None,
        bin: Vec::new(),
        include_path: Vec::new(),
        dev: false,
        raw: Value::Null,
        install_dir: None,
        install_from_source: false,
    })
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}
