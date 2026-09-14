//! `config.bump-after-update`/`--bump-after-update` (#205): after an
//! `update` resolves, raise every root `require`/`require-dev` constraint
//! whose package was actually installed or upgraded this run to a caret (or
//! equivalent) constraint on its just-locked version, matching
//! `Composer\Command\BumpCommand::doBump`'s own filter (only
//! `$lockTransaction`'s `InstallOperation`/`UpdateOperation` targets are
//! eligible, not everything currently required) and
//! `Composer\Package\Version\VersionBumper::bumpRequirement`'s rewrite rule.
//!
//! Not ported: the standalone `composer bump` command (`update`'s own flag
//! and config cover the issue; a project wanting the same rewrite outside
//! an update can already get it by running one with `--bump-after-update`
//! and `--lock` off), branch-alias resolution for a package still locked to
//! a `dev-*` version (`VersionBumper`'s own `ArrayLoader::getBranchAlias`
//! lookup needs the provider file's `extra` block, which `ResolvedPackage`
//! doesn't carry — such a package is left untouched, same as upstream's own
//! "cannot be processed" bail with no alias), and multi-atom AND clauses
//! within one `||` branch (`>=1.0 <2.0`): only a clause that is a single
//! recognisable atom (`^`, `~`, `x`/`*` wildcard, `>=`, or a bare `*`) gets
//! rewritten, matching upstream everywhere requirements are actually
//! written this way in practice.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use regex::Regex;
use serde_json::Value;

use crate::semver;

/// Which requirement sections a `bump-after-update` value bumps
/// (`UpdateCommand::execute`'s own `$bumpAfterUpdate === 'dev'` ->
/// `require-dev` only, `'no-dev'` -> `require` only; anything else,
/// including bare `true`/`"all"`, bumps both).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Both,
    DevOnly,
    RequireOnly,
}

impl Scope {
    fn includes_require(self) -> bool {
        self != Scope::DevOnly
    }

    fn includes_require_dev(self) -> bool {
        self != Scope::RequireOnly
    }
}

/// Resolves the effective `bump-after-update` scope: `flag` (already
/// `Some` only when `--bump-after-update` was actually passed) wins over
/// `config.bump-after-update` off the raw root `Value`, same
/// flag-over-config layering `UpdateCommand::execute` does. `None` means
/// off (unset flag, and either no `config.bump-after-update` or an
/// explicit `false`).
pub fn resolve_scope(flag: Option<&str>, config: Option<&Value>) -> Option<Scope> {
    if let Some(flag) = flag {
        return Some(scope_of(flag));
    }
    match config? {
        Value::Bool(true) => Some(Scope::Both),
        Value::String(s) => Some(scope_of(s)),
        // `Value::Bool(false)` and anything else (a non-bool, non-string
        // `config.bump-after-update`) both mean off.
        _ => None,
    }
}

fn scope_of(value: &str) -> Scope {
    match value {
        "dev" => Scope::DevOnly,
        "no-dev" => Scope::RequireOnly,
        _ => Scope::Both,
    }
}

/// Bumps `root`'s `require`/`require-dev` in place, one constraint at a
/// time, for every package in `updated_names` (already-lowercased) that is
/// also a direct root requirement and has a locked version in
/// `locked_versions`. Platform packages are skipped
/// (`PlatformRepository::isPlatformPackage`, `BumpCommand::doBump`'s own
/// skip). Returns how many requirements actually changed.
#[expect(
    clippy::implicit_hasher,
    reason = "the only caller builds both maps straight off a solve result with the default \
              hasher; genericizing over BuildHasher buys nothing here"
)]
pub fn apply(
    root: &mut Value,
    scope: Scope,
    updated_names: &HashSet<String>,
    locked_versions: &HashMap<String, String>,
) -> Result<usize> {
    let mut changed = 0;
    for (key, included) in [
        ("require", scope.includes_require()),
        ("require-dev", scope.includes_require_dev()),
    ] {
        if !included {
            continue;
        }
        let Some(Value::Object(links)) = root.get_mut(key) else {
            continue;
        };
        for (name, constraint_value) in links.iter_mut() {
            let lower = name.to_ascii_lowercase();
            if crate::repository::is_platform_package(&lower) || !updated_names.contains(&lower) {
                continue;
            }
            let Some(locked_version) = locked_versions.get(&lower) else {
                continue;
            };
            let Some(constraint) = constraint_value.as_str() else {
                continue;
            };
            let bumped = bump_requirement(constraint, locked_version)?;
            if bumped != constraint {
                *constraint_value = Value::String(bumped);
                changed += 1;
            }
        }
    }
    Ok(changed)
}

