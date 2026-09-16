//! `viv validate`: a native `ValidateCommand`/`ConfigValidator` port for
//! `composer.json`, plus `composer.lock` freshness/completeness (via
//! [`crate::lock::is_fresh`]/[`crate::lock::missing_requirements`]).
//!
//! `ConfigValidator` layers three things vivace keeps apart:
//! `JsonFile::validateSchema` (a `justinrainbow/json-schema` walk over
//! `res/composer-schema.json`), `ConfigValidator`'s own hand-written checks,
//! and `ValidatingArrayLoader`'s hand-written checks (run as part of loading
//! a package from the parsed manifest). The schema pass mostly duplicates
//! what the hand-written checks already catch (a wrong JSON type), and
//! reproducing the rest byte-for-byte would need a JSON-schema validator
//! dependency (`justinrainbow/json-schema`'s own error wording); this port
//! skips that pass and sticks to the hand-written checks, with one
//! exception (#233): the schema pass is also where Composer's publish-only
//! required-property check lives (`name`, `description` — hardcoded in
//! `JsonFile::validateSchema(STRICT_SCHEMA)`, not in the schema file's own
//! top level), and a missing one is the difference between `composer
//! validate`'s exit 2 and a silent exit 0 in CI. That one finding is ported
//! by hand as `check_required_for_publish`; nothing else from the schema
//! pass is.
//!
//! Ported: name format (`ValidatingArrayLoader::hasPackageNamingError`,
//! root and every link type), the publish-only uppercase-name suggestion,
//! missing-license and version-field-present warnings, deprecated
//! `composer-installer` type, require/require-dev overlap, provide/replace
//! shadowing a requirement, commit-ref requires, `scripts-descriptions`/
//! `scripts-aliases` naming non-existent scripts, empty PSR-0/PSR-4
//! prefixes, the link-type loop's own checks (self-reference, key format,
//! constraint parse, unbound-constraint warning), and the schema pass's
//! required-`name`/`description` publish errors.
//!
//! Not ported (no fixture needs it yet, add alongside one that does):
//! JSON-schema-driven type errors beyond what the checks above already
//! catch, `additionalProperties: false` (the strict schema also rejects
//! unknown top-level keys, not just missing `name`/`description`), SPDX
//! license *validity* (only "is one set at all" is checked),
//! duplicate-key detection, `authors`/`support`/`funding`/`php-ext`/
//! `autoload`/`minimum-stability`/`source`/`dist`/`extra.branch-alias`
//! validation, and the strict-vs-unbound-constraint (`CHECK_STRICT_CONSTRAINTS`)
//! and match-nothing (`Intervals::compactConstraint`) warnings.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::Value;

use crate::lock;
use crate::normalize;
use crate::semver;

/// `viv validate` flags (`ValidateCommand::configure`, minus
/// `--no-check-version`, which vivace doesn't expose).
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors Composer's validate flags"
)]
#[derive(Args, Debug, Clone)]
pub struct ValidateArgs {
    /// Path to the `composer.json` file to validate (default: `composer.json`
    /// in the current directory). Combining this with `--project-dir` is an
    /// error: a `FILE` picks the manifest, `--project-dir` picks the
    /// directory `FILE` and `composer.lock` both default from, and giving
    /// both leaves it ambiguous which lock the freshness check should read.
    pub file: Option<PathBuf>,
    /// Run as though invoked from this directory (#234): `composer.json`,
    /// `composer.lock` and (with `--with-dependencies`) `vendor/` all
    /// resolve from here instead of the current directory.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Skip the unbound-version-constraint warning.
    #[arg(long = "no-check-all")]
    pub no_check_all: bool,
    /// Check the lock file is up to date even when `config.lock` is off
    /// (vivace has no `config.lock` support, so this only affects the
    /// error/warning split, not whether the check runs).
    #[arg(long = "check-lock")]
    pub check_lock: bool,
    /// Don't check whether the lock file is up to date.
    #[arg(long = "no-check-lock")]
    pub no_check_lock: bool,
    /// Don't check for publish errors (name best-practice violations).
    #[arg(long = "no-check-publish")]
    pub no_check_publish: bool,
    /// Also validate the `composer.json` of every installed dependency.
    #[arg(short = 'A', long = "with-dependencies")]
    pub with_dependencies: bool,
    /// Exit non-zero for warnings too, not just errors.
    #[arg(long)]
    pub strict: bool,
    /// Apply every finding that has one unambiguous fix (#262), then
    /// re-run validation and report what's left. Composer has no
    /// equivalent flag.
    #[arg(long)]
    pub fix: bool,
}

/// `ValidateCommand`'s own doc block: `0` ok, `1` warnings (`--strict`
/// only), `2` errors, `3` file missing/unreadable.
const EXIT_OK: u8 = 0;
const EXIT_WARNINGS: u8 = 1;
const EXIT_ERRORS: u8 = 2;
const EXIT_UNREADABLE: u8 = 3;

