//! `viv`: install PHP dependencies from an existing `composer.lock`.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use vivace::audit::{self, AuditArgs};
use vivace::install::{self, CacheArgs, DumpAutoloadArgs, InstallArgs};
use vivace::normalize::{self, NormalizeArgs};
use vivace::require::{self, RemoveArgs, RequireArgs};
use vivace::show::{self, OutdatedArgs, ShowArgs};
use vivace::solver::problem::SolverError;
use vivace::update::{self, UpdateArgs};

#[derive(Parser)]
#[command(
    name = "viv",
    version,
    about = "Fast composer install from composer.lock"
)]
struct Cli {
    /// Raise logging to debug.
    #[arg(short = 'v', long, global = true)]
    verbose: bool,
    /// Store location (default `$XDG_CACHE_HOME/vivace`, or `~/.cache/vivace`).
    #[arg(long, global = true)]
    cache_dir: Option<PathBuf>,
    /// Fail fast on any request instead of connecting: install errors,
    /// naming every package not already in the store; update solves from
    /// cached repository metadata only, erroring on an uncached package.
    /// Also set by `COMPOSER_DISABLE_NETWORK` (any value but unset, empty or
    /// `0`; Composer's own git-priming `prime` value is not special-cased
    /// here, since neither `install` nor `update` touch a git source).
    #[arg(long, global = true)]
    offline: bool,
    #[command(subcommand)]
    command: Command,
}

/// Composer's own `(bool) Platform::getEnv('COMPOSER_DISABLE_NETWORK')` cast:
/// unset or empty is `false` (`getenv` returns `false`, PHP's `(bool)` of
/// that or `""` is `false`), the literal `"0"` is `false` too (PHP's numeric
/// string special case), anything else — including `"1"` and `"prime"` — is
/// `true`.
fn network_disabled_by_env() -> bool {
    network_disabled(std::env::var("COMPOSER_DISABLE_NETWORK").ok().as_deref())
}

fn network_disabled(value: Option<&str>) -> bool {
    value.is_some_and(|value| !value.is_empty() && value != "0")
}

#[cfg(test)]
mod tests {
    use super::network_disabled;

    #[test]
    fn network_disabled_reads_composer_disable_networks_bool_cast() {
        assert!(!network_disabled(None), "unset");
        assert!(!network_disabled(Some("")), "empty");
        assert!(!network_disabled(Some("0")), "the literal \"0\" is falsy");
        assert!(network_disabled(Some("1")));
        assert!(
            network_disabled(Some("prime")),
            "HttpDownloader's own (bool) cast doesn't special-case \"prime\" \
             the way Git.php does"
        );
    }
}

#[derive(Subcommand)]
enum Command {
    /// Install packages from composer.lock.
    Install(InstallArgs),
    /// Resolve composer.json and write a composer.lock (full or partial
    /// update).
    Update(UpdateArgs),
    /// Add a dependency to composer.json and resolve it. Stops at the
    /// lock: run `viv install` afterwards (vivace does not chain into
    /// install the way `composer require` does).
    Require(RequireArgs),
    /// Remove a dependency from composer.json and resolve the rest.
    Remove(RemoveArgs),
    /// Regenerate the autoload files and `vendor/bin` from an already
    /// installed `vendor/`, without fetching or linking.
    DumpAutoload(DumpAutoloadArgs),
    /// Normalize composer.json's key order and formatting, a native
    /// `composer normalize` (ergebnis/composer-normalize).
    Normalize(NormalizeArgs),
    /// Cache maintenance: prune stale entries, or remove the cache outright.
    Cache(CacheArgs),
    /// Check installed (or locked) packages for security vulnerability
    /// advisories and abandoned packages.
    Audit(AuditArgs),
    /// List installed packages, or inspect one (`--tree`/`-t` for the
    /// require tree).
    Show(ShowArgs),
    /// `show --tree`'s spelling (#86).
    Tree(ShowArgs),
    /// List installed packages with a newer version available
    /// (`show --latest --outdated`).
    Outdated(OutdatedArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    let offline = cli.offline || network_disabled_by_env();
    match cli.command {
        Command::Install(args) => match install::run(&args, cli.cache_dir.as_deref(), offline) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                err_out(&format!("{err:#}"));
                ExitCode::from(1)
            }
        },
        Command::Update(args) => match update::run(&args, cli.cache_dir.as_deref(), offline) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => resolver_error(&err),
        },
        Command::Require(args) => match require::run_require(&args, cli.cache_dir.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => resolver_error(&err),
        },
        Command::Remove(args) => match require::run_remove(&args, cli.cache_dir.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => resolver_error(&err),
        },
        Command::DumpAutoload(args) => match install::dump_autoload(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                err_out(&format!("{err:#}"));
                ExitCode::from(1)
            }
        },
        Command::Normalize(args) => match normalize::run(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                err_out(&format!("{err:#}"));
                ExitCode::from(1)
            }
        },
        Command::Cache(args) => match install::cache(&args, cli.cache_dir.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                err_out(&format!("{err:#}"));
                ExitCode::from(1)
            }
        },
        Command::Audit(args) => match audit::run(&args) {
            Ok(status) => ExitCode::from(status),
            Err(err) => {
                err_out(&format!("{err:#}"));
                ExitCode::from(1)
            }
        },
        Command::Show(args) => match show::run(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                err_out(&format!("{err:#}"));
                ExitCode::from(1)
            }
        },
        Command::Tree(args) => match show::run_tree(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                err_out(&format!("{err:#}"));
                ExitCode::from(1)
            }
        },
        Command::Outdated(args) => {
            match show::run_outdated(&args, cli.cache_dir.as_deref(), offline) {
                Ok(true) => ExitCode::from(1),
                Ok(false) => ExitCode::SUCCESS,
                Err(err) => {
                    err_out(&format!("{err:#}"));
                    ExitCode::from(1)
                }
            }
        }
    }
}

/// `update`/`require`/`remove`'s shared error path: a [`SolverError`]
/// prints its own Composer-shaped message with no extra context wrapping
/// and exits `2`, matching `Installer::ERROR_DEPENDENCY_RESOLUTION_FAILED`'s
/// exit code; anything else (a missing file, a bad `composer.json`, ...)
/// keeps the plain `{err:#}` chain and exit `1`.
fn resolver_error(err: &anyhow::Error) -> ExitCode {
    if let Some(solver_error) = err.downcast_ref::<SolverError>() {
        err_out(&format!("{solver_error}"));
        return ExitCode::from(2);
    }
    err_out(&format!("{err:#}"));
    ExitCode::from(1)
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr` lint.
fn err_out(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

fn init_logging(verbose: bool) {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(if verbose { "debug" } else { "info" }));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
}
