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

/// PHP `strnatcasecmp`: case-insensitive, digit runs compared numerically.
pub fn natcasecmp(a: &str, b: &str) -> Ordering {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    let (mut a, mut b) = (a.as_bytes(), b.as_bytes());
    loop {
        match (a.first(), b.first()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) if x.is_ascii_digit() && y.is_ascii_digit() => {
                let na = a.iter().take_while(|c| c.is_ascii_digit()).count();
                let nb = b.iter().take_while(|c| c.is_ascii_digit()).count();
                let (da, db) = (&a[..na], &b[..nb]);
                let (ta, tb) = (trim_zeros(da), trim_zeros(db));
                let ord = ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb));
                if ord != Ordering::Equal {
                    return ord;
                }
                a = &a[na..];
                b = &b[nb..];
            }
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(y);
                }
                a = &a[1..];
                b = &b[1..];
            }
        }
    }
}

fn trim_zeros(d: &[u8]) -> &[u8] {
    let n = d.iter().take_while(|c| **c == b'0').count();
    &d[n.min(d.len().saturating_sub(1))..]
}

#[cfg(test)]
mod tests {
    use super::sort_packages;

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