/// The result of validating one manifest (`ConfigValidator::validate`'s
/// `[$errors, $publishErrors, $warnings]` triple).
#[derive(Debug, Default)]
struct Validated {
    errors: Vec<String>,
    publish_errors: Vec<String>,
    warnings: Vec<String>,
}

/// Run `viv validate`, returning the process exit code.
pub fn run(args: &ValidateArgs) -> Result<u8> {
    let has_project_dir = args.project_dir != Path::new(".");
    if args.file.is_some() && has_project_dir {
        bail!(
            "cannot combine a FILE argument with --project-dir: which composer.lock is meant is \
             ambiguous; pass one or the other"
        );
    }

    // `Factory::getComposerFile()`'s own default: `getenv('COMPOSER') ?:
    // './composer.json'` (env override not supported here, no fixture needs
    // it), printed verbatim in every message so the leading `./` matters.
    // Kept relative even under `--project-dir`, matching what `cd <dir> &&
    // viv validate` would itself print.
    let file = args
        .file
        .clone()
        .unwrap_or_else(|| PathBuf::from("./composer.json"));
    let read_path = if has_project_dir {
        args.project_dir.join(&file)
    } else {
        file.clone()
    };

    let Ok(bytes) = fs_err::read(&read_path) else {
        err_out(&format!("{} not found.", file.display()));
        return Ok(EXIT_UNREADABLE);
    };
    let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
        err_out(&format!("{} does not contain valid JSON.", file.display()));
        return Ok(EXIT_UNREADABLE);
    };

    let project_dir = if has_project_dir {
        args.project_dir.clone()
    } else {
        file.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    };

    if args.fix {
        return run_fix(args, &read_path, &file, &project_dir, manifest, &bytes);
    }

    let check_all = !args.no_check_all;
    let mut result = validate_manifest(&manifest, check_all);
    let check_publish = !args.no_check_publish;
    let (check_lock, lock_errors) = lock_check(args, &project_dir, &bytes)?;

    let name = file.display().to_string();
    output_result(
        &name,
        &mut result,
        check_publish,
        check_lock,
        &lock_errors,
        true,
    );
    print_fixable_count(&result);
    let mut exit_code = exit_for(&result, args.strict);

    if args.with_dependencies {
        exit_code = exit_code.max(validate_dependencies(
            &project_dir,
            check_all,
            args,
            check_publish,
        )?);
    }

    Ok(exit_code)
}

/// `viv validate --fix` (#262): applies every fixable finding from the
/// issue's own table (`apply_fixable_fixes`), writes `composer.json` back
/// through the normaliser (same write-then-normalize path `update.rs`'s own
/// `bump-after-update` reuses), rewrites the lock's `content-hash` the way
/// `update --lock` does if it was stale, then re-runs validation and reports
/// what's left. Composer has no `--fix`, so there is no oracle for any of
/// this beyond the plain re-run's own report, which stays Composer-exact.
fn run_fix(
    args: &ValidateArgs,
    read_path: &Path,
    file: &Path,
    project_dir: &Path,
    manifest: Value,
    original_bytes: &[u8],
) -> Result<u8> {
    let check_all = !args.no_check_all;
    let check_publish = !args.no_check_publish;
    let (_, lock_errors) = lock_check(args, project_dir, original_bytes)?;
    let lock_was_stale = lock_errors
        .iter()
        .any(|error| error == lock::STALE_LOCK_VALIDATE_WARNING);

    let mut root = manifest;
    let fix_lines = apply_fixable_fixes(&mut root);
    for line in &fix_lines {
        err_out(line);
    }

    let indent = normalize::detect_indent(&String::from_utf8_lossy(original_bytes));
    crate::require::write_composer_json(read_path, &root)?;
    normalize::maybe_normalize(read_path, &indent)?;

    let lock_path = project_dir.join("composer.lock");
    let mut rewrote_lock = false;
    if lock_was_stale && lock_path.exists() {
        let fixed_bytes = fs_err::read(read_path).context("reading composer.json")?;
        let lock = crate::update::lock_only(&lock_path, &fixed_bytes)?;
        fs_err::write(&lock_path, lock)?;
        err_out("Updated composer.lock's content-hash");
        rewrote_lock = true;
    }

    if !fix_lines.is_empty() || rewrote_lock {
        err_out("");
    }

    let bytes = fs_err::read(read_path).context("reading composer.json")?;
    let fixed_manifest: Value = serde_json::from_slice(&bytes).context("parsing composer.json")?;
    let mut result = validate_manifest(&fixed_manifest, check_all);
    let (check_lock, lock_errors) = lock_check(args, project_dir, &bytes)?;
    let name = file.display().to_string();
    output_result(
        &name,
        &mut result,
        check_publish,
        check_lock,
        &lock_errors,
        true,
    );

    let remaining = result.errors.len() + result.warnings.len();
    if remaining > 0 {
        err_out("");
        err_out(&format!("{remaining} findings remain that need a decision"));
    }

    Ok(exit_for(&result, args.strict))
}

