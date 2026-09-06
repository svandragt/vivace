//! A port of `Problem.php`/`SolverProblemsException.php` good enough to
//! name the packages and constraints an unsatisfiable request involved in
//! Composer's own wording, for the four shapes `docs/resolver-design.md`
//! stage 5 (composer/composer#42) calls out: an unsatisfiable root
//! requirement, a conflict between two requirements, a package that cannot
//! be found at all, and a platform requirement. Not ported:
//! `formatDeduplicatedRules`'s version-range collapsing
//! (`[1.0.0, 1.2.0]`-style grouping across near-identical messages),
//! security-advisory/abandoned/filter-list wording, and
//! `computeCheckForLowerPrioRepo`'s repository-priority diagnostics —
//! vivace has one repository, so that class of message never applies.
//!
//! `Problem` collects `Reason`s exactly as before; the pretty-printing that
//! used to live in a `Display` impl now needs `&Pool` (to look up a
//! package's pretty name/version and to ask whether a name exists at all),
//! so it moved to [`Problem::pretty_string`], called once at
//! `solver::solve`'s only `Err(SolverError { .. })` site while `pool` is
//! still in scope (`SolverError` itself now carries pre-rendered strings,
//! not `Problem`s, since a `Problem` has no lifetime of its own to borrow
//! `Pool` across the `?`/`Result` boundary).

use std::fmt;

use crate::solver::pool::{self, Pool};
use crate::solver::rules::Reason;

/// One `Problem`: every [`Reason`] the solver could trace back from an
/// unsatisfiable rule (`Rule::RULE_PACKAGE` rules never reach here, matching
/// `analyze_unsolvable_rule`'s "package rules cannot be part of a problem").
#[derive(Default)]
pub struct Problem {
    reasons: Vec<Reason>,
}

impl Problem {
    pub fn new() -> Problem {
        Problem::default()
    }

    pub fn add_reason(&mut self, reason: Reason) {
        self.reasons.push(reason);
    }

    /// `Problem::getPrettyString`. Composer builds this bottom-up from
    /// `array_merge(...array_reverse($this->reasons))` (sections in
    /// reverse, each section's own order preserved); `add_reason` never
    /// sections here (no `nextSection` caller in this port), so replaying
    /// `self.reasons` in insertion order is the same list.
    pub fn pretty_string(&self, pool: &Pool) -> String {
        // `getPrettyString`'s single-reason fast path: a lone
        // `RULE_ROOT_REQUIRE` with *zero* pool matches for the name (any
        // version, ignoring the constraint that made it unsatisfiable)
        // short-circuits straight to `getMissingPackageReason`. A name that
        // does exist falls through to the general per-reason list below,
        // same as every other reason count.
        if let [
            Reason::RootRequire {
                package_name,
                pretty_constraint,
            },
        ] = self.reasons.as_slice()
            && pool.what_provides(package_name, None).is_empty()
        {
            return format!(
                "\n    {}",
                missing_package_reason(pool, package_name, pretty_constraint)
            );
        }

        let mut lines: Vec<String> = self
            .reasons
            .iter()
            .map(|reason| reason_line(pool, reason))
            .filter(|line| !line.is_empty())
            .collect();
        lines.dedup();
        format!("\n    - {}", lines.join("\n    - "))
    }
}

/// `Rule::getPrettyString`, the lines `formatDeduplicatedRules` joins with
/// `"\n    - "` (the version-range dedup itself is not ported, see the
/// module doc).
fn reason_line(pool: &Pool, reason: &Reason) -> String {
    match reason {
        Reason::RootRequire {
            package_name,
            pretty_constraint,
        } => {
            let packages = pool.what_provides(package_name, None);
            if packages.is_empty() {
                return format!(
                    "No package found to satisfy root composer.json require {package_name} \
                     {pretty_constraint}"
                );
            }
            format!(
                "Root composer.json requires {package_name} {pretty_constraint} -> satisfiable \
                 by {}.",
                package_list(pool, &packages)
            )
        }
        Reason::Fixed { package_index } => {
            let package = pool.package_by_id(pool::id_of(*package_index));
            format!(
                "{} is present at version {} and cannot be modified by Composer",
                package.name, package.pretty_version
            )
        }
        Reason::PackageConflict {
            target_index,
            target,
            pretty_constraint,
            ..
        } => {
            let conflicter = pool.package_by_id(pool::id_of(*target_index));
            format!(
                "{} conflicts with {target} {pretty_constraint}.",
                conflicter.pretty_string()
            )
        }
        Reason::PackageRequires {
            source_index,
            target,
            pretty_constraint,
        } => {
            let source = pool.package_by_id(pool::id_of(*source_index));
            let providers = pool.what_provides(target, None);
            if providers.is_empty() {
                format!(
                    "{} requires {target} {pretty_constraint} -> {}",
                    source.pretty_string(),
                    missing_package_suffix(pool, target)
                )
            } else {
                format!(
                    "{} requires {target} {pretty_constraint} -> satisfiable by {}.",
                    source.pretty_string(),
                    package_list(pool, &providers)
                )
            }
        }
        Reason::PackageSameName(name) => {
            format!("Only one package named {name} can be installed at a time")
        }
        Reason::PackageAlias { .. } | Reason::PackageInverseAlias { .. } | Reason::Learned(_) => {
            // Not a leaf reason this port's four message shapes need to
            // spell out: `analyze_unsolvable_rule` already recurses through
            // `Learned` down to the request-level reasons that actually
            // caused the conflict, and the two alias reasons only ever
            // co-occur with one of those in practice.
            String::new()
        }
    }
}

