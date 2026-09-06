//! Composer version normalisation, a port of `composer/semver`'s
//! `VersionParser::normalize`. Used for `version_normalized` in
//! `installed.json` and `version` in `installed.php`.

use std::sync::LazyLock;

use anyhow::{Result, bail};
use regex::Regex;

const MODIFIER: &str =
    r"[._-]?(?:(stable|beta|b|RC|alpha|a|patch|pl|p)((?:[.-]?\d+)*)?)?([.-]?dev)?";
const STABILITIES: &str = "stable|RC|beta|alpha|dev";

static ALIAS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([^,\s]+) +as +([^,\s]+)$").unwrap());
static STABILITY_FLAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("(?i)@(?:{STABILITIES})$")).unwrap());
static BUILD_META: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^([^,\s+]+)\+[^\s]+$").unwrap());
static CLASSICAL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^v?(\d{{1,5}})(\.\d+)?(\.\d+)?(\.\d+)?{MODIFIER}$"
    ))
    .unwrap()
});
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?i)^v?(\d{{4}}(?:[.:-]?\d{{2}}){{1,6}}(?:[.:-]?\d{{1,3}}){{0,2}}){MODIFIER}$"
    ))
    .unwrap()
});
static DEV_SUFFIX: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)^(.*?)[.-]?dev$").unwrap());
static BRANCH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^v?(\d+)(\.(?:\d+|[xX*]))?(\.(?:\d+|[xX*]))?(\.(?:\d+|[xX*]))?$").unwrap()
});

/// Normalise a Composer version string to its four-component canonical form
/// (`1.2.3.4`, `1.0.0.0-beta2`, `dev-main`, `1.9999999.9999999.9999999-dev`).
pub fn normalize(version: &str) -> Result<String> {
    let orig = version;
    let mut version = version.trim();

    if let Some(m) = ALIAS.captures(version) {
        version = m.get(1).unwrap().as_str();
    }
    if let Some(m) = STABILITY_FLAG.find(version) {
        version = &version[..m.start()];
    }
    let master;
    if matches!(version, "master" | "trunk" | "default") {
        master = format!("dev-{version}");
        version = &master;
    }
    if version
        .get(..4)
        .is_some_and(|s| s.eq_ignore_ascii_case("dev-"))
    {
        return Ok(format!("dev-{}", &version[4..]));
    }
    if let Some(m) = BUILD_META.captures(version) {
        version = m.get(1).unwrap().as_str();
    }

    let (mut out, modifier) = if let Some(m) = CLASSICAL.captures(version) {
        let part = |i: usize| m.get(i).map_or(".0", |g| g.as_str());
        let out = format!("{}{}{}{}", &m[1], part(2), part(3), part(4));
        (out, Some((m.get(5), m.get(6), m.get(7))))
    } else if let Some(m) = DATE.captures(version) {
        let out: String = m[1]
            .chars()
            .map(|c| if c.is_ascii_digit() { c } else { '.' })
            .collect();
        (out, Some((m.get(2), m.get(3), m.get(4))))
    } else {
        (String::new(), None)
    };

    if let Some((stability, number, dev)) = modifier {
        if let Some(st) = stability.filter(|g| !g.as_str().is_empty()) {
            if st.as_str().eq_ignore_ascii_case("stable") {
                return Ok(out);
            }
            out.push('-');
            out.push_str(expand_stability(st.as_str()));
            if let Some(n) = number {
                out.push_str(n.as_str().trim_start_matches(['.', '-']));
            }
        }
        if dev.is_some_and(|g| !g.as_str().is_empty()) {
            out.push_str("-dev");
        }
        return Ok(out);
    }

    if let Some(m) = DEV_SUFFIX.captures(version) {
        let normalized = normalize_branch(&m[1]);
        if !normalized.contains("dev-") {
            return Ok(normalized);
        }
    }

    bail!("Invalid version string \"{orig}\"")
}

/// Composer's `normalizeBranch`: `1.x` becomes `1.9999999.9999999.9999999-dev`,
/// anything non-numeric becomes `dev-<name>`.
pub fn normalize_branch(name: &str) -> String {
    let name = name.trim();
    if let Some(m) = BRANCH.captures(name) {
        let mut out = String::new();
        for i in 1..5 {
            match m.get(i) {
                Some(g) => out.push_str(&g.as_str().replace(['*', 'X'], "x")),
                None => out.push_str(".x"),
            }
        }
        return format!("{}-dev", out.replace('x', "9999999"));
    }
    format!("dev-{name}")
}

fn expand_stability(s: &str) -> &'static str {
    match s.to_ascii_lowercase().as_str() {
        "a" | "alpha" => "alpha",
        "b" | "beta" => "beta",
        "p" | "pl" | "patch" => "patch",
        "rc" => "RC",
        _ => "stable",
    }
}

#[cfg(test)]
mod tests {
    use super::normalize;

