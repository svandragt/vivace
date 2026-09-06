//! `vendor/composer/platform_check.php`, a port of
//! `AutoloadGenerator::getPlatformCheck` with the slice of `composer/semver`
//! needed to find a constraint's lower bound and test provider overlap.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use regex::Regex;
use serde_json::{Map, Value};

use crate::lock::PlatformCheck;
use crate::version;

/// One package's platform-relevant links. Dev packages still count as
/// extension providers but their own requirements are not checked.
pub struct PlatformInput<'a> {
    pub name: &'a str,
    pub dev: bool,
    pub require: &'a Map<String, Value>,
    pub provide: &'a Map<String, Value>,
    pub replace: &'a Map<String, Value>,
}

/// `--ignore-platform-reqs` / `--ignore-platform-req=<name>` (with `*`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum IgnorePlatform {
    #[default]
    None,
    All,
    List(Vec<String>),
}

/// Returns the file body, or `None` when there is nothing to check.
pub fn platform_check(
    packages: &[PlatformInput],
    mode: PlatformCheck,
    ignore: &IgnorePlatform,
) -> Result<Option<String>> {
    if mode == PlatformCheck::Off || *ignore == IgnorePlatform::All {
        return Ok(None);
    }
    let ignored: Vec<Regex> = match ignore {
        IgnorePlatform::List(patterns) => patterns
            .iter()
            .map(|p| Regex::new(&format!("(?i)^{}$", regex::escape(p).replace(r"\*", ".*"))))
            .collect::<std::result::Result<_, _>>()?,
        _ => Vec::new(),
    };

    // Extension providers: any package's `replace`/`provide` of `ext-*`.
    let mut providers: HashMap<String, Vec<Vec<Interval>>> = HashMap::new();
    for package in packages {
        for (target, constraint) in package.replace.iter().chain(package.provide) {
            if let Some(ext) = extension_name(target) {
                providers
                    .entry(ext)
                    .or_default()
                    .push(parse_constraints(constraint_str(constraint), package.name)?);
            }
        }
    }

    let mut lowest_php = Bound::zero();
    let mut php_64bit = false;
    let mut extensions: BTreeMap<String, String> = BTreeMap::new();
    for package in packages {
        // Dev requirements are skipped: the check is a production safeguard.
        if package.dev {
            continue;
        }
        for (target, constraint) in package.require {
            let target = target.to_ascii_lowercase();
            if ignored.iter().any(|re| re.is_match(&target)) {
                continue;
            }
            if target == "php" || target == "php-64bit" {
                let bound = lower_bound(&parse_constraints(
                    constraint_str(constraint),
                    package.name,
                )?);
                if bound.compare_to(&lowest_php, Ordering::Greater) {
                    lowest_php = bound;
                }
            }
            if target == "php-64bit" {
                php_64bit = true;
            }
            if mode != PlatformCheck::All {
                continue;
            }
            let Some(mut ext) = extension_name(&target) else {
                continue;
            };
            let required = parse_constraints(constraint_str(constraint), package.name)?;
            if providers
                .get(&ext)
                .is_some_and(|list| list.iter().any(|provided| intersects(provided, &required)))
            {
                continue;
            }
            if ext == "zend-opcache" {
                ext = "zend opcache".into();
            }
            let quoted = format!("'{ext}'");
            let line = if ext == "pcntl" || ext == "readline" {
                format!(
                    "PHP_SAPI !== 'cli' || extension_loaded({quoted}) || $missingExtensions[] = {quoted};\n"
                )
            } else {
                format!("extension_loaded({quoted}) || $missingExtensions[] = {quoted};\n")
            };
            extensions.insert(quoted, line);
        }
    }

    let mut required_php = String::new();
    if !lowest_php.is_zero() {
        let operator = if lowest_php.inclusive { ">=" } else { ">" };
        let dotted = lowest_php.version.replace('-', ".");
        let chunks: Vec<&str> = dotted.split('.').collect();
        let num = |i: usize| {
            chunks
                .get(i)
                .and_then(|c| c.parse::<u64>().ok())
                .unwrap_or(0)
        };
        let version_id = num(0) * 10000 + num(1) * 100 + num(2);
        let human = chunks[..chunks.len().min(3)].join(".");
        required_php = format!(
            "\nif (!(PHP_VERSION_ID {operator} {version_id})) {{\n    $issues[] = 'Your Composer dependencies require a PHP version \"{operator} {human}\". You are running ' . PHP_VERSION . '.';\n}}\n"
        );
    }
    if php_64bit {
        required_php.push_str(
            "\nif (PHP_INT_SIZE !== 8) {\n    $issues[] = 'Your Composer dependencies require a 64-bit build of PHP.';\n}\n",
        );
    }
    let mut required_extensions = String::new();
    if !extensions.is_empty() {
        let lines: String = extensions.into_values().collect();
        required_extensions = format!(
            "\n$missingExtensions = array();\n{lines}\nif ($missingExtensions) {{\n    $issues[] = 'Your Composer dependencies require the following PHP extensions to be installed: ' . implode(', ', $missingExtensions) . '.';\n}}\n"
        );
    }
    if required_php.is_empty() && required_extensions.is_empty() {
        return Ok(None);
    }
    Ok(Some(format!(
        "<?php\n\n// platform_check.php @generated by Composer\n\n$issues = array();\n{required_php}{required_extensions}\nif ($issues) {{\n    if (!headers_sent()) {{\n        header('HTTP/1.1 500 Internal Server Error');\n    }}\n    if (!ini_get('display_errors')) {{\n        if (PHP_SAPI === 'cli' || PHP_SAPI === 'phpdbg') {{\n            fwrite(STDERR, 'Composer detected issues in your platform:' . PHP_EOL.PHP_EOL . implode(PHP_EOL, $issues) . PHP_EOL.PHP_EOL);\n        }} elseif (!headers_sent()) {{\n            echo 'Composer detected issues in your platform:' . PHP_EOL.PHP_EOL . str_replace('You are running '.PHP_VERSION.'.', '', implode(PHP_EOL, $issues)) . PHP_EOL.PHP_EOL;\n        }}\n    }}\n    throw new \\RuntimeException(\n        'Composer detected issues in your platform: ' . implode(' ', $issues)\n    );\n}}\n"
    )))
}