/// `Problem::getMissingPackageReason`'s `[prefix, suffix]` tuple, joined:
/// the platform-package and found-but-mismatched-constraint branches this
/// stage's fixtures exercise, plus the final could-not-be-found-at-all
/// fallback. The security-advisory/abandoned/filter-list/lower-priority-repository
/// branches are not reached (single-repository, no security data). Each
/// branch has its own prefix shape (a not-found-at-all package gets no
/// constraint text at all in the message, unlike the other two), so this
/// isn't a single shared template.
fn missing_package_reason(pool: &Pool, package_name: &str, pretty_constraint: &str) -> String {
    if crate::repository::is_platform_package(package_name) {
        return format!(
            "- Root composer.json requires {package_name} {pretty_constraint} but {}",
            missing_package_suffix(pool, package_name)
        );
    }
    if pool.what_provides(package_name, None).is_empty() {
        return format!(
            "- Root composer.json requires {package_name}, it {}",
            missing_package_suffix(pool, package_name)
        );
    }
    format!(
        "- Root composer.json requires {package_name} {pretty_constraint}, {}",
        missing_package_suffix(pool, package_name)
    )
}

/// The suffix half of `getMissingPackageReason`'s `[prefix, suffix]` tuple:
/// also reused standalone by `Reason::PackageRequires`'s `-> ...` tail
/// (`Rule::getPrettyString`'s own `$text . ' -> ' . $reason[1]`, suffix
/// only, no prefix).
fn missing_package_suffix(pool: &Pool, package_name: &str) -> String {
    if crate::repository::is_platform_package(package_name) {
        let installed = pool.what_provides(package_name, None);
        if let Some(&id) = installed.first() {
            let package = pool.package_by_id(id);
            return format!(
                "your {package_name} version ({}) does not satisfy that requirement.",
                package.pretty_version
            );
        }
        return format!("{package_name} is missing from your platform.");
    }

    let any_version = pool.what_provides(package_name, None);
    if any_version.is_empty() {
        return "could not be found in any version, there may be a typo in the package name."
            .to_string();
    }
    format!(
        "found {} but it does not match the constraint.",
        package_list(pool, &any_version)
    )
}

/// `Problem::getPackageList`, without the verbose/removed-version-group
/// bracketing: `name[v1, v2, ...]`, versions in pool insertion order
/// (already Composer's own newest-first provider order for a typical
/// Packagist feed, close enough for a message rather than a solver
/// decision).
fn package_list(pool: &Pool, ids: &[i32]) -> String {
    let mut by_name: Vec<(String, Vec<String>)> = Vec::new();
    for &id in ids {
        let package = pool.package_by_id(id);
        match by_name.iter_mut().find(|(name, _)| *name == package.name) {
            Some((_, versions)) => versions.push(package.pretty_version.clone()),
            None => by_name.push((package.name.clone(), vec![package.pretty_version.clone()])),
        }
    }
    by_name
        .into_iter()
        .map(|(name, versions)| format!("{name}[{}]", versions.join(", ")))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The solver's public error: one or more [`Problem`]s, pre-rendered
/// against the pool that produced them (`SolverError` used to hold
/// `Problem`s directly and implement `Display` itself; see the module
/// doc for why that moved to `Problem::pretty_string`).
#[derive(Default)]
pub struct SolverError {
    pub problems: Vec<String>,
}

impl SolverError {
    pub fn from_problems(problems: &[Problem], pool: &Pool) -> SolverError {
        SolverError {
            problems: problems.iter().map(|p| p.pretty_string(pool)).collect(),
        }
    }
}

/// `SolverProblemsException::getPrettyString`'s "Potential causes" hint,
/// appended verbatim whenever any problem's text contains either phrase
/// (`str_contains($text, 'could not be found') ||
/// str_contains($text, 'no matching package found')`). The other hints
/// there (missing PHP extensions, `--with-all-dependencies`, two
/// ocramius/package-versions special cases) all depend on data this port
/// doesn't track (`isCausedByLock`, extension detection) and are not
/// reached by this stage's fixtures.
const TYPO_HINT: &str = "Potential causes:\n - A typo in the package name\n - The package is not \
     available in a stable-enough version according to your minimum-stability setting\n   see \
     <https://getcomposer.org/doc/04-schema.md#minimum-stability> for more details.\n - It's a \
     private package and you forgot to add a custom repository to find it\n\nRead \
     <https://getcomposer.org/doc/articles/troubleshooting.md> for further common problems.";

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The blank line here is Composer's own: the fixed intro sentence a
        // command prints, then `SolverProblemsException::getPrettyString`'s
        // text, which itself starts with `"\n"` before the first `  Problem`.
        writeln!(
            f,
            "Your requirements could not be resolved to an installable set of packages.\n"
        )?;
        let mut any_not_found = false;
        for (i, problem) in self.problems.iter().enumerate() {
            // `problem` already starts with its own leading `\n` (`Problem::
            // getPrettyString`'s `"\n    ".implode(...)`/`"\n    - "` lead-in),
            // so `"  Problem N"` takes no newline of its own here — one
            // `writeln!` would double it into a blank line Composer's real
            // output never has.
            writeln!(f, "  Problem {}{problem}", i + 1)?;
            any_not_found |= problem.contains("could not be found")
                || problem.contains("no matching package found");
        }
        if any_not_found {
            writeln!(f, "\n{TYPO_HINT}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl std::error::Error for SolverError {}