/// `--fix`'s own fixes (#262's table), applied to `root` in the issue's
/// order and returning each one's single stderr line. Re-derives what to
/// change straight from `root` rather than parsing `is_fixable`'s messages,
/// so the two can't silently disagree about what "fixed" means. The
/// provide/replace shadowing fix removes from `provide`/`replace`, exactly
/// as Composer's own warning text says ("Remove it from provide/replace if
/// you wish to install it."), keeping `require` so the real package still
/// installs.
fn apply_fixable_fixes(root: &mut Value) -> Vec<String> {
    let mut lines = Vec::new();

    if let Some(required) = root
        .get("require")
        .and_then(Value::as_object)
        .map(|require| require.keys().cloned().collect::<Vec<_>>())
    {
        for package in required {
            if root
                .get("require-dev")
                .and_then(Value::as_object)
                .is_some_and(|dev| dev.contains_key(&package))
                && crate::require::remove_sub_node(root, "require-dev", &package)
            {
                lines.push(format!(
                    "Removed {package} from require-dev, it is already in require"
                ));
            }
        }
    }
    crate::require::remove_main_key_if_empty(root, "require-dev");

    for link_type in ["provide", "replace"] {
        let Some(shadowing) = root
            .get(link_type)
            .and_then(Value::as_object)
            .map(|links| links.keys().cloned().collect::<Vec<_>>())
        else {
            continue;
        };
        for package in shadowing {
            let required = root
                .get("require")
                .and_then(Value::as_object)
                .is_some_and(|require| require.contains_key(&package));
            if required && crate::require::remove_sub_node(root, link_type, &package) {
                lines.push(format!(
                    "Removed {package} from {link_type}, it is also required"
                ));
            }
        }
        crate::require::remove_main_key_if_empty(root, link_type);
    }

    for section in ["scripts-descriptions", "scripts-aliases"] {
        let Some(orphans) = root.get(section).and_then(Value::as_object).map(|entries| {
            entries
                .keys()
                .filter(|name| {
                    !root
                        .get("scripts")
                        .and_then(Value::as_object)
                        .is_some_and(|scripts| scripts.contains_key(name.as_str()))
                })
                .cloned()
                .collect::<Vec<_>>()
        }) else {
            continue;
        };
        for name in orphans {
            if crate::require::remove_sub_node(root, section, &name) {
                lines.push(format!("Removed \"{name}\" from {section}, no such script"));
            }
        }
        crate::require::remove_main_key_if_empty(root, section);
    }

    if let Some(root_name) = root.get("name").and_then(Value::as_str).map(str::to_string) {
        for link_type in ["require", "require-dev", "conflict", "provide", "replace"] {
            let Some(self_key) = root
                .get(link_type)
                .and_then(Value::as_object)
                .and_then(|links| {
                    links
                        .keys()
                        .find(|key| key.eq_ignore_ascii_case(&root_name))
                        .cloned()
                })
            else {
                continue;
            };
            if crate::require::remove_sub_node(root, link_type, &self_key) {
                lines.push(format!(
                    "Removed {self_key} from {link_type}, a package cannot set a {link_type} on \
                     itself"
                ));
            }
            crate::require::remove_main_key_if_empty(root, link_type);
        }

        if root_name.contains(|c: char| c.is_ascii_uppercase()) {
            let suggested = suggest_kebab_name(&root_name);
            if let Some(obj) = root.as_object_mut() {
                obj.insert("name".to_string(), Value::String(suggested.clone()));
            }
            lines.push(format!(
                "Renamed package to \"{suggested}\"; check anywhere the old name is referenced"
            ));
        }
    }

    lines
}

/// Findings `--fix` (#262) can resolve on its own, matched by the exact
/// message shape each check pushes: only used for the "N of M can be fixed"
/// summary line, never to decide what to change (`apply_fixable_fixes` does
/// that straight off the manifest). The schema-pattern-mismatch line an
/// upper-case name also triggers (`check_name`'s first push) isn't matched
/// here: it also fires for names invalid in ways a rename can't fix, and no
/// fixture yet needs the two told apart.
/// ponytail: undercounts by at most one line on a manifest with an
/// upper-case name; split it out once a fixture needs that precision.
fn is_fixable(message: &str) -> bool {
    message == lock::STALE_LOCK_VALIDATE_WARNING
        || message.contains("required both in require and require-dev")
        || (message.starts_with("The package ") && message.contains(" is also listed in "))
        || message.contains("found in \"scripts-descriptions\"")
        || message.contains("found in \"scripts-aliases\"")
        || (message.contains(" : a package cannot set a ") && message.ends_with(" on itself"))
        || message.contains("it should not contain uppercase characters. We suggest using")
        || (message.starts_with("Name \"") && message.contains("does not match the best practice"))
}

