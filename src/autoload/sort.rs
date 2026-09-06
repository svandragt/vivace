//! Port of Composer's `PackageSorter::sortPackages`. Decides the order of
//! `autoload_files.php`: dependencies before the packages that require them.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

/// Sort package names so that dependencies precede their dependants.
///
/// `packages` pairs each name with the names it requires. A package's weight
/// is lowered by `1 - importance(user)` for each package that requires it
/// (cycles contribute 0); lower weight sorts first, ties break with
/// `strnatcasecmp`. `weights` seeds starting weights (Composer uses this for
/// plugins; we keep it for fixture parity).
pub fn sort_packages<'a>(packages: &[(&'a str, &[&str])], weights: &[(&str, i64)]) -> Vec<&'a str> {
    let mut usage: HashMap<&str, Vec<&str>> = HashMap::new();
    for (name, requires) in packages {
        for target in *requires {
            usage.entry(target).or_default().push(name);
        }
    }
    let weights: HashMap<&str, i64> = weights.iter().copied().collect();

    let mut computed: HashMap<&str, i64> = HashMap::new();
    let mut computing: HashSet<&str> = HashSet::new();
    let mut order: Vec<(i64, usize, &'a str)> = packages
        .iter()
        .enumerate()
        .map(|(index, (name, _))| {
            let weight = importance(name, &usage, &weights, &mut computed, &mut computing);
            (weight, index, *name)
        })
        .collect();
    order.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| natcasecmp(a.2, b.2)));
    order.into_iter().map(|(_, _, name)| name).collect()
}

fn importance<'u>(
    name: &'u str,
    usage: &HashMap<&'u str, Vec<&'u str>>,
    weights: &HashMap<&str, i64>,
    computed: &mut HashMap<&'u str, i64>,
    computing: &mut HashSet<&'u str>,
) -> i64 {
    if let Some(w) = computed.get(name) {
        return *w;
    }
    if !computing.insert(name) {
        return 0; // cycle
    }
    let mut weight = weights.get(name).copied().unwrap_or(0);
    if let Some(users) = usage.get(name) {
        for user in users {
            weight -= 1 - importance(user, usage, weights, computed, computing);
        }
    }
    computing.remove(name);
    computed.insert(name, weight);
    weight
}

/// PHP `strnatcasecmp`, ported from `strnatcmp_ex` in
/// `ext/standard/strnatcmp.c` (whitespace-skipping omitted: package names
/// don't carry it). A digit run is compared byte-by-byte, left-aligned
/// (`compare_left`) whenever either side's current digit is `0`, otherwise
/// numerically by run length then value (`compare_right`) — `strnatcmp_ex`
/// also strips a string's own leading zeros once up front, which is why e.g.
/// `"0"` vs `"00"` is Equal, not Less.
pub fn natcasecmp(left: &str, right: &str) -> Ordering {
    let (left, right) = (left.to_ascii_lowercase(), right.to_ascii_lowercase());
    let (left, right) = (left.as_bytes(), right.as_bytes());
    let mut pos_l = skip_leading_zeros(left, 0);
    let mut pos_r = skip_leading_zeros(right, 0);
    loop {
        match (left.get(pos_l), right.get(pos_r)) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(l), Some(r)) if l.is_ascii_digit() && r.is_ascii_digit() => {
                let fractional = *l == b'0' || *r == b'0';
                let (ord, next_l, next_r) = if fractional {
                    compare_left(left, pos_l, right, pos_r)
                } else {
                    compare_right(left, pos_l, right, pos_r)
                };
                if ord != Ordering::Equal {
                    return ord;
                }
                (pos_l, pos_r) = (next_l, next_r);
            }
            (Some(l), Some(r)) => {
                if l != r {
                    return l.cmp(r);
                }
                (pos_l, pos_r) = (pos_l + 1, pos_r + 1);
            }
        }
    }
}

/// A string's own leading zeros are only significant relative to the digits
/// that follow, so a run of them collapses to the last one before the loop
/// even starts.
fn skip_leading_zeros(bytes: &[u8], mut pos: usize) -> usize {
    while bytes.get(pos) == Some(&b'0') && bytes.get(pos + 1).is_some_and(u8::is_ascii_digit) {
        pos += 1;
    }
    pos
}

/// The longest digit run wins; equal-length runs fall back to the first
/// differing byte.
fn compare_right(
    left: &[u8],
    mut pos_l: usize,
    right: &[u8],
    mut pos_r: usize,
) -> (Ordering, usize, usize) {
    let mut bias = Ordering::Equal;
    loop {
        match (digit_at(left, pos_l), digit_at(right, pos_r)) {
            (None, None) => return (bias, pos_l, pos_r),
            (None, Some(_)) => return (Ordering::Less, pos_l, pos_r),
            (Some(_), None) => return (Ordering::Greater, pos_l, pos_r),
            (Some(l), Some(r)) => {
                bias = bias.then(l.cmp(&r));
                (pos_l, pos_r) = (pos_l + 1, pos_r + 1);
            }
        }
    }
}

/// Left-aligned, byte-by-byte: the first differing digit wins.
fn compare_left(
    left: &[u8],
    mut pos_l: usize,
    right: &[u8],
    mut pos_r: usize,
) -> (Ordering, usize, usize) {
    loop {
        match (digit_at(left, pos_l), digit_at(right, pos_r)) {
            (None, None) => return (Ordering::Equal, pos_l, pos_r),
            (None, Some(_)) => return (Ordering::Less, pos_l, pos_r),
            (Some(_), None) => return (Ordering::Greater, pos_l, pos_r),
            (Some(l), Some(r)) if l != r => return (l.cmp(&r), pos_l, pos_r),
            _ => (pos_l, pos_r) = (pos_l + 1, pos_r + 1),
        }
    }
}