/// `VersionBumper::bumpRequirement`, cut down as the module doc describes.
fn bump_requirement(pretty_constraint: &str, locked_pretty_version: &str) -> Result<String> {
    // A branch anywhere in the constraint, or a package still locked to
    // one, has no numeric lower bound to bump onto
    // (`Intervals::get($constraint)['branches']['names']` non-empty /
    // `str_starts_with($package->getVersion(), 'dev-')`).
    if pretty_constraint.contains("dev-") || locked_pretty_version.starts_with("dev-") {
        return Ok(pretty_constraint.to_string());
    }

    let normalized = semver::normalize(locked_pretty_version)?;
    let version = normalized.as_str();

    let major_re = Regex::new(r"^(?:([1-9][0-9]*)|(0\.[0-9]+))").expect("static regex");
    let Some(caps) = major_re.captures(version) else {
        return Ok(pretty_constraint.to_string());
    };
    let major = caps
        .get(1)
        .or_else(|| caps.get(2))
        .expect("one alternative always matches")
        .as_str();

    let strip_re = Regex::new(r"(?:\.(?:0|9999999))+(?:-dev)?$").expect("static regex");
    let version_without_suffix = strip_re.replace(version, "").into_owned();
    let new_pretty_constraint = format!("^{version_without_suffix}");

    // Not a simple stable version once the trailing zero padding is gone
    // (still carries a stability suffix Composer never bumps onto):
    // leave the requirement alone entirely, matching upstream's own abort.
    let simple_re = Regex::new(r"^\^[0-9]+(?:\.[0-9]+)*$").expect("static regex");
    if !simple_re.is_match(&new_pretty_constraint) {
        return Ok(pretty_constraint.to_string());
    }

    let major_pat = regex::escape(major);
    let caret_re = Regex::new(&format!(r"^\^v?{major_pat}(?:\.[0-9]+)*$")).expect("built regex");
    let tilde_re =
        Regex::new(&format!(r"^~v?{major_pat}(?:\.[0-9]+){{1,3}}$")).expect("built regex");
    let wildcard_re = Regex::new(&format!(r"^v?{major_pat}(?:\.[*x])+$")).expect("built regex");
    let gte_re = Regex::new(r"^>=v?[0-9]+(?:\.[0-9]+)*$").expect("static regex");

    let mut any_matched = false;
    let mut modified_segments = Vec::new();
    for segment in pretty_constraint.split("||") {
        let trimmed = segment.trim();
        let leading = &segment[..segment.len() - segment.trim_start().len()];
        let trailing = &segment[segment.trim_end().len()..];

        let dots = |s: &str| s.matches('.').count();
        let suffix = if dots(trimmed) == 2 && dots(&version_without_suffix) == 1 {
            ".0"
        } else {
            ""
        };

        let replacement = if tilde_re.is_match(trimmed) && dots(trimmed) != 1 {
            let width = dots(trimmed) + 1;
            let mut bits: Vec<&str> = version_without_suffix.split('.').collect();
            while bits.len() < width {
                bits.push("0");
            }
            Some(format!("~{}", bits[..width].join(".")))
        } else if trimmed == "*" || gte_re.is_match(trimmed) {
            Some(format!(">={version_without_suffix}{suffix}"))
        } else if caret_re.is_match(trimmed)
            || tilde_re.is_match(trimmed)
            || wildcard_re.is_match(trimmed)
        {
            Some(format!("{new_pretty_constraint}{suffix}"))
        } else {
            None
        };

        match replacement {
            Some(replacement) => {
                any_matched = true;
                modified_segments.push(format!("{leading}{replacement}{trailing}"));
            }
            None => modified_segments.push(segment.to_string()),
        }
    }

    if !any_matched {
        return Ok(pretty_constraint.to_string());
    }
    let modified = modified_segments.join("||");

    // Equivalent to the original interval-for-interval: no real change.
    let old_constraint = semver::parse_constraint(pretty_constraint)?;
    let new_constraint = semver::parse_constraint(&modified)?;
    if semver::is_subset_of(&new_constraint, &old_constraint)
        && semver::is_subset_of(&old_constraint, &new_constraint)
    {
        return Ok(pretty_constraint.to_string());
    }
    Ok(modified)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `VersionBumper::bumpRequirement`'s own worked examples
    /// (`VersionBumper.php`'s doc comment), minus the two this port
    /// doesn't cover (`dev-master` locked without a branch alias, and the
    /// standalone `bump` command isn't in scope).
    #[test]
    fn bump_requirement_matches_upstream_worked_examples() {
        assert_eq!(bump_requirement("^1.0", "1.2.1").unwrap(), "^1.2.1");
        assert_eq!(bump_requirement("^1.2", "1.2.0").unwrap(), "^1.2");
        assert_eq!(bump_requirement("^1.2.0", "1.3.0").unwrap(), "^1.3.0");
        assert_eq!(
            bump_requirement("^1.2 || ^2.3", "1.3.0").unwrap(),
            "^1.3 || ^2.3"
        );
        assert_eq!(
            bump_requirement("^1.2 || ^2.3", "2.4.0").unwrap(),
            "^1.2 || ^2.4"
        );
        assert_eq!(bump_requirement("~2", "2.0-beta.1").unwrap(), "~2");
        assert_eq!(bump_requirement("~2.0.0", "2.0.3").unwrap(), "~2.0.3");
        assert_eq!(bump_requirement("~2.0", "2.0.3").unwrap(), "^2.0.3");
        assert_eq!(bump_requirement("*", "1.2.3").unwrap(), ">=1.2.3");
    }

    #[test]
    fn bump_requirement_leaves_a_dev_locked_package_untouched() {
        assert_eq!(bump_requirement("^1.0", "dev-main").unwrap(), "^1.0");
    }

    #[test]
    fn resolve_scope_prefers_the_flag_over_config() {
        assert_eq!(
            resolve_scope(Some("dev"), Some(&Value::Bool(true))),
            Some(Scope::DevOnly)
        );
        assert_eq!(resolve_scope(None, Some(&Value::Bool(false))), None);
        assert_eq!(resolve_scope(None, None), None);
        assert_eq!(
            resolve_scope(None, Some(&Value::String("no-dev".into()))),
            Some(Scope::RequireOnly)
        );
    }

    #[test]
    fn apply_skips_a_package_not_touched_this_update() {
        let mut root = serde_json::json!({"require": {"monolog/monolog": "^3.0"}});
        let updated = HashSet::new();
        let locked = HashMap::from([("monolog/monolog".to_string(), "3.12.0".to_string())]);
        let changed = apply(&mut root, Scope::Both, &updated, &locked).unwrap();
        assert_eq!(changed, 0);
        assert_eq!(root["require"]["monolog/monolog"], "^3.0");
    }

    #[test]
    fn apply_bumps_the_reproduction_from_205() {
        let mut root = serde_json::json!({"require": {"monolog/monolog": "^3.0"}});
        let updated = HashSet::from(["monolog/monolog".to_string()]);
        let locked = HashMap::from([("monolog/monolog".to_string(), "3.12.0".to_string())]);
        let changed = apply(&mut root, Scope::Both, &updated, &locked).unwrap();
        assert_eq!(changed, 1);
        assert_eq!(root["require"]["monolog/monolog"], "^3.12");
    }
}
