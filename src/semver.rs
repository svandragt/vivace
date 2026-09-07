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

/// Wraps a string the caller already knows is in `normalize`'s canonical
/// form (Packagist's own `version_normalized` field, or
/// `crate::version::normalize_branch`'s output), skipping `normalize`'s
/// regex passes entirely — ~300 ns/call measured down to ~10 ns/call (#120:
/// `ClosureWalk::process` re-normalising an already-normalized
/// `version_normalized` on every rescan of every version, over every
/// package in a closure, was part of that walk's synchronous CPU). Still
/// runs the ASCII guard: an untrusted repository could ship a bogus value
/// here.
pub(crate) fn from_normalized(version: String) -> Result<NormalizedVersion> {
    reject_non_ascii(&version)?;
    Ok(NormalizedVersion(version))
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

/// #76 follow-up: `Constraint::matches` costs ~775 ns/call in isolation
/// (`bench/results/profile.md` §2.6) — it re-allocates a `SingleConstraint`
/// and walks the whole parsed constraint tree every call, dynamic dispatch
/// included. `pool_optimizer.rs` calls it millions of times against a large
/// closure (many versions × many distinct requiring constraints), so that
/// cost alone dominates `viv update`. `CompiledConstraint` amortises it:
/// build once per distinct constraint string (`CompiledConstraint::compile`,
/// itself calling `Intervals::get` once), match many times via
/// [`CompiledConstraint::matches`] against a [`VersionKey`] pre-parsed once
/// per package version (`parse_version_key`) — a handful of integer
/// comparisons, no string work, no allocation, mirroring Composer's own
/// `CompilingMatcher`.
///
/// Dev branches (`dev-<name>`, as opposed to a numeric version that merely
/// carries a `-dev` stability suffix) are not modelled here: `Intervals::get`
/// already separates its `IntervalResult` into a `numeric` field and an
/// orthogonal `branches` field precisely because a branch name isn't a point
/// on the numeric line, and this module only compiles the `numeric` side.
/// [`VersionKey`] tags a branch version instead of parsing it, and
/// [`CompiledConstraint::matches`] returns `None` for one — the caller falls
/// back to `Constraint::matches` (`pool_optimizer.rs` does exactly this),
/// which stays exactly as correct as before for that rare case.
///
/// One `Part` per dot/dash/underscore-separated version segment
/// (`version_compare`'s own `split_version`), classified once
/// (`classify_part`) instead of re-parsed on every comparison.
/// `stability_order_for_empty_check` exists only for [`compare_slice`]'s
/// "one side ran out of segments" branch: `version_compare` resolves that
/// case with a *different* stability test (`stability_order` on the whole,
/// unsplit segment — an exact keyword match) than the "both sides present"
/// case (`stability_order` on the segment's own already-split prefix), so a
/// segment like `"beta2"` needs both answers cached (prefix match: a
/// beta-numbered prerelease; exact match: not a bare keyword, since
/// `"beta2" != "beta"`) to reproduce `version_compare` bit for bit.
#[derive(Debug, Clone)]
struct Part {
    kind: PartKind,
    stability_order_for_empty_check: i32,
}

#[derive(Debug, Clone)]
enum PartKind {
    Num(i64),
    /// Already lower-cased: `version_compare`'s plain-string fallback
    /// compares case-insensitively.
    Str(String),
    /// A stability prefix (`alpha`/`beta`/`rc`/`dev`/`patch`, plus the
    /// short forms `a`/`b`/`p`/`pl`) with its trailing number, if any.
    Stability(i32, Option<i64>),
}

/// `version.rs`'s private `split_stability_and_number`, longest/most-specific
/// prefix first so `"beta2"` matches `"beta"` rather than falling through to
/// nothing (there's no length-based tie among these particular prefixes, but
/// order still matters for `"pl"` vs `"p"` and `"rc"` never colliding with
/// the others).
const STABILITY_PREFIXES: [(&str, &str); 9] = [
    ("alpha", "alpha"),
    ("beta", "beta"),
    ("patch", "patch"),
    ("dev", "dev"),
    ("rc", "RC"),
    ("pl", "patch"),
    ("a", "alpha"),
    ("b", "beta"),
    ("p", "patch"),
];

fn classify_part(token: &str) -> Part {
    let lower = token.to_ascii_lowercase();
    let stability_order_for_empty_check = semver_php::stability_order(&lower);
    for (prefix, canonical) in STABILITY_PREFIXES {
        if lower.starts_with(prefix) {
            // `version.rs` slices the *original* token by the ASCII
            // prefix's byte length, not the lower-cased copy; every prefix
            // here is ASCII, so the byte offset is identical either way and
            // slicing `token` directly stays char-boundary safe.
            let rest = token[prefix.len()..].trim_start_matches(['.', '-']);
            let num = rest.parse::<i64>().ok();
            return Part {
                kind: PartKind::Stability(semver_php::stability_order(canonical), num),
                stability_order_for_empty_check,
            };
        }
    }
    let kind = token
        .parse::<i64>()
        .map_or_else(|_| PartKind::Str(lower), PartKind::Num);
    Part {
        kind,
        stability_order_for_empty_check,
    }
}

/// `version.rs`'s private `split_version`: dot/dash/underscore-separated,
/// empty segments dropped (consecutive separators don't produce them).
fn parse_parts(version: &str) -> Vec<Part> {
    version
        .split(['.', '-', '_'])
        .filter(|part| !part.is_empty())
        .map(classify_part)
        .collect()
}

/// The paired-classification order/number `version.rs`'s non-empty branch
/// compares by: `(0, None)` for a plain number or string (never a stability
/// marker there, or the prefix loop above would have classified it as one).
fn order_num(part: &Part) -> (i32, Option<i64>) {
    match part.kind {
        PartKind::Stability(order, num) => (order, num),
        PartKind::Num(_) | PartKind::Str(_) => (0, None),
    }
}

/// `version.rs`'s private `compare_parts`, operating on already-classified
/// [`Part`]s instead of re-classifying a fresh `&str` pair every call.
fn compare_parts(a: &Part, b: &Part) -> Ordering {
    let (order_a, num_a) = order_num(a);
    let (order_b, num_b) = order_num(b);
    if order_a != 0 || order_b != 0 {
        if order_a != order_b {
            return order_a.cmp(&order_b);
        }
        return match (num_a, num_b) {
            (Some(x), Some(y)) => x.cmp(&y),
            (Some(_), None) => Ordering::Greater,
            (None, Some(_)) => Ordering::Less,
            (None, None) => Ordering::Equal,
        };
    }
    match (&a.kind, &b.kind) {
        (PartKind::Num(x), PartKind::Num(y)) => x.cmp(y),
        (PartKind::Num(_), PartKind::Str(_)) => Ordering::Greater,
        (PartKind::Str(_), PartKind::Num(_)) => Ordering::Less,
        (PartKind::Str(x), PartKind::Str(y)) => x.cmp(y),
        (PartKind::Stability(..), _) | (_, PartKind::Stability(..)) => {
            unreachable!("stability parts are handled above")
        }
    }
}

/// `version.rs`'s `version_compare`'s "one side ran out of segments" branch:
/// treats the missing side as `""`, which the *other* side's own
/// `stability_order` (an exact keyword match, not the prefix match
/// `compare_parts` uses) breaks the tie against.
fn compare_part_opt(a: Option<&Part>, b: Option<&Part>) -> Ordering {
    match (a, b) {
        (None, None) => Ordering::Equal,
        (None, Some(p)) => {
            if p.stability_order_for_empty_check != 0 {
                Ordering::Greater
            } else {
                Ordering::Less
            }
        }
        (Some(p), None) => {
            if p.stability_order_for_empty_check != 0 {
                Ordering::Less
            } else {
                Ordering::Greater
            }
        }
        (Some(pa), Some(pb)) => compare_parts(pa, pb),
    }
}

/// `version.rs`'s `version_compare`, operating on two pre-classified slices.
fn compare_slice(a: &[Part], b: &[Part]) -> Ordering {
    for i in 0..a.len().max(b.len()) {
        let ordering = compare_part_opt(a.get(i), b.get(i));
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    Ordering::Equal
}

/// A pre-parsed [`NormalizedVersion`], reused across every
/// [`CompiledConstraint::matches`] call for that package instead of
/// re-parsing the string each time. Cheap to build (`parse_version_key`
/// once per pool package), not free to skip (parsing is exactly the work
/// `CompiledConstraint` exists to hoist out of the hot loop). Also `Ord`
/// (see the impl below), so a `DefaultPolicy` sort can compare two package
/// versions directly instead of `crate::semver::compare`'s own per-call
/// re-normalisation.
pub struct VersionKey(VersionKeyRepr);

enum VersionKeyRepr {
    Numeric(Vec<Part>),
    /// `dev-<name>`: not a point on the numeric line (`Bound::is_zero`'s own
    /// module doc explains why `Intervals` keeps branches in a separate
    /// field), so [`CompiledConstraint::matches`] can't answer for one.
    /// Still carries its parsed parts: `Ord` needs them for the *mixed*
    /// case (one branch, one numeric), which `crate::semver::compare` (via
    /// `match_specific`, `compare_branches: true`) resolves with a plain
    /// `version_compare` too — only *two branches* gets special handling
    /// below.
    Branch(Vec<Part>),
}

impl VersionKey {
    fn parts(&self) -> &[Part] {
        match &self.0 {
            VersionKeyRepr::Numeric(parts) | VersionKeyRepr::Branch(parts) => parts,
        }
    }

    fn is_branch(&self) -> bool {
        matches!(self.0, VersionKeyRepr::Branch(_))
    }
}

/// `crate::semver::compare`'s own `Ord`, direct on pre-parsed parts instead
/// of via `greater_than`/`less_than` (each of which re-normalises *and*
/// re-allocates a fresh `SingleConstraint` per call). Two branch versions
/// fold to `Equal` regardless of name (`match_specific`'s "both branches,
/// operators aren't both `Eq`" case always returns `false` for both
/// directions) — the same documented gap `crate::semver::compare`'s own doc
/// comment already carries; a branch compared against a non-branch (a
/// numeric alias target, `dev-master`-style) falls through to the same
/// part-by-part comparison as two numeric versions, matching
/// `match_specific`'s own "`compare_branches` is `true`, fall through"
/// branch.
impl Ord for VersionKey {
    fn cmp(&self, other: &Self) -> Ordering {
        if self.is_branch() && other.is_branch() {
            return Ordering::Equal;
        }
        compare_slice(self.parts(), other.parts())
    }
}

impl PartialOrd for VersionKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl PartialEq for VersionKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}

impl Eq for VersionKey {}

/// Composer's own `SingleConstraint::is_dev_branch`: a literal branch name,
/// as opposed to a numeric version that merely carries a `-dev` suffix
/// (`1.0.0.0-dev`, `9999999.9999999.9999999.9999999-dev`).
pub fn parse_version_key(version: &NormalizedVersion) -> VersionKey {
    let s = version.as_str();
    let parts = parse_parts(s);
    if s.starts_with("dev-") {
        VersionKey(VersionKeyRepr::Branch(parts))
    } else {
        VersionKey(VersionKeyRepr::Numeric(parts))
    }
}

/// One numeric sub-range of a constraint (`Interval`'s own `start`/`end`,
/// each always `Ge`/`Gt` and `Le`/`Lt` respectively — `Intervals`'
/// `generate_single_intervals`/`generate_multi_intervals` never produce any
/// other operator on either side), pre-parsed into [`Part`]s once.
struct CompiledInterval {
    start: Vec<Part>,
    start_inclusive: bool,
    end: Vec<Part>,
    end_inclusive: bool,
}

/// A [`Constraint`] compiled into numeric sub-ranges once, matched many
/// times with no allocation or string work — see the module doc above
/// `Part` for why this exists. Build one per distinct constraint *string*
/// (callers with a hot loop over many package versions against a handful of
/// distinct requiring constraints should cache by that string, not rebuild
/// per package).
pub struct CompiledConstraint {
    numeric: Vec<CompiledInterval>,
}

impl CompiledConstraint {
    /// `Intervals::get`, called once, is the disjunction-safe source of
    /// truth this hoists out of the hot path: it already resolves nested
    /// AND/OR structure (including `Intervals::compactConstraint`'s own
    /// canonicalisation) into a flat set of non-overlapping numeric
    /// intervals, so this doesn't need its own AND/OR tree walker.
    pub fn compile(constraint: &Constraint) -> CompiledConstraint {
        let result = semver_php::Intervals::new().get(constraint.0.as_ref());
        let numeric = result
            .numeric
            .into_iter()
            .map(|interval| CompiledInterval {
                start_inclusive: interval.start().operator() == Operator::Ge,
                start: parse_parts(interval.start().version()),
                end_inclusive: interval.end().operator() == Operator::Le,
                end: parse_parts(interval.end().version()),
            })
            .collect();
        CompiledConstraint { numeric }
    }

    /// `Some(matches)` for a numeric `key`; `None` when `key` is a dev
    /// branch (the caller's cue to fall back to `Constraint::matches`,
    /// which alone knows how to match a branch — see the module doc).
    pub fn matches(&self, key: &VersionKey) -> Option<bool> {
        let VersionKeyRepr::Numeric(parts) = &key.0 else {
            return None;
        };
        Some(self.numeric.iter().any(|interval| {
            let above_start = match compare_slice(parts, &interval.start) {
                Ordering::Less => false,
                Ordering::Equal => interval.start_inclusive,
                Ordering::Greater => true,
            };
            above_start
                && match compare_slice(parts, &interval.end) {
                    Ordering::Greater => false,
                    Ordering::Equal => interval.end_inclusive,
                    Ordering::Less => true,
                }
        }))
    }
}
