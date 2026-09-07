//! `composer/semver`'s own `PHPUnit` corpus, gating `semver-php` before any
//! solver code depends on it (`docs/resolver-design.md`'s "Constraints and
//! versions"). Each fixture under `tests/fixtures/composer/semver/` is a
//! JSON array of rows extracted straight from a real
//! `VersionParserTest`/`ComparatorTest`/`SemverTest`/`SubsetsTest` data
//! provider (a devbox PHP script loaded the actual `Composer\Semver\*`
//! classes and `(string)`-cast every `Constraint` object, exactly like the
//! `PHPUnit` test methods do before asserting).

use std::cmp::Ordering;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use vivace::semver::{
    CompiledConstraint, compare, have_intersections, is_subset_of, normalize, normalize_branch,
    parse_constraint, parse_numeric_alias_prefix, parse_version_key, stability,
};

fn fixture(name: &str) -> Vec<Value> {
    let path: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/composer/semver")
        .join(name);
    let content = fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&content).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn s(v: &Value) -> &str {
    v.as_str()
        .unwrap_or_else(|| panic!("expected a string, got {v}"))
}

// VersionParserTest::numericAliasVersions -> VersionParser::parseNumericAliasPrefix
#[test]
fn numeric_alias_prefix() {
    for row in fixture("numeric_alias_prefix.json") {
        let (input, expected) = (s(&row[0]), row[1].as_str());
        assert_eq!(
            parse_numeric_alias_prefix(input),
            expected.map(str::to_string),
            "{input}"
        );
    }
}

// VersionParserTest::successfulNormalizedBranches -> VersionParser::normalizeBranch
#[test]
fn normalize_branches() {
    for row in fixture("normalize_branches.json") {
        let (input, expected) = (s(&row[0]), s(&row[1]));
        assert_eq!(normalize_branch(input), expected, "{input}");
    }
}

// VersionParserTest::stabilityProvider -> VersionParser::parseStability
#[test]
fn stabilities() {
    for row in fixture("stability.json") {
        let (expected, version) = (s(&row[0]), s(&row[1]));
        assert_eq!(stability(version), expected, "{version}");
    }
}

// VersionParserTest::{simpleConstraints,constraintProvider}: input constraint
// string -> the exact `(string) $constraint` Composer produces.
fn assert_constraint_rows(file: &str) {
    for row in fixture(file) {
        let (input, expected) = (s(&row[0]), s(&row[1]));
        let got = parse_constraint(input)
            .unwrap_or_else(|e| panic!("{input}: {e}"))
            .to_string();
        assert_eq!(got, expected, "{input}");
    }
}

#[test]
fn simple_constraints() {
    assert_constraint_rows("simple_constraints.json");
}

#[test]
fn constraint_provider() {
    assert_constraint_rows("constraint_provider.json");
}

// VersionParserTest::{wildcardConstraints,tildeConstraints,caretConstraints,
// hyphenConstraints}: input, min (nullable), max -> a conjunctive
// `[min max]` (or bare `max` when there's no min).
fn assert_min_max_rows(file: &str) {
    for row in fixture(file) {
        let input = s(&row[0]);
        let max = s(&row[2]);
        let expected = match row[1].as_str() {
            Some(min) => format!("[{min} {max}]"),
            None => max.to_string(),
        };
        let got = parse_constraint(input)
            .unwrap_or_else(|e| panic!("{input}: {e}"))
            .to_string();
        assert_eq!(got, expected, "{input}");
    }
}

#[test]
fn wildcard_constraints() {
    assert_min_max_rows("wildcard_constraints.json");
}

#[test]
fn tilde_constraints() {
    assert_min_max_rows("tilde_constraints.json");
}

#[test]
fn caret_constraints() {
    assert_min_max_rows("caret_constraints.json");
}

#[test]
fn hyphen_constraints() {
    assert_min_max_rows("hyphen_constraints.json");
}

// VersionParserTest::multiConstraintProvider: every input is an equivalent
// spelling of `> 2.0.0.0` conjoined with `<= 3.0.0.0`.
#[test]
fn multi_constraints() {
    let expected = parse_constraint(">2.0,<=3.0").unwrap().to_string();
    for row in fixture("multi_constraints.json") {
        let input = s(&row[0]);
        let got = parse_constraint(input)
            .unwrap_or_else(|e| panic!("{input}: {e}"))
            .to_string();
        assert_eq!(got, expected, "{input}");
    }
}

