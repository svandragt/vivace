//! `viv validate`: a native `ValidateCommand`/`ConfigValidator` port for
//! `composer.json`, plus `composer.lock` freshness/completeness (via
//! [`crate::lock::is_fresh`]/[`crate::lock::missing_requirements`]).
//!
//! `ConfigValidator` layers three things vivace keeps apart:
//! `JsonFile::validateSchema` (a `justinrainbow/json-schema` walk over
//! `res/composer-schema.json`), `ConfigValidator`'s own hand-written checks,
//! and `ValidatingArrayLoader`'s hand-written checks (run as part of loading
//! a package from the parsed manifest). The schema pass mostly duplicates
//! what the hand-written checks already catch (a wrong JSON type, a missing
//! required key) and needs a JSON-schema validator dependency to reproduce
//! byte-for-byte (`justinrainbow/json-schema`'s own error wording); this
//! port skips it and sticks to the hand-written checks, the two commands'
//! actual behavioural surface.
//!
//! Ported: name format (`ValidatingArrayLoader::hasPackageNamingError`,
//! root and every link type), the publish-only uppercase-name suggestion,
//! missing-license and version-field-present warnings, deprecated
//! `composer-installer` type, require/require-dev overlap, provide/replace
//! shadowing a requirement, commit-ref requires, `scripts-descriptions`/
//! `scripts-aliases` naming non-existent scripts, empty PSR-0/PSR-4
//! prefixes, and the link-type loop's own checks (self-reference, key
//! format, constraint parse, unbound-constraint warning).
//!
//! Not ported (no fixture needs it yet, add alongside one that does):
//! JSON-schema-driven type errors beyond what the checks above already
//! catch, SPDX license *validity* (only "is one set at all" is checked),
//! duplicate-key detection, `authors`/`support`/`funding`/`php-ext`/
//! `autoload`/`minimum-stability`/`source`/`dist`/`extra.branch-alias`
//! validation, and the strict-vs-unbound-constraint (`CHECK_STRICT_CONSTRAINTS`)
//! and match-nothing (`Intervals::compactConstraint`) warnings.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use serde_json::Value;

use crate::lock;
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
    /// in the current directory).
    pub file: Option<PathBuf>,
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
    // `Factory::getComposerFile()`'s own default: `getenv('COMPOSER') ?:
    // './composer.json'` (env override not supported here, no fixture needs
    // it), printed verbatim in every message so the leading `./` matters.
    let file = args
        .file
        .clone()
        .unwrap_or_else(|| PathBuf::from("./composer.json"));

    let Ok(bytes) = fs_err::read(&file) else {
        err_out(&format!("{} not found.", file.display()));
        return Ok(EXIT_UNREADABLE);
    };
    let Ok(manifest) = serde_json::from_slice::<Value>(&bytes) else {
        err_out(&format!("{} does not contain valid JSON.", file.display()));
        return Ok(EXIT_UNREADABLE);
    };

    let check_all = !args.no_check_all;
    let mut result = validate_manifest(&manifest, check_all);

    let check_publish = !args.no_check_publish;
    let project_dir = file
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
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
    let mut exit_code = exit_for(&result, args.strict);

    if args.with_dependencies {
        exit_code = exit_code.max(validate_dependencies(
            project_dir,
            check_all,
            args,
            check_publish,
        )?);
    }

    Ok(exit_code)
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
        lock_errors.push(
            "- The lock file is not up to date with the latest changes in composer.json, it is \
             recommended that you run `composer update` or `composer update <package name>`."
                .to_string(),
        );
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
/// hand-written checks, minus the JSON-schema pass (see module doc).
fn validate_manifest(manifest: &Value, check_all: bool) -> Validated {
    let mut result = Validated::default();

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
