//! Thin facade over `semver_php`, a port of `composer/semver`'s
//! `VersionParser`, `Constraint`, `MultiConstraint`, `Interval` and
//! `Intervals`. Gated on the corpus in `tests/fixtures/composer/semver/`
//! (see `docs/resolver-design.md`'s "Constraints and versions" section)
//! rather than trusted outright: it has one release and few downloads, so
//! this module is the whole surface a fork or vendor would need to replace.

use std::cmp::Ordering;

use anyhow::{Context, Result};
use semver_php::{Operator, SingleConstraint, VersionParser};

/// A Composer-normalised version string (`VersionParser::normalize`'s
/// output), the shape `Constraint::matches` expects on the right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedVersion(String);

impl NormalizedVersion {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// `semver-php` 0.1.0 indexes constraint/version strings by byte offset
/// without checking char-boundary alignment, so a non-ASCII byte (which
/// only ever sits mid-char in UTF-8) can panic instead of erroring. Reject
/// it up front with Composer's own `VersionParser` wording rather than
/// delegating into that crash.
fn reject_non_ascii(constraint: &str) -> Result<()> {
    if constraint.is_ascii() {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "Could not parse version constraint {constraint}: Invalid version string \"{constraint}\""
    ))
}

/// Normalise a Composer version string to its canonical four-component
/// form. Delegates to `crate::version::normalize`, which already passes the
/// composer/semver normalise corpus.
pub fn normalize(version: &str) -> Result<NormalizedVersion> {
    reject_non_ascii(version)?;
    Ok(NormalizedVersion(crate::version::normalize(version)?))
}

/// A parsed version constraint (`VersionParser::parseConstraints`).
pub struct Constraint(Box<dyn semver_php::Constraint>);

impl Constraint {
    /// Whether `version` satisfies this constraint (`Semver::satisfies`:
    /// the version becomes a single `==` constraint, matched against self).
    pub fn matches(&self, version: &NormalizedVersion) -> bool {
        let point = SingleConstraint::new(Operator::Eq, version.as_str());
        self.0.matches(&point)
    }
}

impl std::fmt::Display for Constraint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Parse a constraint expression (`^1.0`, `>=2.0 <3.0`, `~1.2 || ^2.0`, ...).
pub fn parse_constraint(spec: &str) -> Result<Constraint> {
    reject_non_ascii(spec)?;
    VersionParser::parse_constraints(spec)
        .map(Constraint)
        .with_context(|| format!("parsing version constraint {spec:?}"))
}

/// Composer's `Comparator`: a version ordering that is stability- and
/// dev-branch-aware, not a plain string or numeric compare. Both sides
/// must already be normalised: Composer only ever compares normalised
/// versions (`Package::getVersion`, lock entries).
///
/// ponytail: two *different* arbitrary dev branches (`dev-foo` vs
/// `dev-bar`, neither a numeric alias nor `dev-master`/`dev-trunk`) have no
/// order in Composer at all: `greaterThan`, `lessThan` and `equalTo` are
/// all `false` for that pair. `Ordering` has no fourth "incomparable"
/// variant, so this folds that case into `Equal`; the mismatch only bites a
/// direct compare between two unrelated feature branches, never a solver
/// decision (which compares against `dev-master`-style aliases or numeric
/// constraints). Widen this if a resolver stage needs the distinction.
pub fn compare(a: &NormalizedVersion, b: &NormalizedVersion) -> Ordering {
    if semver_php::greater_than(a.as_str(), b.as_str()).unwrap_or(false) {
        Ordering::Greater
    } else if semver_php::less_than(a.as_str(), b.as_str()).unwrap_or(false) {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

/// `VersionParser::parseStability`: `"dev"`, `"alpha"`, `"beta"`, `"RC"` or
/// `"stable"`.
pub fn stability(version: &str) -> &'static str {
    VersionParser::parse_stability(version).as_str()
}

/// Whether `candidate` is a subset of `constraint` (`Intervals::isSubsetOf`).
pub fn is_subset_of(candidate: &Constraint, constraint: &Constraint) -> bool {
    semver_php::Intervals::new().is_subset_of(candidate.0.as_ref(), constraint.0.as_ref())
}

/// Whether two constraints overlap at all (`Intervals::haveIntersections`).
pub fn have_intersections(a: &Constraint, b: &Constraint) -> bool {
    semver_php::Intervals::new().have_intersections(a.0.as_ref(), b.0.as_ref())
}

/// `VersionParser::normalizeBranch`: `v1.x` / `2.0.*` -> `9999999`-filled
/// dev version, anything else -> `dev-<name>`.
pub fn normalize_branch(name: &str) -> String {
    VersionParser::normalize_branch(name)
}

/// `VersionParser::parseNumericAliasPrefix`: the numeric prefix a branch
/// like `2.x-dev` aliases to (`"2."`), or `None` for a non-numeric branch.
pub fn parse_numeric_alias_prefix(branch: &str) -> Option<String> {
    VersionParser::parse_numeric_alias_prefix(branch)
}