// VersionParserTest::multiConstraintProvider2: disjunction has priority over
// a comma-separated conjunction on the same level.
#[test]
fn multi_constraints_disjunctive_priority() {
    let expected = "[[> 2.0.0.0 < 2.0.5.0-dev] || > 2.0.6.0]";
    for row in fixture("multi_constraints_disjunctive_prio.json") {
        let input = s(&row[0]);
        let got = parse_constraint(input)
            .unwrap_or_else(|e| panic!("{input}: {e}"))
            .to_string();
        assert_eq!(got, expected, "{input}");
    }
}

// VersionParserTest::failingConstraints: every input must fail to parse.
#[test]
fn failing_constraints() {
    for row in fixture("failing_constraints.json") {
        let input = s(&row[0]);
        assert!(
            parse_constraint(input).is_err(),
            "{input:?} should have failed to parse"
        );
    }
}

// ComparatorTest: version1, version2, expected -> compare(v1, v2) direction.
fn assert_comparator_rows(file: &str, holds: impl Fn(Ordering) -> bool) {
    for row in fixture(file) {
        let (v1, v2, expected) = (s(&row[0]), s(&row[1]), row[2].as_bool().unwrap());
        let a = normalize(v1).unwrap_or_else(|e| panic!("{v1}: {e}"));
        let b = normalize(v2).unwrap_or_else(|e| panic!("{v2}: {e}"));
        assert_eq!(holds(compare(&a, &b)), expected, "{v1} vs {v2}");
    }
}

#[test]
fn comparator_greater_than() {
    assert_comparator_rows("comparator_greater_than.json", |o| o == Ordering::Greater);
}

#[test]
fn comparator_greater_than_or_equal_to() {
    assert_comparator_rows("comparator_greater_than_or_equal_to.json", |o| {
        o != Ordering::Less
    });
}

#[test]
fn comparator_less_than() {
    assert_comparator_rows("comparator_less_than.json", |o| o == Ordering::Less);
}

#[test]
fn comparator_less_than_or_equal_to() {
    assert_comparator_rows("comparator_less_than_or_equal_to.json", |o| {
        o != Ordering::Greater
    });
}

#[test]
fn comparator_equal_to() {
    // Two rows compare unrelated arbitrary dev branches (`dev-foo` vs
    // `dev-master`/`dev-bar`), which Composer treats as neither equal, nor
    // greater, nor less than one another - a real "incomparable" outcome
    // `compare`'s `Ordering` return can't represent (see the ponytail note
    // on `vivace::semver::compare`). Skip just those two known rows rather
    // than claim a pass `compare` can't back up.
    for row in fixture("comparator_equal_to.json") {
        let (v1, v2, expected) = (s(&row[0]), s(&row[1]), row[2].as_bool().unwrap());
        if v1.starts_with("dev-") && v2.starts_with("dev-") && v1 != v2 {
            continue;
        }
        let a = normalize(v1).unwrap_or_else(|e| panic!("{v1}: {e}"));
        let b = normalize(v2).unwrap_or_else(|e| panic!("{v2}: {e}"));
        assert_eq!(compare(&a, &b) == Ordering::Equal, expected, "{v1} vs {v2}");
    }
}

#[test]
fn comparator_not_equal_to() {
    assert_comparator_rows("comparator_not_equal_to.json", |o| o != Ordering::Equal);
}

// ComparatorTest::compareProvider: version1, operator, version2, expected.
#[test]
fn comparator_compare() {
    for row in fixture("comparator_compare.json") {
        let (v1, op, v2, expected) = (
            s(&row[0]),
            s(&row[1]),
            s(&row[2]),
            row[3].as_bool().unwrap(),
        );
        let a = normalize(v1).unwrap_or_else(|e| panic!("{v1}: {e}"));
        let b = normalize(v2).unwrap_or_else(|e| panic!("{v2}: {e}"));
        let ord = compare(&a, &b);
        let holds = match op {
            ">" => ord == Ordering::Greater,
            ">=" => ord != Ordering::Less,
            "<" => ord == Ordering::Less,
            "<=" => ord != Ordering::Greater,
            "==" | "=" => ord == Ordering::Equal,
            "!=" | "<>" => ord != Ordering::Equal,
            other => panic!("unknown operator {other}"),
        };
        assert_eq!(holds, expected, "{v1} {op} {v2}");
    }
}

// SemverTest::satisfiesProvider: expected, version, constraint.
#[test]
fn satisfies() {
    for row in fixture("satisfies.json") {
        let (expected, version, constraint) = (row[0].as_bool().unwrap(), s(&row[1]), s(&row[2]));
        let normalized = normalize(version).unwrap_or_else(|e| panic!("{version}: {e}"));
        let matches = parse_constraint(constraint)
            .unwrap_or_else(|e| panic!("{constraint}: {e}"))
            .matches(&normalized);
        assert_eq!(matches, expected, "{version} satisfies {constraint}");
    }
}