fn digit_at(bytes: &[u8], pos: usize) -> Option<u8> {
    bytes.get(pos).copied().filter(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use super::{natcasecmp, sort_packages};

    // PHP strnatcasecmp, verified against PHP 8.4 (php -r 'var_dump(strnatcasecmp(...))')
    // and ext/standard/strnatcmp.c: a leading-zero digit run is only special
    // ("fractional", compared left-aligned byte-by-byte) relative to where the
    // comparison currently stands, not by total digit-run length, so e.g.
    // "0" vs "00" and "010" vs "0010" are Equal (both fully consumed as the
    // same left-aligned digits), not Less/Greater.
    #[test]
    fn natcasecmp_matches_php() {
        let cases: &[(&str, &str, Ordering)] = &[
            ("0", "00", Ordering::Equal),
            ("00", "0", Ordering::Equal),
            ("010", "0010", Ordering::Equal),
            ("bar2", "bar10", Ordering::Less),
            ("Bar2", "bar10", Ordering::Less),
            ("a", "b", Ordering::Less),
            ("x10", "x9", Ordering::Greater),
        ];
        for (a, b, expected) in cases {
            assert_eq!(natcasecmp(a, b), *expected, "natcasecmp({a:?}, {b:?})");
        }
    }

    // The actual bug: a digit run with a leading zero *mid-string* (not at
    // the very start) must dispatch to a left-aligned, byte-by-byte compare,
    // not "strip leading zeros then compare digit-run lengths".
    #[test]
    fn natcasecmp_leading_zero_mid_string() {
        assert_eq!(natcasecmp("a01b", "a1b"), Ordering::Less);
        assert_eq!(natcasecmp("a1b", "a01b"), Ordering::Greater);
    }

    fn check(input: &[(&str, &[&str])], expected: &[&str], weights: &[(&str, i64)]) {
        assert_eq!(sort_packages(input, weights), expected);
    }

    // composer/composer tests/Composer/Test/Util/PackageSorterTest.php
    #[test]
    fn no_dependencies_keeps_order() {
        check(
            &[
                ("foo/bar1", &[]),
                ("foo/bar2", &[]),
                ("foo/bar3", &[]),
                ("foo/bar4", &[]),
            ],
            &["foo/bar1", "foo/bar2", "foo/bar3", "foo/bar4"],
            &[],
        );
    }

    #[test]
    fn one_package_is_dep() {
        check(
            &[
                ("foo/bar1", &["foo/bar4"]),
                ("foo/bar2", &["foo/bar4"]),
                ("foo/bar3", &["foo/bar4"]),
                ("foo/bar4", &[]),
            ],
            &["foo/bar4", "foo/bar1", "foo/bar2", "foo/bar3"],
            &[],
        );
    }

    #[test]
    fn one_package_has_more_deps() {
        check(
            &[
                ("foo/bar1", &["foo/bar2"]),
                ("foo/bar2", &["foo/bar4"]),
                ("foo/bar3", &["foo/bar4"]),
                ("foo/bar4", &[]),
            ],
            &["foo/bar4", "foo/bar2", "foo/bar1", "foo/bar3"],
            &[],
        );
    }

    #[test]
    fn required_by_many_but_requires_one() {
        check(
            &[
                ("foo/bar1", &["foo/bar3"]),
                ("foo/bar2", &["foo/bar3"]),
                ("foo/bar3", &["foo/bar4"]),
                ("foo/bar4", &[]),
                ("foo/bar5", &["foo/bar3"]),
                ("foo/bar6", &["foo/bar3"]),
            ],
            &[
                "foo/bar4", "foo/bar3", "foo/bar1", "foo/bar2", "foo/bar5", "foo/bar6",
            ],
            &[],
        );
    }

    #[test]
    fn one_package_has_many_requires() {
        check(
            &[
                ("foo/bar1", &["foo/bar2"]),
                ("foo/bar2", &[]),
                ("foo/bar3", &["foo/bar4"]),
                ("foo/bar4", &[]),
                ("foo/bar5", &["foo/bar2"]),
                ("foo/bar6", &["foo/bar2"]),
            ],
            &[
                "foo/bar2", "foo/bar4", "foo/bar1", "foo/bar3", "foo/bar5", "foo/bar6",
            ],
            &[],
        );
    }

    #[test]
    fn circular_deps_sorted_alphabetically_when_equal() {
        check(
            &[
                ("foo/bar1", &["circular/part1"]),
                ("foo/bar2", &["circular/part2"]),
                ("circular/part1", &["circular/part2"]),
                ("circular/part2", &["circular/part1"]),
            ],
            &["circular/part1", "circular/part2", "foo/bar1", "foo/bar2"],
            &[],
        );
    }

    #[test]
    fn equal_weight_sorted_naturally() {
        check(
            &[
                ("foo/bar10", &["foo/dep"]),
                ("foo/bar2", &["foo/dep"]),
                ("foo/baz", &["foo/dep"]),
                ("foo/dep", &[]),
            ],
            &["foo/dep", "foo/bar2", "foo/bar10", "foo/baz"],
            &[],
        );
    }

    #[test]
    fn pre_weighted_packages_bumped_with_their_deps() {
        check(
            &[
                ("foo/bar", &["foo/dep"]),
                ("foo/bar2", &["foo/dep2"]),
                ("foo/dep", &[]),
                ("foo/dep2", &[]),
            ],
            &["foo/dep", "foo/bar", "foo/dep2", "foo/bar2"],
            &[("foo/bar", -1000)],
        );
    }
}