fn constraint_str(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// `ext-foo` gives `foo`; Composer lowercases link targets.
fn extension_name(target: &str) -> Option<String> {
    let rest = target
        .get(4..)
        .filter(|_| target[..4].eq_ignore_ascii_case("ext-"))?;
    (!rest.is_empty()).then(|| rest.to_ascii_lowercase())
}

/// `composer/semver`'s `Bound`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Bound {
    version: String,
    inclusive: bool,
}

impl Bound {
    fn zero() -> Self {
        Bound {
            version: "0.0.0.0-dev".into(),
            inclusive: true,
        }
    }

    fn infinity() -> Self {
        Bound {
            version: format!("{}.0.0.0", i64::MAX),
            inclusive: false,
        }
    }

    fn is_zero(&self) -> bool {
        *self == Bound::zero()
    }

    /// `Bound::compareTo`: is `self` strictly `direction` of `other`?
    fn compare_to(&self, other: &Bound, direction: Ordering) -> bool {
        if self == other {
            return false;
        }
        match version_compare(&self.version, &other.version) {
            Ordering::Equal => (direction == Ordering::Greater) == other.inclusive,
            ord => ord == direction,
        }
    }
}

/// One conjunctive group of a constraint: `[lower, upper]` with each end
/// inclusive or not. A `||` constraint is a list of these.
#[derive(Debug, Clone)]
struct Interval {
    lower: Bound,
    upper: Bound,
}

impl Interval {
    fn all() -> Self {
        Interval {
            lower: Bound::zero(),
            upper: Bound::infinity(),
        }
    }
}

/// `MultiConstraint::getLowerBound` for a disjunction: the lowest lower bound.
fn lower_bound(groups: &[Interval]) -> Bound {
    let mut lowest = groups[0].lower.clone();
    for group in &groups[1..] {
        if group.lower.compare_to(&lowest, Ordering::Less) {
            lowest = group.lower.clone();
        }
    }
    lowest
}

