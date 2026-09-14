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
        | "--no-progress"
        | "-o"
        | "--optimize-autoloader"
        | "-a"
        | "--classmap-authoritative"
        | "--apcu-autoloader"
        | "-d"
        | "--working-dir"
        | "--ignore-platform-reqs"
        | "--ignore-platform-req"
        | "-v"
        | "-vv"
        | "-vvv"
        // viv has `--no-plugins` (`install`/`dump-autoload` both take it,
        // #227): dropping it silently, like `--no-interaction`, downgraded a
        // plugin refusal back to a hard error instead of the warning the
        // user asked for by passing it.
        | "--no-plugins" => Flag::Keep,
        "--no-interaction" | "-n" | "--prefer-dist" => Flag::Drop,
        "-q" | "--quiet" => Flag::DropNoted("viv has no equivalent of {flag}, ignoring it"),
        _ => Flag::Unknown,
    }
}

/// Flags whose value is a separate following argument rather than joined
/// with `=` (`-d /tmp` as well as `-d=/tmp`; #228). Every `translate*` loop
/// below checks this before classifying, so the value is consumed alongside
/// its flag instead of being classified — or, in `translate_with_packages`/
/// `translate_create_project`, mistaken for a package-name positional — on
/// its own. Not every flag here is accepted by every subcommand; each
/// `classify*` function's own match arms still gate that.
const VALUE_FLAGS: &[&str] = &[
    "-d",
    "--working-dir",
    "--ignore-platform-req",
    "--repository",
];

/// Translate `install`/`dump-autoload` args, or `None` if any arg isn't understood.
fn translate(args: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        // `--working-dir=foo` / `--ignore-platform-req=foo`: match on the flag part only.
        let flag = arg.split('=').next().unwrap_or(arg);
        let value = (!arg.contains('=') && VALUE_FLAGS.contains(&flag))
            .then(|| iter.next())
            .flatten();
        match classify(flag) {
            Flag::Keep => {
                out.push(if flag == "--working-dir" {
                    arg.replacen("--working-dir", "-d", 1)
                } else if flag == "-vv" || flag == "-vvv" {
                    "-v".to_string()
                } else {
                    arg.clone()
                });
                if let Some(value) = value {
                    out.push(value.clone());
                }
            }
            Flag::Drop => {}
            Flag::DropNoted(msg) => {
                err_out(&format!(
                    "composer (viv shim): {}",
                    msg.replace("{flag}", flag)
                ));
            }
            Flag::Unknown => {
                note_unknown_flag(flag);
                return None;
            }
        }
    }
    Some(out)
}

/// Flags accepted for `create-project`, mapped onto viv `new`'s own set.
/// Everything `new` doesn't support at all (`--prefer-source`, `--keep-vcs`,
/// `--stability`, `--ask`, ...) is `Unknown`, falling over to the real
/// Composer rather than silently dropping something that changes behaviour.
fn classify_create_project(arg: &str) -> Flag {
    match arg {
        "--no-dev" | "--no-install" | "--no-scripts" | "--repository" => Flag::Keep,
        "--prefer-dist" | "--no-interaction" | "-n" => Flag::Drop,
        "--ignore-platform-reqs" | "--ignore-platform-req" | "-q" | "--quiet" => {
            Flag::DropNoted("viv has no equivalent of {flag}, ignoring it")
        }
        _ => Flag::Unknown,
    }
}

/// Translate `create-project`'s positionals unchanged (`viv new` takes the
/// same `vendor/package [dir [constraint]]` shape) and its flags via
/// [`classify_create_project`], `--repository`'s value (either spelling,
/// #228) consumed via [`VALUE_FLAGS`] rather than special-cased here.
fn translate_create_project(args: &[String]) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') {
            out.push(arg.clone());
            continue;
        }
        let flag = arg.split('=').next().unwrap_or(arg);
        let value = (!arg.contains('=') && VALUE_FLAGS.contains(&flag))
            .then(|| iter.next())
            .flatten();
        match classify_create_project(flag) {
            Flag::Keep => {
                out.push(arg.clone());
                if let Some(value) = value {
                    out.push(value.clone());
                }
            }
            Flag::Drop => {}
            Flag::DropNoted(msg) => {
                err_out(&format!(
                    "composer (viv shim): {}",
                    msg.replace("{flag}", flag)
                ));
            }
            Flag::Unknown => {
                note_unknown_flag(flag);
                return None;
            }
        }
    }
    Some(out)
}

/// Flags accepted for `update`, mapped onto `viv update`'s own set
/// (`UpdateArgs`). `--with`/`--root-reqs`/`--prefer-source`/`--no-suggest`
/// have no viv equivalent at all and aren't listed, so they fall through as
/// `Unknown` via the wildcard, same reasoning as `classify_create_project`.
fn classify_update(arg: &str) -> Flag {
    match arg {
        "--no-dev"
        | "--dry-run"
        | "--lock"
        | "--prefer-lowest"
        | "--prefer-stable"
        | "--minimal-changes"
        | "-w"
        | "--with-dependencies"
        | "-W"
        | "--with-all-dependencies"
        | "--no-scripts"
        | "--no-plugins"
        | "--no-install"
        | "--no-blocking"
        | "--no-security-blocking"
        | "-d"
        | "--working-dir"
        | "--ignore-platform-reqs"
        | "--ignore-platform-req"
        | "-v"
        | "-vv"
        | "-vvv" => Flag::Keep,
        "--no-interaction" | "-n" | "--prefer-dist" => Flag::Drop,
        "-q" | "--quiet" | "--no-progress" => {
            Flag::DropNoted("viv has no equivalent of {flag}, ignoring it")
        }
        _ => Flag::Unknown,
    }
}