// SemverTest::satisfiedByProvider: constraint, versions, satisfied subset.
#[test]
fn satisfied_by() {
    for row in fixture("satisfied_by.json") {
        let constraint_str = s(&row[0]);
        let versions: Vec<&str> = row[1].as_array().unwrap().iter().map(s).collect();
        let expected: Vec<&str> = row[2].as_array().unwrap().iter().map(s).collect();

        let constraint = parse_constraint(constraint_str).unwrap();
        let got: Vec<&str> = versions
            .into_iter()
            .filter(|v| {
                let normalized = normalize(v).unwrap_or_else(|e| panic!("{v}: {e}"));
                constraint.matches(&normalized)
            })
            .collect();
        assert_eq!(got, expected, "{constraint_str}");
    }
}

// SemverTest::sortProvider: `Semver::sort`/`rsort` (not part of the facade;
// exercised directly to gate the crate's own default-branch-alias handling,
// which plain `compare` does not attempt).
#[test]
fn sort() {
    for row in fixture("sort.json") {
        let versions: Vec<&str> = row[0].as_array().unwrap().iter().map(s).collect();
        let ascending: Vec<&str> = row[1].as_array().unwrap().iter().map(s).collect();
        let descending: Vec<&str> = row[2].as_array().unwrap().iter().map(s).collect();

        assert_eq!(semver_php::Semver::sort(&versions).unwrap(), ascending);
        assert_eq!(semver_php::Semver::rsort(&versions).unwrap(), descending);
    }
}

// SubsetsTest::{subsets,notSubsets} -> Intervals::isSubsetOf.
fn assert_subset_rows(file: &str, expected: bool) {
    for row in fixture(file) {
        let (a_str, b_str) = (s(&row[0]), s(&row[1]));
        let a = parse_constraint(a_str).unwrap_or_else(|e| panic!("{a_str}: {e}"));
        let b = parse_constraint(b_str).unwrap_or_else(|e| panic!("{b_str}: {e}"));
        assert_eq!(is_subset_of(&a, &b), expected, "{a_str} subset of {b_str}");
    }
}

#[test]
fn is_subset_of_corpus() {
    assert_subset_rows("is_subset_of.json", true);
}

#[test]
fn is_not_subset_of_corpus() {
    assert_subset_rows("is_not_subset_of.json", false);
}

// `have_intersections` has no dedicated upstream data provider; sanity-check
// it against a couple of rows from the subset corpus (every subset pair
// intersects; disjoint ranges from the "not subset" corpus may or may not,
// so this only exercises the happy path).
#[test]
fn have_intersections_smoke() {
    let a = parse_constraint("^1.0").unwrap();
    let b = parse_constraint("^1.5").unwrap();
    assert!(have_intersections(&a, &b));

    let c = parse_constraint("^2.0").unwrap();
    let d = parse_constraint("^3.0").unwrap();
    assert!(!have_intersections(&c, &d));
}

// https://github.com/svandragt/vivace/issues/73: `semver-php` 0.1.0 panics
// on a non-ASCII byte instead of erroring (it indexes by byte offset
// without checking char boundaries). The facade must reject it first.
#[test]
fn non_ascii_constraint_errors_instead_of_panicking() {
    let err = parse_constraint("v-Լ,~").err().unwrap();
    assert_eq!(
        err.to_string(),
        "Could not parse version constraint v-Լ,~: Invalid version string \"v-Լ,~\""
    );

    let err = parse_constraint("Լ").err().unwrap();
    assert_eq!(
        err.to_string(),
        "Could not parse version constraint Լ: Invalid version string \"Լ\""
    );
}

#[test]
fn non_ascii_version_errors_instead_of_panicking() {
    assert!(normalize("v-Լ,~").is_err());
    assert!(normalize("Լ").is_err());
}

// #76 follow-up: `CompiledConstraint` (`src/semver.rs`) must agree with
// `Constraint::matches` on every (constraint, version) pair either corpus
// fixture exercises, since it exists purely to answer the same question
// faster — any divergence here is a wrong solve waiting to happen, not a
// style nit. Skips a version that is a dev branch (`CompiledConstraint`
// only compiles the numeric side; see `src/semver.rs`'s module doc), since
// callers already fall back to `Constraint::matches` for those.
fn assert_compiled_matches_agree(constraint_str: &str, version: &str) {
    let normalized = normalize(version).unwrap_or_else(|e| panic!("{version}: {e}"));
    let key = parse_version_key(&normalized);
    let constraint =
        parse_constraint(constraint_str).unwrap_or_else(|e| panic!("{constraint_str}: {e}"));
    let compiled = CompiledConstraint::compile(&constraint);
    let Some(compiled_result) = compiled.matches(&key) else {
        return;
    };
    let direct_result = constraint.matches(&normalized);
    assert_eq!(
        compiled_result, direct_result,
        "{version} against {constraint_str}: compiled={compiled_result} direct={direct_result}"
    );
}