/// Do the two constraints share any version? (`ConstraintInterface::matches`
/// for the numeric constraints platform packages use.)
fn intersects(a: &[Interval], b: &[Interval]) -> bool {
    let below = |lower: &Bound, upper: &Bound| match version_compare(&lower.version, &upper.version)
    {
        Ordering::Less => true,
        Ordering::Equal => lower.inclusive && upper.inclusive,
        Ordering::Greater => false,
    };
    a.iter().any(|x| {
        b.iter()
            .any(|y| below(&x.lower, &y.upper) && below(&y.lower, &x.upper))
    })
}

static OPERATOR_SPACE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(<>|!=|>=?|<=?|==?|~|\^)\s+").unwrap());
static OR_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\|\|?\s*").unwrap());
static AND_SPLIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*,\s*|\s+").unwrap());

/// `VersionParser::parseConstraints`, reduced to bounds.
fn parse_constraints(constraints: &str, package: &str) -> Result<Vec<Interval>> {
    let text = constraints.trim();
    if text.is_empty() {
        bail!("{package}: empty version constraint");
    }
    // Composer's and-split regex uses lookbehind to keep `>= 7.2` and
    // `7.2 - 8.0` together; normalise those shapes first instead.
    let text = OPERATOR_SPACE
        .replace_all(text, "$1")
        .replace(" - ", "\u{1}");
    let mut groups = Vec::new();
    for or_part in OR_SPLIT.split(&text) {
        let mut interval = Interval::all();
        for and_part in AND_SPLIT.split(or_part) {
            let part = and_part.replace('\u{1}', " - ");
            let parsed = parse_constraint(&part).with_context(|| {
                format!("{package}: unsupported version constraint \"{constraints}\"")
            })?;
            if parsed.lower.compare_to(&interval.lower, Ordering::Greater) {
                interval.lower = parsed.lower;
            }
            if parsed.upper.compare_to(&interval.upper, Ordering::Less) {
                interval.upper = parsed.upper;
            }
        }
        groups.push(interval);
    }
    Ok(groups)
}

const MODIFIER: &str =
    r"[._-]?(?:(stable|beta|b|RC|alpha|a|patch|pl|p)((?:[.-]?\d+)*)?)?([.-]?dev)?";

static WILDCARD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^v?[x*](\.[x*])*$").unwrap());
static STABILITY_FLAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^([^,\s]*?)@(stable|RC|beta|alpha|dev)$").unwrap());
static TILDE_CARET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^([~^]>?)v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:\.(\d+))?(?:{MODIFIER}|\.([xX*][.-]?dev))(?:\+[^\s]+)?$"
    ))
    .unwrap()
});
static X_RANGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:\.[xX*])+$").unwrap());
static HYPHEN: LazyLock<Regex> = LazyLock::new(|| {
    let version = format!(
        r"v?(\d+)(?:\.(\d+))?(?:\.(\d+))?(?:\.(\d+))?(?:{MODIFIER}|\.([xX*][.-]?dev))(?:\+[^\s]+)?"
    );
    Regex::new(&format!("(?i)^({version}) +- +({version})$")).unwrap()
});
static COMPARATOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(<>|!=|>=?|<=?|==?)?\s*(.*)$").unwrap());
static MODIFIER_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i)-{MODIFIER}$")).unwrap());

fn group<'a>(m: &'a regex::Captures<'a>, i: usize) -> &'a str {
    m.get(i).map_or("", |g| g.as_str())
}

