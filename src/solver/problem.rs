//! A blunt stand-in for `Problem.php`/`SolverProblemsException.php`: names
//! the packages and constraints an unsatisfiable request involved, without
//! `Problem.php`'s dependency-tree-aware pretty printing (grouped
//! "Problem 1/2/...", `->` chains through replace/provide, verbose
//! per-package lists). That full port is stage 5 (composer/composer#42);
//! this stage only needs a message that names the offending package.

use std::fmt;

use crate::solver::rules::Reason;

/// One `Problem`: every [`Reason`] the solver could trace back from an
/// unsatisfiable rule, in Composer's own `TYPE_PACKAGE` rules cannot be
/// part of a problem" sense filtered out already (see `solver.rs`'s
/// `analyze_unsolvable_rule`).
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
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.reasons.is_empty() {
            return write!(f, "the dependency graph is unsatisfiable");
        }
        for (i, reason) in self.reasons.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            match reason {
                Reason::RootRequire {
                    package_name,
                    pretty_constraint,
                } => {
                    write!(
                        f,
                        "No package found to satisfy root composer.json require {package_name} {pretty_constraint}"
                    )?;
                }
                Reason::Fixed { package_index } => {
                    write!(
                        f,
                        "  - platform package at pool index {package_index} is required and cannot be modified"
                    )?;
                }
                Reason::PackageConflict {
                    target,
                    pretty_constraint,
                    ..
                } => {
                    write!(f, "  - conflicts with {target} {pretty_constraint}")?;
                }
                Reason::PackageRequires {
                    target,
                    pretty_constraint,
                    ..
                } => {
                    write!(
                        f,
                        "  - requires {target} {pretty_constraint} -> no matching package found"
                    )?;
                }
                Reason::PackageSameName(name) => {
                    write!(
                        f,
                        "  - only one package named {name} can be installed at a time"
                    )?;
                }
                Reason::PackageAlias { .. }
                | Reason::PackageInverseAlias { .. }
                | Reason::Learned(_) => {
                    // Not a leaf reason a blunt message needs to spell out:
                    // `analyze_unsolvable_rule` already recurses through
                    // `Learned` down to the request-level reasons that
                    // actually caused the conflict.
                }
            }
        }
        Ok(())
    }
}

/// The solver's public error: one or more [`Problem`]s, matching
/// `SolverProblemsException` holding a list rather than a single message.
#[derive(Default)]
pub struct SolverError {
    pub problems: Vec<Problem>,
}

impl fmt::Display for SolverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "Your requirements could not be resolved to an installable set of packages."
        )?;
        for (i, problem) in self.problems.iter().enumerate() {
            writeln!(f, "  Problem {}", i + 1)?;
            writeln!(f, "{problem}")?;
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