/// The "N of M findings can be fixed" line (#262), printed after Composer's
/// own report when at least one finding `is_fixable`. `result` has already
/// been mutated by `output_result` (publish/lock findings folded into
/// `errors`/`warnings` exactly as printed), so `errors.len() +
/// warnings.len()` here is exactly what just appeared on screen.
fn print_fixable_count(result: &Validated) {
    let total = result.errors.len() + result.warnings.len();
    let fixable = result
        .errors
        .iter()
        .chain(&result.warnings)
        .filter(|message| is_fixable(message))
        .count();
    if fixable == 0 {
        return;
    }
    err_out("");
    err_out(&format!(
        "{fixable} of {total} findings can be fixed: run viv validate --fix"
    ));
}

/// `ValidateCommand::execute`'s lock-check block: `Locker::isFresh` and
/// `Locker::getMissingRequirementInfo`, gated on a lock file existing at
/// all. Returns `(checkLock, lockErrors)`; `lockErrors` is empty when there
/// is no lock or it is both fresh and complete.
fn lock_check(
    args: &ValidateArgs,
    project_dir: &Path,
    root_json: &[u8],
) -> Result<(bool, Vec<String>)> {
    let check_lock = !args.no_check_lock || args.check_lock;
    let lock_path = project_dir.join("composer.lock");
    let Ok(lock) = lock::read_lock(&lock_path) else {
        return Ok((check_lock, Vec::new()));
    };
    let root = lock::parse_root(root_json).context("parsing composer.json")?;

    let mut lock_errors = Vec::new();
    if !lock::is_fresh(&lock, root_json)? {
        // Composer-verbatim, contractual stdout — see the constant's own
        // doc comment before touching this (#239).
        lock_errors.push(lock::STALE_LOCK_VALIDATE_WARNING.to_string());
    }
    lock_errors.extend(lock::missing_requirements(&lock, &root, true));
    Ok((check_lock, lock_errors))
}

/// `--with-dependencies`: revalidate every installed package's own
/// `composer.json`, folding the worst exit code in (`ValidateCommand`'s own
/// `max($depCode, $exitCode)`). Publish and lock checks aren't repeated per
/// dependency there either.
fn validate_dependencies(
    project_dir: &Path,
    check_all: bool,
    args: &ValidateArgs,
    check_publish: bool,
) -> Result<u8> {
    let installed_path = project_dir.join("vendor/composer/installed.json");
    let Ok(content) = fs_err::read_to_string(&installed_path) else {
        return Ok(EXIT_OK);
    };
    let installed: Value = serde_json::from_str(&content)
        .with_context(|| format!("parsing {} as JSON", installed_path.display()))?;
    let mut exit_code = EXIT_OK;
    for pkg in installed
        .get("packages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let name = pkg.get("name").and_then(Value::as_str).unwrap_or_default();
        let install_dir = match pkg.get("install-path").and_then(Value::as_str) {
            Some(rel) => project_dir.join("vendor/composer").join(rel),
            None => project_dir.join("vendor").join(name),
        };
        let dep_file = install_dir.join("composer.json");
        let Ok(dep_bytes) = fs_err::read(&dep_file) else {
            continue;
        };
        let Ok(dep_manifest) = serde_json::from_slice::<Value>(&dep_bytes) else {
            continue;
        };
        let mut result = validate_manifest(&dep_manifest, check_all);
        output_result(name, &mut result, check_publish, false, &[], false);
        exit_code = exit_code.max(exit_for(&result, args.strict));
    }
    Ok(exit_code)
}

fn exit_for(result: &Validated, strict: bool) -> u8 {
    if !result.errors.is_empty() {
        EXIT_ERRORS
    } else if strict && !result.warnings.is_empty() {
        EXIT_WARNINGS
    } else {
        EXIT_OK
    }
}

/// `ConfigValidator::validate` + `ValidatingArrayLoader::load`'s
/// hand-written checks, plus the one JSON-schema-pass finding that reaches
/// `composer validate`'s exit code (see module doc): `check_required_for_publish`
/// runs first, matching `ConfigValidator::validate`'s own order (the schema
/// pass runs before any of its hand-written checks).
fn validate_manifest(manifest: &Value, check_all: bool) -> Validated {
    let mut result = Validated::default();

    check_required_for_publish(manifest, &mut result);
    check_license(manifest, &mut result);
    check_version_field_present(manifest, &mut result);
    check_name_uppercase_publish(manifest, &mut result);
    check_deprecated_type(manifest, &mut result);
    check_require_dev_overlap(manifest, &mut result);
    check_provide_replace_shadowing(manifest, &mut result);
    check_commit_refs(manifest, &mut result);
    check_scripts_shape(manifest, &mut result);
    check_empty_namespace_prefixes(manifest, &mut result);

    check_name(manifest, &mut result);
    check_link_types(manifest, check_all, &mut result);
    check_suggest(manifest, &mut result);

    result
}

fn is_missing_or_empty(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => true,
        Some(Value::String(s)) => s.is_empty(),
        Some(Value::Array(a)) => a.is_empty(),
        Some(Value::Object(o)) => o.is_empty(),
        Some(Value::Bool(b)) => !b,
        Some(Value::Number(n)) => n.as_f64() == Some(0.0),
    }
}