/// `VersionParser::parseConstraint` for one operator-and-version.
fn parse_constraint(constraint: &str) -> Result<Interval> {
    let mut constraint = constraint;
    if let Some(m) = STABILITY_FLAG.captures(constraint) {
        // The stability flag only shifts a bound's pre-release suffix, which
        // never changes the PHP version id or human-readable form.
        constraint = if group(&m, 1).is_empty() {
            "*"
        } else {
            m.get(1).unwrap().as_str()
        };
    }
    if WILDCARD.is_match(constraint) {
        return Ok(Interval::all());
    }

    if let Some(m) = TILDE_CARET.captures(constraint) {
        if group(&m, 1) == "~>" {
            bail!("invalid operator \"~>\", you probably meant \"~\"");
        }
        let nums = [group(&m, 2), group(&m, 3), group(&m, 4), group(&m, 5)];
        let no_stability =
            group(&m, 6).is_empty() && group(&m, 8).is_empty() && group(&m, 9).is_empty();
        let mut position = nums.iter().rposition(|n| !n.is_empty()).unwrap_or(0) + 1;
        if group(&m, 1) == "^" {
            position = if nums[0] != "0" || nums[1].is_empty() {
                1
            } else if nums[1] != "0" || nums[2].is_empty() {
                2
            } else {
                3
            };
        } else if !group(&m, 9).is_empty() {
            position += 1;
        }
        let low = format!(
            "{}{}",
            &constraint[1..],
            if no_stability { "-dev" } else { "" }
        );
        let high_position = if group(&m, 1) == "^" {
            position
        } else {
            position.saturating_sub(1).max(1)
        };
        return Ok(Interval {
            lower: Bound {
                version: version::normalize(&low)?,
                inclusive: true,
            },
            upper: Bound {
                version: format!("{}-dev", manipulate(&nums, high_position, true)),
                inclusive: false,
            },
        });
    }

    if let Some(m) = X_RANGE.captures(constraint) {
        let nums = [group(&m, 1), group(&m, 2), group(&m, 3), ""];
        let position = nums.iter().rposition(|n| !n.is_empty()).unwrap_or(0) + 1;
        let low = format!("{}-dev", manipulate(&nums, position, false));
        let high = Bound {
            version: format!("{}-dev", manipulate(&nums, position, true)),
            inclusive: false,
        };
        let lower = if low == "0.0.0.0-dev" {
            Bound::zero()
        } else {
            Bound {
                version: low,
                inclusive: true,
            }
        };
        return Ok(Interval { lower, upper: high });
    }

    if let Some(m) = HYPHEN.captures(constraint) {
        let (from, to) = (group(&m, 1), group(&m, 10));
        let low_stability =
            group(&m, 6).is_empty() && group(&m, 8).is_empty() && group(&m, 9).is_empty();
        let lower = Bound {
            version: format!(
                "{}{}",
                version::normalize(from)?,
                if low_stability { "-dev" } else { "" }
            ),
            inclusive: true,
        };
        let to_nums = [group(&m, 11), group(&m, 12), group(&m, 13), group(&m, 14)];
        let full = (!to_nums[1].is_empty() && !to_nums[2].is_empty())
            || !group(&m, 15).is_empty()
            || !group(&m, 17).is_empty()
            || !group(&m, 18).is_empty();
        let upper = if full {
            Bound {
                version: version::normalize(to)?,
                inclusive: true,
            }
        } else {
            version::normalize(to)?;
            let position = if to_nums[1].is_empty() { 1 } else { 2 };
            Bound {
                version: format!("{}-dev", manipulate(&to_nums, position, true)),
                inclusive: false,
            }
        };
        return Ok(Interval { lower, upper });
    }

    let m = COMPARATOR.captures(constraint).expect("matches any string");
    let op = group(&m, 1);
    let raw = group(&m, 2);
    let mut version = version::normalize(raw)?;
    if (op == "<" || op == ">=")
        && !MODIFIER_SUFFIX.is_match(&raw.to_lowercase())
        && !raw.starts_with("dev-")
    {
        version.push_str("-dev");
    }
    let at = |inclusive: bool| Bound {
        version: version.clone(),
        inclusive,
    };
    Ok(match op {
        "" | "=" | "==" => Interval {
            lower: at(true),
            upper: at(true),
        },
        "<" => Interval {
            lower: Bound::zero(),
            upper: at(false),
        },
        "<=" => Interval {
            lower: Bound::zero(),
            upper: at(true),
        },
        ">" => Interval {
            lower: at(false),
            upper: Bound::infinity(),
        },
        ">=" => Interval {
            lower: at(true),
            upper: Bound::infinity(),
        },
        _ => Interval::all(),
    })
}

/// `VersionParser::manipulateVersionString`: zero everything after
/// `position` (1-based) and optionally bump the component at it.
fn manipulate(nums: &[&str; 4], position: usize, increment: bool) -> String {
    let parts: Vec<String> = (1..=4)
        .map(|i| {
            if i > position {
                "0".to_owned()
            } else if i == position && increment {
                (nums[i - 1].parse::<u64>().unwrap_or(0) + 1).to_string()
            } else {
                nums[i - 1].to_owned()
            }
        })
        .collect();
    parts.join(".")
}