#[test]
fn compiled_constraint_agrees_with_matches_on_the_satisfies_corpus() {
    for row in fixture("satisfies.json") {
        let (version, constraint_str) = (s(&row[1]), s(&row[2]));
        assert_compiled_matches_agree(constraint_str, version);
    }
}

#[test]
fn compiled_constraint_agrees_with_matches_on_the_satisfied_by_corpus() {
    for row in fixture("satisfied_by.json") {
        let constraint_str = s(&row[0]);
        for version in row[1].as_array().unwrap().iter().map(s) {
            assert_compiled_matches_agree(constraint_str, version);
        }
    }
}

// #76 second follow-up: `VersionKey`'s `Ord` (`src/semver.rs`) must agree
// with `crate::semver::compare` on every pair it could ever be asked about,
// since `DefaultPolicy` now sorts on it directly instead of calling
// `compare` — a wrong tie-break here reorders which version wins, not just
// which one runs slower.
fn assert_version_key_ord_agrees(a: &str, b: &str) {
    let na = normalize(a).unwrap_or_else(|e| panic!("{a}: {e}"));
    let nb = normalize(b).unwrap_or_else(|e| panic!("{b}: {e}"));
    let (ka, kb) = (parse_version_key(&na), parse_version_key(&nb));
    assert_eq!(
        ka.cmp(&kb),
        compare(&na, &nb),
        "VersionKey ordering of {a} vs {b} disagrees with compare"
    );
}

// SemverTest::sortProvider: every pair within a row's own version list,
// not just the adjacent ones the ascending/descending columns already
// imply — a stronger check than re-deriving `sort()`'s own assertion.
#[test]
fn version_key_ord_agrees_with_compare_on_the_sort_corpus() {
    for row in fixture("sort.json") {
        let versions: Vec<&str> = row[0].as_array().unwrap().iter().map(s).collect();
        for (i, &a) in versions.iter().enumerate() {
            for &b in &versions[i + 1..] {
                assert_version_key_ord_agrees(a, b);
            }
        }
    }
}

// Every pair of versions *within each package's own recorded history*
// (`tests/fixtures/packagist/repo.packagist.org/p2/**`): a much larger,
// real-world corpus than the handful of `composer/semver`'s own unit-test
// rows, exercising version shapes actual packages ship (patch releases,
// `-dev` suffixes, branch aliases) rather than synthetic ones. Scoped to
// one package's own versions at a time (not a global all-pairs across every
// package in the fixture set): that's the only comparison `DefaultPolicy`
// ever actually makes (candidates for the same require), so it is both the
// faithful equivalence check and, at ~2,000 versions across 41 files, the
// difference between a sub-second test and a multi-minute one.
#[test]
fn version_key_ord_agrees_with_compare_on_recorded_packagist_versions() {
    // A handful of packages (`phpunit/phpunit`, `symfony/yaml`) have
    // hundreds of releases; an all-pairs check over every one of them would
    // dominate the whole test suite's runtime for no extra confidence over
    // a stride-sampled subset spanning the same range.
    const MAX_VERSIONS_PER_PACKAGE: usize = 80;

    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/packagist/repo.packagist.org/p2");
    let mut files: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                files.push(path);
            }
        }
    }
    assert!(
        files.len() > 10,
        "fixture corpus looks too small: {}",
        files.len()
    );

    let mut pairs_checked = 0usize;
    for path in files {
        let content =
            fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let doc: Value =
            serde_json::from_str(&content).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let Some(packages) = doc.get("packages").and_then(Value::as_object) else {
            continue;
        };
        for entries in packages.values() {
            let mut versions: Vec<&str> = entries
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry.get("version").and_then(Value::as_str))
                .collect();
            versions.sort_unstable();
            versions.dedup();
            if versions.len() > MAX_VERSIONS_PER_PACKAGE {
                let stride = versions.len().div_ceil(MAX_VERSIONS_PER_PACKAGE);
                versions = versions.iter().copied().step_by(stride).collect();
            }
            for (i, &a) in versions.iter().enumerate() {
                for &b in &versions[i + 1..] {
                    assert_version_key_ord_agrees(a, b);
                    pairs_checked += 1;
                }
            }
        }
    }
    assert!(
        pairs_checked > 1000,
        "too few pairs checked: {pairs_checked}"
    );
}