/// `JsonFile::validateSchema(STRICT_SCHEMA)`'s own override
/// (`$schemaData->required = ['name', 'description']` in `JsonFile.php`,
/// not anything the schema file itself declares — `res/composer-schema.json`
/// has no top-level `required` key; see module doc) reproduced as a
/// hand-written check: the only two schema-pass findings that turn into a
/// publish error and so reach the exit code (#233). `required` in the
/// `justinrainbow/json-schema` sense means "key present", not "non-empty" —
/// an empty string still satisfies it, so this checks presence only.
fn check_required_for_publish(manifest: &Value, result: &mut Validated) {
    for property in ["name", "description"] {
        if manifest.get(property).is_none() {
            result
                .publish_errors
                .push(format!("{property} : The property {property} is required"));
        }
    }
}

/// `ConfigValidator::validate`'s own license check: only "is one set at
/// all", not SPDX validity (see module doc).
fn check_license(manifest: &Value, result: &mut Validated) {
    if is_missing_or_empty(manifest.get("license")) {
        result.warnings.push(
            "No license specified, it is recommended to do so. For closed-source software you \
             may use \"proprietary\" as license."
                .to_string(),
        );
    }
}

fn check_version_field_present(manifest: &Value, result: &mut Validated) {
    if manifest.get("version").is_some() {
        result.warnings.push(
            "The version field is present, it is recommended to leave it out if the package is \
             published on Packagist."
                .to_string(),
        );
    }
}

fn check_name_uppercase_publish(manifest: &Value, result: &mut Validated) {
    let Some(name) = manifest.get("name").and_then(Value::as_str) else {
        return;
    };
    if name.is_empty() || !name.contains(|c: char| c.is_ascii_uppercase()) {
        return;
    }
    let suggested = suggest_kebab_name(name);
    result.publish_errors.push(format!(
        "Name \"{name}\" does not match the best practice (e.g. lower-cased/with-dashes). We \
         suggest using \"{suggested}\" instead. As such you will not be able to submit it to \
         Packagist."
    ));
}

fn check_deprecated_type(manifest: &Value, result: &mut Validated) {
    if manifest.get("type").and_then(Value::as_str) == Some("composer-installer") {
        result.warnings.push(
            "The package type 'composer-installer' is deprecated. Please distribute your custom \
             installers as plugins from now on. See \
             https://getcomposer.org/doc/articles/plugins.md for plugin documentation."
                .to_string(),
        );
    }
}

fn check_require_dev_overlap(manifest: &Value, result: &mut Validated) {
    let (Some(require), Some(require_dev)) = (
        manifest.get("require").and_then(Value::as_object),
        manifest.get("require-dev").and_then(Value::as_object),
    ) else {
        return;
    };
    let overlap: Vec<&str> = require
        .keys()
        .filter(|k| require_dev.contains_key(*k))
        .map(String::as_str)
        .collect();
    if overlap.is_empty() {
        return;
    }
    let plural = if overlap.len() > 1 { "are" } else { "is" };
    result.warnings.push(format!(
        "{} {plural} required both in require and require-dev, this can lead to unexpected \
         behavior",
        overlap.join(", ")
    ));
}

fn check_provide_replace_shadowing(manifest: &Value, result: &mut Validated) {
    for link_type in ["provide", "replace"] {
        let Some(links) = manifest.get(link_type).and_then(Value::as_object) else {
            continue;
        };
        for require_type in ["require", "require-dev"] {
            let Some(requires) = manifest.get(require_type).and_then(Value::as_object) else {
                continue;
            };
            for provide in links.keys() {
                if requires.contains_key(provide) {
                    result.warnings.push(format!(
                        "The package {provide} in {require_type} is also listed in {link_type} \
                         which satisfies the requirement. Remove it from {link_type} if you wish \
                         to install it."
                    ));
                }
            }
        }
    }
}

/// `array_merge($require, $requireDev)`: `require`'s key order, with a
/// `require-dev` duplicate overwriting the value in place rather than
/// moving or duplicating the entry.
fn check_commit_refs(manifest: &Value, result: &mut Validated) {
    let mut merged: Vec<(&str, &Value)> = Vec::new();
    for key in ["require", "require-dev"] {
        let Some(links) = manifest.get(key).and_then(Value::as_object) else {
            continue;
        };
        for (package, constraint) in links {
            if let Some(entry) = merged.iter_mut().find(|(p, _)| *p == package) {
                entry.1 = constraint;
            } else {
                merged.push((package.as_str(), constraint));
            }
        }
    }
    for (package, constraint) in merged {
        if constraint.as_str().is_some_and(|c| c.contains('#')) {
            result.warnings.push(format!(
                "The package \"{package}\" is pointing to a commit-ref, this is bad practice \
                 and can cause unforeseen issues."
            ));
        }
    }
}