/// PHP `version_compare` on normalised versions.
fn version_compare(a: &str, b: &str) -> Ordering {
    fn parts(s: &str) -> Vec<&str> {
        let mut out = Vec::new();
        let mut start = 0;
        let bytes = s.as_bytes();
        for i in 0..=bytes.len() {
            let boundary = i == bytes.len()
                || matches!(bytes[i], b'.' | b'-' | b'_' | b'+')
                || (i > start && bytes[i].is_ascii_digit() != bytes[i - 1].is_ascii_digit());
            if boundary {
                if i > start {
                    out.push(&s[start..i]);
                }
                start = if i < bytes.len() && matches!(bytes[i], b'.' | b'-' | b'_' | b'+') {
                    i + 1
                } else {
                    i
                };
            }
        }
        out
    }
    fn weight(part: &str) -> i8 {
        match part {
            "dev" => 0,
            "alpha" | "a" => 1,
            "beta" | "b" => 2,
            "RC" | "rc" => 3,
            "#" => 4,
            "pl" | "p" => 5,
            _ if part.starts_with(|c: char| c.is_ascii_digit()) => 4,
            _ => -6,
        }
    }
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let ord = match (a.get(i), b.get(i)) {
            (Some(x), Some(y)) => match (x.parse::<u64>(), y.parse::<u64>()) {
                (Ok(x), Ok(y)) => x.cmp(&y),
                _ => weight(x).cmp(&weight(y)),
            },
            // A missing part loses to a number and ranks as `#` against a suffix.
            (None, Some(y)) => {
                if y.parse::<u64>().is_ok() {
                    Ordering::Less
                } else {
                    weight("#").cmp(&weight(y))
                }
            }
            (Some(x), None) => {
                if x.parse::<u64>().is_ok() {
                    Ordering::Greater
                } else {
                    weight(x).cmp(&weight("#"))
                }
            }
            (None, None) => Ordering::Equal,
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    Ordering::Equal
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::{Map, Value, json};

    use super::{IgnorePlatform, PlatformInput, platform_check};
    use crate::lock::{PlatformCheck, read_lock, read_root};

    fn links(pairs: &[(&str, &str)]) -> Map<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), json!(v)))
            .collect()
    }

    fn golden(name: &str) -> String {
        fs_err::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/composer/autoload/platform")
                .join(format!("{name}.php")),
        )
        .unwrap()
    }

    type Links<'a> = Vec<(&'a str, &'a str)>;
    /// (label, require, provide, replace, ignore, expected fixture)
    type Case<'a> = (
        &'a str,
        Links<'a>,
        Links<'a>,
        Links<'a>,
        IgnorePlatform,
        Option<&'a str>,
    );

    /// `AutoloadGeneratorTest::platformCheckProvider`, all 12 rows.
    #[test]
    fn platform_check_matches_composer_fixtures() {
        let all = ["ext-xml", "ext-json"].map(|e| (e, "*"));
        let cases: Vec<Case> = vec![
            (
                "typical",
                vec![("php", "^7.2"), all[0], all[1]],
                vec![],
                vec![],
                IgnorePlatform::None,
                Some("typical"),
            ),
            (
                "no lower bound",
                vec![("php", "< 8")],
                vec![],
                vec![],
                IgnorePlatform::None,
                None,
            ),
            (
                "no upper bound",
                vec![("php", ">= 7.2")],
                vec![],
                vec![],
                IgnorePlatform::None,
                Some("no_php_upper_bound"),
            ),
            (
                "specific release",
                vec![("php", "^7.2.8")],
                vec![],
                vec![],
                IgnorePlatform::None,
                Some("specific_php_release"),
            ),
            (
                "specific 64bit",
                vec![("php-64bit", "^7.2.8")],
                vec![],
                vec![],
                IgnorePlatform::None,
                Some("specific_php_64bit_required"),
            ),
            (
                "64bit",
                vec![("php-64bit", "*")],
                vec![],
                vec![],
                IgnorePlatform::None,
                Some("php_64bit_required"),
            ),
            (
                "no php",
                vec![all[0], all[1]],
                vec![],
                vec![],
                IgnorePlatform::None,
                Some("no_php_required"),
            ),
            (
                "ignore all",
                vec![("php", "^7.2"), all[0], all[1]],
                vec![],
                vec![],
                IgnorePlatform::All,
                None,
            ),
            (
                "ignore list",
                vec![("php", "^7.2.8"), all[0], all[1], ("ext-pdo", "*")],
                vec![],
                vec![],
                IgnorePlatform::List(vec!["php".into(), "ext-pdo".into()]),
                Some("no_php_required"),
            ),
            (
                "ignore wildcard",
                vec![
                    ("php", "^7.2.8"),
                    all[0],
                    all[1],
                    ("ext-fileinfo", "*"),
                    ("ext-filesystem", "*"),
                    ("ext-filter", "*"),
                ],
                vec![],
                vec![],
                IgnorePlatform::List(vec!["php".into(), "ext-fil*".into()]),
                Some("no_php_required"),
            ),
            (
                "no extensions",
                vec![("php", "^7.2")],
                vec![],
                vec![],
                IgnorePlatform::None,
                Some("no_extensions_required"),
            ),
            (
                "replaced/provided",
                vec![
                    ("ext-xml", "^7.2"),
                    ("ext-pdo", "^7.2"),
                    ("ext-bcmath", "^7.2"),
                ],
                vec![("ext-PDO", "7.1.*"), ("ext-BCMath", "^7.1")],
                vec![("ext-XML", "*")],
                IgnorePlatform::None,
                Some("replaced_provided_exts"),
            ),
        ];
        for (label, require, provide, replace, ignore, expected) in cases {
            let (require, provide, replace) = (links(&require), links(&provide), links(&replace));
            let input = PlatformInput {
                name: "root/a",
                dev: false,
                require: &require,
                provide: &provide,
                replace: &replace,
            };
            let out = platform_check(&[input], PlatformCheck::All, &ignore).unwrap();
            assert_eq!(out, expected.map(golden), "{label}");
        }
    }

    #[test]
    fn platform_check_matches_monolog_golden() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/monolog");
        let root = read_root(&dir.join("composer.json")).unwrap();
        let lock = read_lock(&dir.join("composer.lock")).unwrap();
        let empty = Map::new();
        let mut inputs = vec![PlatformInput {
            name: "vivace/fixture-monolog",
            dev: false,
            require: &root.require,
            provide: &empty,
            replace: &empty,
        }];
        inputs.extend(lock.packages(true).map(|p| PlatformInput {
            name: &p.name,
            dev: p.dev,
            require: &p.require,
            provide: &p.provide,
            replace: &p.replace,
        }));
        let expected =
            fs_err::read_to_string(dir.join("expected/dev/composer/platform_check.php")).unwrap();
        assert_eq!(
            platform_check(&inputs, root.config.platform_check, &IgnorePlatform::None).unwrap(),
            Some(expected)
        );
    }

    #[test]
    fn platform_check_off_and_php_only() {
        let require = links(&[("php", ">=8.1"), ("ext-json", "*")]);
        let empty = Map::new();
        let input = || PlatformInput {
            name: "a/a",
            dev: false,
            require: &require,
            provide: &empty,
            replace: &empty,
        };
        assert_eq!(
            platform_check(&[input()], PlatformCheck::Off, &IgnorePlatform::None).unwrap(),
            None
        );
        let php_only = platform_check(&[input()], PlatformCheck::PhpOnly, &IgnorePlatform::None)
            .unwrap()
            .unwrap();
        assert!(php_only.contains("PHP_VERSION_ID >= 80100"));
        assert!(!php_only.contains("extension_loaded"));
    }

    #[test]
    fn platform_check_lower_bound_shapes() {
        let empty = Map::new();
        let check = |constraint: &str| -> Option<String> {
            let require = links(&[("php", constraint)]);
            let input = PlatformInput {
                name: "a/a",
                dev: false,
                require: &require,
                provide: &empty,
                replace: &empty,
            };
            platform_check(&[input], PlatformCheck::PhpOnly, &IgnorePlatform::None)
                .unwrap()
                .map(|s| {
                    let line = s
                        .lines()
                        .find(|l| l.starts_with("if (!(PHP_VERSION_ID"))
                        .unwrap();
                    let msg = s
                        .lines()
                        .find(|l| l.contains("require a PHP version"))
                        .unwrap();
                    format!("{}|{}", line.trim(), msg.split('"').nth(1).unwrap())
                })
        };
        assert_eq!(
            check(">8.1").as_deref(),
            Some("if (!(PHP_VERSION_ID > 80100)) {|> 8.1.0")
        );
        assert_eq!(
            check("~8.1").as_deref(),
            Some("if (!(PHP_VERSION_ID >= 80100)) {|>= 8.1.0")
        );
        assert_eq!(
            check("8.1.*").as_deref(),
            Some("if (!(PHP_VERSION_ID >= 80100)) {|>= 8.1.0")
        );
        assert_eq!(
            check("8.1.2").as_deref(),
            Some("if (!(PHP_VERSION_ID >= 80102)) {|>= 8.1.2")
        );
        assert_eq!(
            check("7.4 - 8.2").as_deref(),
            Some("if (!(PHP_VERSION_ID >= 70400)) {|>= 7.4.0")
        );
        assert_eq!(
            check("^7.4 || ^8.0").as_deref(),
            Some("if (!(PHP_VERSION_ID >= 70400)) {|>= 7.4.0")
        );
        assert_eq!(
            check(">=7.4, <8.3").as_deref(),
            Some("if (!(PHP_VERSION_ID >= 70400)) {|>= 7.4.0")
        );
        assert_eq!(
            check(">=7.4 <8.3").as_deref(),
            Some("if (!(PHP_VERSION_ID >= 70400)) {|>= 7.4.0")
        );
        assert_eq!(check("*"), None);
        assert_eq!(check("<8"), None);
        assert_eq!(check("!=8.0").as_deref(), None);
    }

    #[test]
    fn platform_check_takes_highest_bound_and_skips_dev_requires() {
        let empty = Map::new();
        let a = links(&[("php", ">=7.4")]);
        let b = links(&[("php", ">=8.2")]);
        let c = links(&[("php", ">=8.4")]);
        let inputs = [
            PlatformInput {
                name: "a/a",
                dev: false,
                require: &a,
                provide: &empty,
                replace: &empty,
            },
            PlatformInput {
                name: "b/b",
                dev: false,
                require: &b,
                provide: &empty,
                replace: &empty,
            },
            PlatformInput {
                name: "c/c",
                dev: true,
                require: &c,
                provide: &empty,
                replace: &empty,
            },
        ];
        let out = platform_check(&inputs, PlatformCheck::PhpOnly, &IgnorePlatform::None)
            .unwrap()
            .unwrap();
        assert!(out.contains("PHP_VERSION_ID >= 80200"), "{out}");
    }

    #[test]
    fn platform_check_extension_naming() {
        let empty = Map::new();
        let require = links(&[
            ("ext-zend-opcache", "*"),
            ("ext-pcntl", "*"),
            ("ext-readline", "*"),
            ("ext-Curl", "*"),
        ]);
        let input = PlatformInput {
            name: "a/a",
            dev: false,
            require: &require,
            provide: &empty,
            replace: &empty,
        };
        let out = platform_check(&[input], PlatformCheck::All, &IgnorePlatform::None)
            .unwrap()
            .unwrap();
        let lines: Vec<&str> = out
            .lines()
            .filter(|l| l.contains("missingExtensions[] ="))
            .collect();
        assert_eq!(
            lines,
            [
                "extension_loaded('curl') || $missingExtensions[] = 'curl';",
                "PHP_SAPI !== 'cli' || extension_loaded('pcntl') || $missingExtensions[] = 'pcntl';",
                "PHP_SAPI !== 'cli' || extension_loaded('readline') || $missingExtensions[] = 'readline';",
                "extension_loaded('zend opcache') || $missingExtensions[] = 'zend opcache';",
            ]
        );
    }

    #[test]
    fn platform_check_rejects_unparseable_constraint_naming_package() {
        let empty = Map::new();
        let require = links(&[("php", "~>7.2")]);
        let input = PlatformInput {
            name: "acme/broken",
            dev: false,
            require: &require,
            provide: &empty,
            replace: &empty,
        };
        let err =
            platform_check(&[input], PlatformCheck::PhpOnly, &IgnorePlatform::None).unwrap_err();
        assert!(err.to_string().contains("acme/broken"), "{err}");
        assert!(err.to_string().contains("~>7.2"), "{err}");
    }
}