/// Flags accepted for `require`, mapped onto `viv add`'s own set
/// (`RequireArgs`).
fn classify_require(arg: &str) -> Flag {
    match arg {
        "--dev"
        | "--no-update"
        | "--prefer-lowest"
        | "--prefer-stable"
        | "--sort-packages"
        | "--no-scripts"
        | "--no-plugins"
        | "--no-install"
        | "--no-blocking"
        | "--no-security-blocking"
        | "-d"
        | "--working-dir"
        | "--ignore-platform-reqs"
        | "--ignore-platform-req"
        | "-v"
        | "-vv"
        | "-vvv" => Flag::Keep,
        "--no-interaction" | "-n" | "--prefer-dist" => Flag::Drop,
        "-q" | "--quiet" | "--no-progress" => {
            Flag::DropNoted("viv has no equivalent of {flag}, ignoring it")
        }
        _ => Flag::Unknown,
    }
}

/// Flags accepted for `remove`, mapped onto `viv rm`'s own set (`RemoveArgs`,
/// a strict subset of `RequireArgs`: no `--prefer-lowest`/`--prefer-stable`/
/// `--sort-packages`, since removing doesn't pick a version).
fn classify_remove(arg: &str) -> Flag {
    match arg {
        "--dev"
        | "--no-update"
        | "--no-scripts"
        | "--no-plugins"
        | "--no-install"
        | "--no-blocking"
        | "--no-security-blocking"
        | "-d"
        | "--working-dir"
        | "--ignore-platform-reqs"
        | "--ignore-platform-req"
        | "-v"
        | "-vv"
        | "-vvv" => Flag::Keep,
        "--no-interaction" | "-n" => Flag::Drop,
        "-q" | "--quiet" | "--no-progress" => {
            Flag::DropNoted("viv has no equivalent of {flag}, ignoring it")
        }
        _ => Flag::Unknown,
    }
}

/// Translate `update`/`add`/`rm` args: package-name positionals (partial
/// update's own package list, `add`/`rm`'s required list) pass through
/// unchanged, same as `translate_create_project`'s positionals; flags go
/// through the given `classify` and, on `Flag::Keep`, the same
/// `--working-dir`-to-`-d`/`-vv`+`-vvv`-to-`-v` rewrite `translate` does.
fn translate_with_packages(args: &[String], classify: fn(&str) -> Flag) -> Option<Vec<String>> {
    let mut out = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if !arg.starts_with('-') {
            out.push(arg.clone());
            continue;
        }
        let flag = arg.split('=').next().unwrap_or(arg);
        // Consumed here, before the next loop iteration's own positional
        // check, so a value flag's value (e.g. `--ignore-platform-req
        // ext-foo`) can't fall through as if `ext-foo` were a package name,
        // whether that flag ends up kept or dropped.
        let value = (!arg.contains('=') && VALUE_FLAGS.contains(&flag))
            .then(|| iter.next())
            .flatten();
        match classify(flag) {
            Flag::Keep => {
                out.push(if flag == "--working-dir" {
                    arg.replacen("--working-dir", "-d", 1)
                } else if flag == "-vv" || flag == "-vvv" {
                    "-v".to_string()
                } else {
                    arg.clone()
                });
                if let Some(value) = value {
                    out.push(value.clone());
                }
            }
            Flag::Drop => {}
            Flag::DropNoted(msg) => {
                err_out(&format!(
                    "composer (viv shim): {}",
                    msg.replace("{flag}", flag)
                ));
            }
            Flag::Unknown => {
                note_unknown_flag(flag);
                return None;
            }
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

/// Names the argument that ended a translation. Says only what was observed:
/// whether that means running the real Composer or refusing to is
/// [`fallback_to_real_composer`]'s call, and it says so itself.
fn note_unknown_flag(flag: &str) {
    err_out(&format!("composer (viv shim): `{flag}` not understood"));
}

/// An argument the shim doesn't understand normally falls back to the real
/// Composer with just the stderr note above. `VIV_SHIM_STRICT=1` turns that
/// fallback into a hard error instead, for a CI job that migrated to viv and
/// wants to know if a gap silently reopened the door back to Composer. The
/// published image (#213) doesn't need this set: it ships with no real
/// Composer to fall back to, so `exec_real_composer` below already hard-errors
/// there regardless.
fn fallback_to_real_composer(args: &[String]) -> ExitCode {
    if std::env::var_os("VIV_SHIM_STRICT").is_some() {
        err_out(
            "composer (viv shim): VIV_SHIM_STRICT is set, refusing to fall back to the real Composer",
        );
        return ExitCode::from(1);
    }
    err_out("composer (viv shim): running the real Composer");
    exec_real_composer(args)
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
        "create-project" => "new",
        "update" => "update",
        "require" => "add",
        "remove" => "rm",
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
    if viv_command == "new" {
        return match translate_create_project(rest) {
            Some(translated) => exec_viv(&[vec!["new".to_string()], translated].concat()),
            None => fallback_to_real_composer(&args),
        };
    }
    let packages_classifier: Option<fn(&str) -> Flag> = match viv_command {
        "update" => Some(classify_update),
        "add" => Some(classify_require),
        "rm" => Some(classify_remove),
        _ => None,
    };
    if let Some(classify) = packages_classifier {
        return match translate_with_packages(rest, classify) {
            Some(translated) => exec_viv(&[vec![viv_command.to_string()], translated].concat()),
            None => fallback_to_real_composer(&args),
        };
    }

    match translate(rest) {
        Some(translated) => exec_viv(&[vec![viv_command.to_string()], translated].concat()),
        None => fallback_to_real_composer(&args),
    }
}