fn check_scripts_shape(manifest: &Value, result: &mut Validated) {
    let scripts = manifest.get("scripts").and_then(Value::as_object);
    let has_script = |name: &str| scripts.is_some_and(|s| s.contains_key(name));

    if let Some(descriptions) = manifest
        .get("scripts-descriptions")
        .and_then(Value::as_object)
    {
        for name in descriptions.keys() {
            if !has_script(name) {
                result.warnings.push(format!(
                    "Description for non-existent script \"{name}\" found in \
                     \"scripts-descriptions\""
                ));
            }
        }
    }
    if let Some(aliases) = manifest.get("scripts-aliases").and_then(Value::as_object) {
        for name in aliases.keys() {
            if !has_script(name) {
                result.warnings.push(format!(
                    "Aliases for non-existent script \"{name}\" found in \"scripts-aliases\""
                ));
            }
        }
    }
}

fn check_empty_namespace_prefixes(manifest: &Value, result: &mut Validated) {
    for kind in ["psr-0", "psr-4"] {
        if manifest
            .pointer(&format!("/autoload/{kind}"))
            .and_then(Value::as_object)
            .is_some_and(|m| m.contains_key(""))
        {
            result.warnings.push(format!(
                "Defining autoload.{kind} with an empty namespace prefix is a bad idea for \
                 performance"
            ));
        }
    }
}

/// `ValidatingArrayLoader::load`'s own name handling: `validateString`
/// (must be present, must be a string) followed by
/// `hasPackageNamingError`, both under the `name : ` prefix.
/// `res/composer-schema.json`'s own `name` `pattern`, checked by
/// `JsonFile::validateSchema(JsonFile::LAX_SCHEMA)`: the one schema rule
/// kept (see module doc) because it fires *before* the hand-written
/// `hasPackageNamingError` message below and both show up together for a
/// `--with-dependencies` package (never for the root file: there, a bad
/// name aborts `composer validate` inside `RootPackageLoader` before either
/// message would print — a Factory/RootPackageLoader-style abort this port
/// doesn't reproduce).
const NAME_SCHEMA_PATTERN: &str =
    r"^[a-z0-9]([_.-]?[a-z0-9]+)*/[a-z0-9](([_.]|-{1,2})?[a-z0-9]+)*$";

/// The schema pattern above, case-sensitive (unlike `hasPackageNamingError`'s
/// own `iD`-flagged regex): an uppercase name fails this one but not that
/// one, surfacing only the schema message, not the naming-error message.
fn matches_name_schema_pattern(name: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(NAME_SCHEMA_PATTERN).unwrap());
    PATTERN.is_match(name)
}

fn check_name(manifest: &Value, result: &mut Validated) {
    match manifest.get("name") {
        None => {}
        Some(Value::String(name)) => {
            if !name.is_empty() && !matches_name_schema_pattern(name) {
                result.errors.push(format!(
                    "name : Does not match the regex pattern {NAME_SCHEMA_PATTERN}"
                ));
            }
            if name.trim().is_empty() {
                result.errors.push("name : must be present".to_string());
            }
            if let Some(err) = has_package_naming_error(name, false) {
                result.errors.push(format!("name : {err}"));
            }
        }
        Some(other) => result.errors.push(format!(
            "name : should be a string, {} given",
            debug_type(other)
        )),
    }
}

fn check_suggest(manifest: &Value, result: &mut Validated) {
    let Some(suggest) = manifest.get("suggest").and_then(Value::as_object) else {
        return;
    };
    for (package, description) in suggest {
        if description.as_str().is_none() {
            result.errors.push(format!(
                "suggest.{package} : invalid value, must be a string describing why the package \
                 is suggested"
            ));
        }
    }
}

/// `BasePackage::$supportedLinkTypes`' own order: require, conflict,
/// provide, replace, require-dev.
const LINK_TYPES: [&str; 5] = ["require", "conflict", "provide", "replace", "require-dev"];

/// `VersionParser::parseConstraints`'s own wrapping: `"Could not parse
/// version constraint $constraint: " . $e->getMessage()`. `semver_php`'s own
/// constraint-syntax errors (operators, whitespace) are coarser than PHP's
/// (see `semver.rs`'s module doc), but a single malformed version token
/// (the common case: `"not-a-constraint"`) fails inside `normalize()`, which
/// `crate::version::normalize` already reproduces byte-for-byte — prefer
/// that message when it also fails, and fall back to `semver_php`'s own
/// otherwise.
fn constraint_parse_error(spec: &str, err: &anyhow::Error) -> String {
    if let Err(normalize_err) = crate::version::normalize(spec) {
        return format!("Could not parse version constraint {spec}: {normalize_err}");
    }
    format!(
        "Could not parse version constraint {spec}: {}",
        err.root_cause()
    )
}

