//! `viv`: install PHP dependencies from an existing `composer.lock`.

use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use vivace::install::{self, CacheArgs, DumpAutoloadArgs, InstallArgs};
use vivace::normalize::{self, NormalizeArgs};

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
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install packages from composer.lock.
    Install(InstallArgs),
    /// Not implemented in vivace v0.1: use `composer update`.
    Update,
    /// Not implemented in vivace v0.1: use `composer require`.
    Require,
    /// Not implemented in vivace v0.1: use `composer remove`.
    Remove,
    /// Regenerate the autoload files and `vendor/bin` from an already
    /// installed `vendor/`, without fetching or linking.
    DumpAutoload(DumpAutoloadArgs),
    /// Normalize composer.json's key order and formatting, a native
    /// `composer normalize` (ergebnis/composer-normalize).
    Normalize(NormalizeArgs),
    /// Cache maintenance: prune stale entries, or remove the cache outright.
    Cache(CacheArgs),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_logging(cli.verbose);
    match cli.command {
        Command::Install(args) => match install::run(&args, cli.cache_dir.as_deref()) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                err_out(&format!("{err:#}"));
                ExitCode::from(1)
            }
        },
        Command::Update => stub("update"),
        Command::Require => stub("require"),
        Command::Remove => stub("remove"),
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
    }
}

/// The stub subcommands vivace v0.1 does not implement: no resolver, so no
/// `update`/`require`/`remove`.
fn stub(name: &str) -> ExitCode {
    err_out(&format!(
        "viv {name} is not implemented in vivace v0.1: use composer {name}"
    ));
    ExitCode::from(2)
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