    // composer/semver tests/VersionParserTest.php::successfulNormalizedVersions
    const OK: &[(&str, &str)] = &[
        ("1.0.0", "1.0.0.0"),
        ("1.2.3.4", "1.2.3.4"),
        ("1.0.0RC1dev", "1.0.0.0-RC1-dev"),
        ("1.0.0-rC15-dev", "1.0.0.0-RC15-dev"),
        ("1.0.0.RC.15-dev", "1.0.0.0-RC15-dev"),
        ("1.0.0-rc1", "1.0.0.0-RC1"),
        ("1.0.0.pl3-dev", "1.0.0.0-patch3-dev"),
        ("1.0-dev", "1.0.0.0-dev"),
        ("0", "0.0.0.0"),
        ("99999", "99999.0.0.0"),
        ("10.4.13-beta", "10.4.13.0-beta"),
        ("10.4.13beta2", "10.4.13.0-beta2"),
        ("10.4.13beta.2", "10.4.13.0-beta2"),
        ("v1.13.11-beta.0", "1.13.11.0-beta0"),
        ("1.13.11.0-beta0", "1.13.11.0-beta0"),
        ("10.4.13-b", "10.4.13.0-beta"),
        ("10.4.13-b5", "10.4.13.0-beta5"),
        ("v1.0.0", "1.0.0.0"),
        ("2010.01", "2010.01.0.0"),
        ("2010.01.02", "2010.01.02.0"),
        ("2010.1.555", "2010.1.555.0"),
        ("2010.10.200", "2010.10.200.0"),
        ("20230131.0.0", "20230131.0.0"),
        ("202301310000.0.0", "202301310000.0.0"),
        ("v20100102", "20100102"),
        ("20100102", "20100102"),
        ("20100102.0", "20100102.0"),
        ("20100102.1.0", "20100102.1.0"),
        ("20100102.0.3", "20100102.0.3"),
        ("100000", "100000"),
        ("2010-01-02-10-20-30.0.3", "2010.01.02.10.20.30.0.3"),
        ("2010-01-02-10-20-30.5", "2010.01.02.10.20.30.5"),
        ("2010-01-02", "2010.01.02"),
        ("2012.06.07", "2012.06.07.0"),
        ("2010-01-02.5", "2010.01.02.5"),
        ("20100102-203040", "20100102.203040"),
        ("20100102.x-dev", "20100102.9999999.9999999.9999999-dev"),
        (
            "20100102.203040.x-dev",
            "20100102.203040.9999999.9999999-dev",
        ),
        ("20100102203040-10", "20100102203040.10"),
        ("20100102-203040-p1", "20100102.203040-patch1"),
        ("201903.0", "201903.0"),
        ("201903.x-dev", "201903.9999999.9999999.9999999-dev"),
        ("201903.0-p2", "201903.0-patch2"),
        ("dev-master", "dev-master"),
        ("master", "dev-master"),
        ("dev-trunk", "dev-trunk"),
        ("1.x-dev", "1.9999999.9999999.9999999-dev"),
        ("dev-feature-foo", "dev-feature-foo"),
        ("DEV-FOOBAR", "dev-FOOBAR"),
        ("dev-feature/foo", "dev-feature/foo"),
        ("dev-feature+issue-1", "dev-feature+issue-1"),
        ("dev-master as 1.0.0", "dev-master"),
        (
            "dev-load-varnish-only-when-used as ^2.0",
            "dev-load-varnish-only-when-used",
        ),
        (
            "dev-load-varnish-only-when-used@dev as ^2.0@dev",
            "dev-load-varnish-only-when-used",
        ),
        ("1.0.0+foo@dev", "1.0.0.0"),
        (
            "dev-load-varnish-only-when-used@stable",
            "dev-load-varnish-only-when-used",
        ),
        ("1.0.0-beta.5+foo", "1.0.0.0-beta5"),
        ("1.0.0+foo", "1.0.0.0"),
        ("1.0.0-alpha.3.1+foo", "1.0.0.0-alpha3.1"),
        ("1.0.0-alpha2.1+foo", "1.0.0.0-alpha2.1"),
        ("1.0.0-alpha-2.1-3+foo", "1.0.0.0-alpha2.1-3"),
        ("1.0.0+foo as 2.0", "1.0.0.0"),
        ("00.01.03.04", "00.01.03.04"),
        ("000.001.003.004", "000.001.003.004"),
        ("0.000.103.204", "0.000.103.204"),
        ("0700", "0700.0.0.0"),
        ("041.x-dev", "041.9999999.9999999.9999999-dev"),
        ("dev-041.003", "dev-041.003"),
        ("dev-1.0.0-dev<1.0.5-dev", "dev-1.0.0-dev<1.0.5-dev"),
        ("dev-foo bar", "dev-foo bar"),
        (" 1.0.0", "1.0.0.0"),
        ("1.0.0 ", "1.0.0.0"),
    ];

    // composer/semver tests/VersionParserTest.php::failingNormalizedVersions
    const FAIL: &[&str] = &[
        "",
        "a",
        "1.0.0-meh",
        "1.0.0.0.0",
        "feature-foo",
        "1.0.0+foo bar",
        "1.0.1-SNAPSHOT",
        "1.0.0<1.0.5-dev",
        "1.0.0-dev<1.0.5-dev",
        "foo bar-dev",
        "1.0 .2",
        " as ",
        " as 1.2",
        "^",
        "^8 || ^",
        "~",
        "~1 ~",
        "~1",
        "^1",
        "1.*",
        "20100102.0.3.4",
        "100000.0.0.0",
        "2023013.0.0",
        "202301311.0.0",
        "20230131000.0.0",
        "2023013100000.0.0",
    ];

    #[test]
    fn normalizes_like_composer() {
        for (input, expected) in OK {
            let got = normalize(input).unwrap_or_else(|e| panic!("{input:?}: {e}"));
            assert_eq!(&got, expected, "input {input:?}");
        }
    }

    #[test]
    fn rejects_like_composer() {
        for input in FAIL {
            assert!(
                normalize(input).is_err(),
                "expected {input:?} to be rejected"
            );
        }
    }

    // A multi-byte char straddling the `version[..4]` slice point must be
    // rejected, not panic.
    #[test]
    fn dev_prefix_check_is_byte_boundary_safe() {
        assert!(normalize("café-dev").is_err());
        assert_eq!(normalize("dev-café").unwrap(), "dev-café");
    }
}