fn check_link_types(manifest: &Value, check_all: bool, result: &mut Validated) {
    let root_name = manifest.get("name").and_then(Value::as_str);
    let unbound_version = semver::normalize("10000000-dev").ok();

    for link_type in LINK_TYPES {
        let Some(links) = manifest.get(link_type).and_then(Value::as_object) else {
            continue;
        };
        for (package, constraint) in links {
            if root_name.is_some_and(|n| n.eq_ignore_ascii_case(package)) {
                result.errors.push(format!(
                    "{link_type}.{package} : a package cannot set a {link_type} on itself"
                ));
                continue;
            }
            if let Some(err) = has_package_naming_error(package, true) {
                result.warnings.push(format!("{link_type}.{err}"));
            } else if !is_valid_link_key(package) {
                result.errors.push(format!(
                    "{link_type}.{package} : invalid key, package names must be strings \
                     containing only [A-Za-z0-9_./-]"
                ));
            }

            let Some(constraint_str) = constraint.as_str() else {
                result.errors.push(format!(
                    "{link_type}.{package} : invalid value, must be a string containing a \
                     version constraint"
                ));
                continue;
            };
            if constraint_str == "self.version" {
                continue;
            }
            let parsed = match semver::parse_constraint(constraint_str) {
                Ok(parsed) => parsed,
                Err(err) => {
                    result.errors.push(format!(
                        "{link_type}.{package} : invalid version constraint ({})",
                        constraint_parse_error(constraint_str, &err)
                    ));
                    continue;
                }
            };
            if check_all
                && link_type == "require"
                && !is_platform_package(package)
                && let Some(unbound_version) = &unbound_version
                && parsed.matches(unbound_version)
            {
                result.warnings.push(format!(
                    "{link_type}.{package} : unbound version constraints ({constraint_str}) \
                     should be avoided"
                ));
            }
        }
    }
}

/// `Preg::isMatch('{^[A-Za-z0-9_./-]+$}', $package)`.
fn is_valid_link_key(package: &str) -> bool {
    !package.is_empty()
        && package
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-'))
}

/// `PlatformRepository::isPlatformPackage`, the subset `lock.rs` already
/// checks (kept as its own copy here since that one isn't `pub`).
fn is_platform_package(name: &str) -> bool {
    name == "php"
        || name.starts_with("php-")
        || name == "hhvm"
        || name.starts_with("ext-")
        || name.starts_with("lib-")
        || name.starts_with("composer-")
}

/// `ValidatingArrayLoader::hasPackageNamingError`'s reserved Windows device
/// names, checked against both the vendor and package half of the name.
const RESERVED_NAMES: [&str; 22] = [
    "nul", "con", "prn", "aux", "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8",
    "com9", "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
];

/// `ValidatingArrayLoader::hasPackageNamingError`.
fn has_package_naming_error(name: &str, is_link: bool) -> Option<String> {
    if is_platform_package(name) {
        return None;
    }
    if !valid_name_format(name) {
        return Some(format!(
            "{name} is invalid, it should have a vendor name, a forward slash, and a package \
             name. The vendor and package name can be words separated by -, . or _. The complete \
             name should match \
             \"^[a-z0-9]([_.-]?[a-z0-9]+)*/[a-z0-9](([_.]?|-{{0,2}})[a-z0-9]+)*$\"."
        ));
    }

    let lower = name.to_ascii_lowercase();
    let mut parts = lower.splitn(2, '/');
    let vendor = parts.next().unwrap_or_default();
    let package = parts.next().unwrap_or_default();
    if RESERVED_NAMES.contains(&vendor) || RESERVED_NAMES.contains(&package) {
        return Some(format!(
            "{name} is reserved, package and vendor names can not match any of: {}.",
            RESERVED_NAMES.join(", ")
        ));
    }

    // Not a filesystem path: `lower` is already the package name lower-cased,
    // so a plain suffix check is exact, not clippy's case-insensitivity concern.
    #[allow(clippy::case_sensitive_file_extension_comparisons)]
    let ends_in_json = lower.ends_with(".json");
    if ends_in_json {
        return Some(format!(
            "{name} is invalid, package names can not end in .json, consider renaming it or \
             perhaps using a -json suffix instead."
        ));
    }

    if name.contains(|c: char| c.is_ascii_uppercase()) {
        if is_link {
            return Some(format!(
                "{name} is invalid, it should not contain uppercase characters. Please use \
                 {lower} instead."
            ));
        }
        let suggested = suggest_kebab_name(name);
        return Some(format!(
            "{name} is invalid, it should not contain uppercase characters. We suggest using \
             {suggested} instead."
        ));
    }

    None
}

/// `Preg::isMatch('{^[a-z0-9](?:[_.-]?[a-z0-9]++)*+/[a-z0-9](?:(?:[_.]|-{1,2})?[a-z0-9]++)*+$}iD',
/// $name)`, case-insensitive.
fn valid_name_format(name: &str) -> bool {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r"(?i)^[a-z0-9](?:[_.-]?[a-z0-9]+)*/[a-z0-9](?:(?:[_.]|-{1,2})?[a-z0-9]+)*$",
        )
        .unwrap()
    });
    PATTERN.is_match(name)
}

