//! Drop-in `composer` shim: translates the subset of Composer's CLI that viv
//! supports into `viv` invocations, execs the real Composer for the rest.
use std::io::Write as _;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

/// Flags accepted for `install`/`dump-autoload` and how the shim treats them.
enum Flag {
    /// Pass through unchanged.
    Keep,
    /// Drop silently: viv either doesn't need it or has no plugins/scripts to affect.
    Drop,
    /// Drop, but tell the user why on stderr.
    DropNoted(&'static str),
    /// Not understood: fall back to the real Composer.
    Unknown,
}

fn classify(arg: &str) -> Flag {
    match arg {
        "--no-dev"
        | "--dry-run"
        | "--no-scripts"
        | "-o"
        | "--optimize-autoloader"
        | "-a"
        | "--classmap-authoritative"
        | "--apcu-autoloader"
        | "-d"
        | "--working-dir"
        | "-v"
        | "-vv"
        | "-vvv" => Flag::Keep,
        "--no-plugins" | "--no-interaction" | "-n" | "--prefer-dist" => Flag::Drop,
        "--ignore-platform-reqs" | "--ignore-platform-req" | "-q" | "--quiet" => {
            Flag::DropNoted("viv has no equivalent of {flag}, ignoring it")
        }
        _ => Flag::Unknown,
    }
}

/// Translate `install`/`dump-autoload` args, or `None` if any arg isn't understood.
fn translate(args: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    for arg in args {
        // `--working-dir=foo` / `--ignore-platform-req=foo`: match on the flag part only.
        let flag = arg.split('=').next().unwrap_or(arg);
        match classify(flag) {
            Flag::Keep => {
                out.push(if flag == "--working-dir" {
                    arg.replacen("--working-dir", "-d", 1)
                } else if flag == "-vv" || flag == "-vvv" {
                    "-v".to_string()
                } else {
                    arg.clone()
                });
            }
            Flag::Drop => {}
            Flag::DropNoted(msg) => {
                err_out(&format!(
                    "composer (viv shim): {}",
                    msg.replace("{flag}", flag)
                ));
            }
            Flag::Unknown => return None,
        }
    }
    Some(out)
}

fn viv_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let candidate = dir.join("viv");
        if candidate.is_file() {
            return candidate;
        }
    }
    PathBuf::from("viv")
}

/// Locate the real Composer, skipping this binary itself when scanning `PATH`.
fn real_composer() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("VIV_COMPOSER_PATH") {
        return Some(PathBuf::from(path));
    }
    let self_canon = std::env::current_exe()
        .ok()
        .and_then(|p| p.canonicalize().ok());
    if let Ok(path_var) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_var) {
            let candidate = dir.join("composer");
            if !candidate.is_file() {
                continue;
            }
            let candidate_canon = candidate.canonicalize().ok();
            if self_canon.is_some() && candidate_canon == self_canon {
                continue;
            }
            return Some(candidate);
        }
    }
    let phar = Path::new("composer.phar");
    if phar.is_file() {
        return Some(phar.to_path_buf());
    }
    None
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr` lint.
fn err_out(message: &str) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

/// `exec`s the real Composer with `args`. Only returns (with a failure code)
/// if the real Composer can't be found or run.
fn exec_real_composer(args: &[String]) -> ExitCode {
    if let Some(composer) = real_composer() {
        let err = Command::new(composer).args(args).exec();
        err_out(&format!(
            "composer (viv shim): failed to exec real Composer: {err}"
        ));
        return ExitCode::from(1);
    }
    err_out(
        "composer (viv shim): no real Composer found. Set VIV_COMPOSER_PATH, put \
         `composer` on PATH, or place `composer.phar` in the project root.",
    );
    ExitCode::from(1)
}

/// `exec`s `viv` with `args`. Only returns (with a failure code) if `viv`
/// can't be run.
///
/// Sets `VIV_VIA_SHIM=1` so `viv install` knows a user typed `composer
/// install` directly, rather than a script under the shim: a Composer-written
/// `vendor/` gets a confirmation prompt before viv adopts it in place (#123).
fn exec_viv(args: &[String]) -> ExitCode {
    let err = Command::new(viv_path())
        .args(args)
        .env("VIV_VIA_SHIM", "1")
        .exec();
    err_out(&format!("composer (viv shim): failed to exec viv: {err}"));
    ExitCode::from(1)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.first().map(String::as_str) == Some("--version") {
        let _ = writeln!(
            std::io::stdout().lock(),
            "viv {} (composer shim)",
            env!("CARGO_PKG_VERSION")
        );
        return exec_real_composer(&args);
    }

    let Some(command) = args.first() else {
        return exec_real_composer(&args);
    };

    let viv_command = match command.as_str() {
        "install" => "install",
        "dump-autoload" | "dumpautoload" => "dump-autoload",
        "normalize" => "normalize",
        _ => "",
    };

    if viv_command.is_empty() {
        return exec_real_composer(&args);
    }

    let rest = &args[1..];
    if viv_command == "normalize" {
        // No flag translation needed: viv's `normalize` already matches Composer's.
        return exec_viv(&[vec!["normalize".to_string()], rest.to_vec()].concat());
    }

    match translate(rest) {
        Some(translated) => exec_viv(&[vec![viv_command.to_string()], translated].concat()),
        None => exec_real_composer(&args),
    }
}