/// `hasPackageNamingError`/`ConfigValidator`'s shared suggestion:
/// `Preg::replace('{(?:([a-z])([A-Z])|([A-Z])([A-Z][a-z]))}', '\1\3-\2\4', $name)`,
/// lower-cased.
fn suggest_kebab_name(name: &str) -> String {
    static PATTERN: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"([a-z])([A-Z])|([A-Z])([A-Z][a-z])").unwrap()
    });
    let replaced = PATTERN.replace_all(name, |caps: &regex::Captures| {
        let mut out = String::new();
        if let Some(m) = caps.get(1) {
            out.push_str(m.as_str());
        }
        if let Some(m) = caps.get(3) {
            out.push_str(m.as_str());
        }
        out.push('-');
        if let Some(m) = caps.get(2) {
            out.push_str(m.as_str());
        }
        if let Some(m) = caps.get(4) {
            out.push_str(m.as_str());
        }
        out
    });
    replaced.to_ascii_lowercase()
}

fn debug_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(n) if n.is_i64() || n.is_u64() => "int",
        Value::Number(_) => "float",
        Value::String(_) => "string",
        Value::Array(_) | Value::Object(_) => "array",
    }
}

/// `ValidateCommand::outputResult`: header line(s) plus `# General
/// errors`/`# General warnings`/`# Publish errors`/`# Lock file
/// errors|warnings` sections, each `- `-prefixed. Errors go to stderr like
/// Composer's `$io->writeError`; the plain "is valid" line goes to stdout.
fn output_result(
    name: &str,
    result: &mut Validated,
    check_publish: bool,
    check_lock: bool,
    lock_errors: &[String],
    print_schema_url: bool,
) {
    let mut do_print_schema_url = false;
    if !result.errors.is_empty() {
        err_out(&format!(
            "{name} is invalid, the following errors/warnings were found:"
        ));
    } else if !result.publish_errors.is_empty() && check_publish {
        err_out(&format!(
            "{name} is valid for simple usage with Composer but has"
        ));
        err_out("strict errors that make it unable to be published as a package");
        do_print_schema_url = print_schema_url;
    } else if !result.warnings.is_empty() {
        err_out(&format!("{name} is valid, but with a few warnings"));
        do_print_schema_url = print_schema_url;
    } else if !lock_errors.is_empty() {
        out(&format!(
            "{name} is valid but your composer.lock has some {}",
            if check_lock { "errors" } else { "warnings" }
        ));
    } else {
        out(&format!("{name} is valid"));
    }

    if do_print_schema_url {
        err_out("See https://getcomposer.org/doc/04-schema.md for details on the schema");
    }

    if !result.errors.is_empty() {
        err_out("# General errors");
        for err in &result.errors {
            err_out(&format!("- {err}"));
        }
    }

    let mut extra_warnings = Vec::new();
    if !result.publish_errors.is_empty() && check_publish {
        err_out("# Publish errors");
        for err in &result.publish_errors {
            err_out(&format!("- {err}"));
        }
        result.errors.append(&mut result.publish_errors);
    }
    if !lock_errors.is_empty() {
        if check_lock {
            err_out("# Lock file errors");
            for err in lock_errors {
                err_out(err);
            }
            result.errors.extend(lock_errors.iter().cloned());
        } else {
            err_out("# Lock file warnings");
            extra_warnings.extend(lock_errors.iter().cloned());
        }
    }

    if !result.warnings.is_empty() {
        err_out("# General warnings");
        for warning in &result.warnings {
            err_out(&format!("- {warning}"));
        }
    }
    for warning in &extra_warnings {
        err_out(warning);
    }
    result.warnings.extend(extra_warnings);
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}

/// stderr via `writeln!`, not `eprintln!`, to satisfy the `print_stderr` lint.
fn err_out(message: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::apply_fixable_fixes;

    /// #262: a package requiring itself is one of `--fix`'s six fixes, but
    /// never reachable as an ordinary `composer validate` finding on a
    /// *root* project — Composer 2.10.2's own `RootPackageLoader` aborts
    /// the whole command first (`In RootPackageLoader.php line 169: Root
    /// package '...' cannot require itself`), verified with `devbox run --
    /// composer validate` against this exact manifest, so
    /// `tests/fixtures/validate/fixable/` can't record a Composer oracle
    /// for it. Covered here directly instead.
    #[test]
    fn fix_deletes_a_self_require() {
        let mut root = json!({
            "name": "acme/selfreq",
            "require": {"php": ">=8.1", "acme/selfreq": "^1.0"},
        });
        let lines = apply_fixable_fixes(&mut root);
        assert_eq!(
            lines,
            vec!["Removed acme/selfreq from require, a package cannot set a require on itself"]
        );
        assert_eq!(
            root,
            json!({"name": "acme/selfreq", "require": {"php": ">=8.1"}})
        );
    }

    /// #262: same reachability caveat as above — an upper-case root name
    /// also aborts real Composer's `validate` before it would ever print
    /// the rename suggestion (verified the same way).
    #[test]
    fn fix_renames_an_uppercase_name() {
        let mut root = json!({"name": "Acme/UpperCase"});
        let lines = apply_fixable_fixes(&mut root);
        assert_eq!(
            lines,
            vec![
                "Renamed package to \"acme/upper-case\"; check anywhere the old name is \
                 referenced"
            ]
        );
        assert_eq!(root["name"], "acme/upper-case");
    }
}
