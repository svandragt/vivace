//! `viv normalize`: a native `ergebnis/composer-normalize` for `composer.json`.
//!
//! Ports the normalizers `ComposerJsonNormalizer` chains, scoped to what a
//! plain `composer.json` (no plugin-order pointers, no schema validation)
//! needs: top-level keys reordered to Composer's JSON schema property order
//! (`res/composer-schema.json`), package links (`require`, `require-dev`,
//! `replace`, `conflict`, `provide`, `suggest`) key-sorted with platform
//! packages first, `config` key-sorted (with `allow-plugins`/
//! `preferred-install` wildcard-aware), `bin` sorted, and version constraint
//! spacing collapsed. Not ported: the generic recursive JSON-schema
//! normalizer that also alphabetises nested objects with no schema
//! properties of their own (`autoload` namespaces, `extra`), and
//! `VersionConstraintNormalizer`'s semantic rewrites (wildcard/tilde/caret
//! conversion, dedup, sorting, overlap removal) — out of scope for this
//! issue, see the PR description.

use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::Args;
use serde_json::{Map, Value};

use crate::autoload::sort::natcmp;

/// `viv normalize`'s own default, and the indent `viv install`/
/// `viv dump-autoload` normalize with (they have no `--indent-size` flag of
/// their own).
const DEFAULT_INDENT_SIZE: usize = 4;

/// `viv normalize` flags.
#[derive(Args, Debug, Clone)]
pub struct NormalizeArgs {
    /// Project directory holding `composer.json`.
    #[arg(short = 'd', long = "project-dir", default_value = ".")]
    pub project_dir: PathBuf,
    /// Check whether `composer.json` is normalized without writing; exits 1
    /// and prints a diff if it is not.
    #[arg(long)]
    pub check: bool,
    /// Spaces per indent level.
    #[arg(long = "indent-size", default_value_t = DEFAULT_INDENT_SIZE)]
    pub indent_size: usize,
}

/// Composer's JSON schema (`res/composer-schema.json`) top-level `properties`
/// order. Keys `composer.json` carries that aren't in this list (a plugin's
/// own extension of the schema) are ksorted and appended, mirroring the
/// generic `SchemaNormalizer`'s `additionalProperties` handling.
const KEY_ORDER: &[&str] = &[
    "name",
    "description",
    "license",
    "type",
    "abandoned",
    "version",
    "default-branch",
    "non-feature-branches",
    "keywords",
    "readme",
    "time",
    "authors",
    "homepage",
    "support",
    "funding",
    "source",
    "dist",
    "require",
    "require-dev",
    "replace",
    "conflict",
    "provide",
    "suggest",
    "repositories",
    "minimum-stability",
    "prefer-stable",
    "autoload",
    "autoload-dev",
    "target-dir",
    "include-path",
    "bin",
    "archive",
    "php-ext",
    "config",
    "extra",
    "scripts",
    "scripts-descriptions",
    "scripts-aliases",
];

/// `PackageHashNormalizer`'s properties: package links, key-sorted with
/// platform packages first (`PLATFORM_PACKAGE_REGEX`).
const PACKAGE_LINKS: &[&str] = &[
    "conflict",
    "provide",
    "replace",
    "require",
    "require-dev",
    "suggest",
];

/// `VersionConstraintNormalizer`'s properties: package links whose *values*
/// are version constraints, so `suggest` (free-text) is excluded.
const VERSION_CONSTRAINT_LINKS: &[&str] =
    &["conflict", "provide", "replace", "require", "require-dev"];

/// `WildcardSorter`'s two callers in `ConfigHashNormalizer`.
const WILDCARD_SORTED_CONFIG_KEYS: &[&str] = &["allow-plugins", "preferred-install"];

/// `viv normalize`: normalize `composer.json` in place, or with `--check`,
/// report whether it would change without writing.
pub fn run(args: &NormalizeArgs) -> Result<()> {
    let project_dir = fs_err::canonicalize(&args.project_dir)
        .with_context(|| format!("{}: project directory", args.project_dir.display()))?;
    let path = project_dir.join("composer.json");
    let original = fs_err::read_to_string(&path)?;
    let normalized = normalize(&original, args.indent_size)
        .with_context(|| format!("{}: normalizing", path.display()))?;

    if original == normalized {
        return Ok(());
    }
    if args.check {
        bail!(
            "{} is not normalized:\n{}",
            path.display(),
            unified_diff(&original, &normalized)
        );
    }
    write_atomic(&path, normalized.as_bytes())?;
    out(&format!("Normalized {}", path.display()));
    Ok(())
}

/// `viv install`/`viv dump-autoload`'s pre-read step (unless `--no-normalize`):
/// rewrite `composer.json` in place if normalizing it changes any bytes,
/// using the same default indent as a bare `viv normalize`. A file that
/// can't be read or fails to parse is left untouched so the caller's own
/// read/parse reports the real error instead of this one masking it.
/// Returns whether it rewrote the file, for the caller's one stderr line.
pub fn maybe_normalize(path: &std::path::Path) -> Result<bool> {
    let Ok(original) = fs_err::read_to_string(path) else {
        return Ok(false);
    };
    let Ok(normalized) = normalize(&original, DEFAULT_INDENT_SIZE) else {
        return Ok(false);
    };
    if original == normalized {
        return Ok(false);
    }
    write_atomic(path, normalized.as_bytes())?;
    Ok(true)
}

/// stdout via `writeln!`, not `println!`, to satisfy the `print_stdout` lint.
fn out(message: &str) {
    let _ = writeln!(std::io::stdout().lock(), "{message}");
}

/// Write `content` to `path` via a temp file in the same directory renamed
/// over the target, so a reader never sees a partial write. Mirrors
/// `install::write_atomic`, kept local so this module doesn't need that
/// function made `pub(crate)`.
fn write_atomic(path: &std::path::Path, content: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    let mut temp = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("creating a temp file next to {}", path.display()))?;
    temp.write_all(content)?;
    temp.persist(path).map_err(|err| err.error)?;
    Ok(())
}

/// Apply every normalizer and re-serialize with `indent_size`-space indent,
/// unescaped slashes and unicode (`serde_json`'s defaults), trailing
/// newline.
fn normalize(content: &str, indent_size: usize) -> Result<String> {
    let value: Value = serde_json::from_str(content).context("parsing composer.json")?;
    let Value::Object(mut root) = value else {
        bail!("composer.json's root is not an object");
    };

    for &key in PACKAGE_LINKS {
        if let Some(Value::Object(links)) = root.get_mut(key) {
            sort_package_links(links);
        }
    }
    for &key in VERSION_CONSTRAINT_LINKS {
        if let Some(Value::Object(links)) = root.get_mut(key) {
            for value in links.values_mut() {
                if let Value::String(constraint) = value {
                    *constraint = normalize_constraint_spacing(constraint);
                }
            }
        }
    }
    if let Some(Value::Array(bin)) = root.get_mut("bin") {
        bin.sort_by(|a, b| {
            a.as_str()
                .unwrap_or_default()
                .cmp(b.as_str().unwrap_or_default())
        });
    }
    if let Some(Value::Object(config)) = root.get_mut("config") {
        sort_config(config);
    }

    let ordered = reorder_top_level(root);

    let indent = " ".repeat(indent_size);
    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&Value::Object(ordered), &mut serializer)?;
    buf.push(b'\n');
    Ok(String::from_utf8(buf)?)
}

/// Schema-order keys first (only those present), then anything else
/// ksorted and appended.
fn reorder_top_level(mut root: Map<String, Value>) -> Map<String, Value> {
    let mut ordered = Map::new();
    for &key in KEY_ORDER {
        if let Some(value) = root.remove(key) {
            ordered.insert(key.to_string(), value);
        }
    }
    let mut rest: Vec<(String, Value)> = root.into_iter().collect();
    rest.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, value) in rest {
        ordered.insert(key, value);
    }
    ordered
}

/// `PackageHashNormalizer::sortPackages`: platform packages (`php`, `hhvm`,
/// `ext-*`, `lib-*`, other `PlatformRepository` platform names such as
/// `composer-plugin-api`) sort before everything else, in that order; within
/// each group, `strnatcmp` on the name. Composer's
/// `mergeDuplicateExtensions` (folding `ext-FOO` case/space variants
/// together) is not ported: no fixture needs it, and duplicate keys are rare
/// in a hand-maintained `composer.json`.
fn sort_package_links(links: &mut Map<String, Value>) {
    let mut entries: Vec<(String, Value)> = std::mem::take(links).into_iter().collect();
    entries.sort_by(|a, b| {
        platform_rank(&a.0)
            .cmp(&platform_rank(&b.0))
            .then_with(|| natcmp(&a.0, &b.0))
    });
    *links = entries.into_iter().collect();
}

/// `PlatformRepository::PLATFORM_PACKAGE_REGEX`, matched against the whole
/// name.
fn is_platform_package(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "php"
            | "php-64bit"
            | "php-ipv6"
            | "php-zts"
            | "php-debug"
            | "hhvm"
            | "composer-plugin-api"
            | "composer-runtime-api"
    ) {
        return true;
    }
    let Some(rest) = lower
        .strip_prefix("ext-")
        .or_else(|| lower.strip_prefix("lib-"))
    else {
        return false;
    };
    !rest.is_empty()
        && rest
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
}

fn platform_rank(name: &str) -> u8 {
    if !is_platform_package(name) {
        return 5;
    }
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("php") {
        0
    } else if lower.starts_with("hhvm") {
        1
    } else if lower.starts_with("ext-") {
        2
    } else if lower.starts_with("lib-") {
        3
    } else {
        4
    }
}

/// `VersionConstraintNormalizer`'s formatting step only (`trim`,
/// `removeExtraSpaces`, `normalizeVersionConstraintSeparators`): collapse
/// runs of whitespace, then a single space around `||` and between
/// AND-ed constraints. Not ported: the semantic rewrites further down that
/// method (leading `v` stripped, `1.x` to `1.*` to `~1.0` to `^1`, alias and
/// overlap removal, constraint sorting) — real inputs `viv normalize` is
/// expected to see are already close to normal form; add these if a fixture
/// needs them.
fn normalize_constraint_spacing(constraint: &str) -> String {
    constraint
        .split('|')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|or_part| or_part.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join(" || ")
}

/// `ConfigHashNormalizer`: ksort `config`'s own keys, then wildcard-aware
/// sort `allow-plugins`/`preferred-install`'s keys (`*` sorts last within
/// its group, via `str::replace('*', '~')` before `strcmp`).
fn sort_config(config: &mut Map<String, Value>) {
    let mut entries: Vec<(String, Value)> = std::mem::take(config).into_iter().collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    for (key, value) in &mut entries {
        if WILDCARD_SORTED_CONFIG_KEYS.contains(&key.as_str())
            && let Value::Object(map) = value
        {
            let mut sub: Vec<(String, Value)> = std::mem::take(map).into_iter().collect();
            sub.sort_by_key(|a| a.0.replace('*', "~"));
            *map = sub.into_iter().collect();
        }
    }
    *config = entries.into_iter().collect();
}

/// A minimal unified-ish diff for `--check`'s error message: no context
/// lines or hunk headers, just every line that differs (`-`/`+`), matched up
/// by the longest common subsequence of unchanged lines. `composer.json`
/// files are small, so the `O(n*m)` LCS table costs nothing worth avoiding.
fn unified_diff(before: &str, after: &str) -> String {
    let before: Vec<&str> = before.lines().collect();
    let after: Vec<&str> = after.lines().collect();
    let (n, m) = (before.len(), after.len());
    let mut lcs = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if before[i] == after[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = String::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if before[i] == after[j] {
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push('-');
            out.push_str(before[i]);
            out.push('\n');
            i += 1;
        } else {
            out.push('+');
            out.push_str(after[j]);
            out.push('\n');
            j += 1;
        }
    }
    for line in &before[i..n] {
        out.push('-');
        out.push_str(line);
        out.push('\n');
    }
    for line in &after[j..m] {
        out.push('+');
        out.push_str(line);
        out.push('\n');
    }
    out.pop();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_level_keys_reorder_to_schema_order() {
        let out = normalize(r#"{"license":"MIT","name":"a/b"}"#, 4).unwrap();
        assert_eq!(
            out,
            "{\n    \"name\": \"a/b\",\n    \"license\": \"MIT\"\n}\n"
        );
    }

    #[test]
    fn unknown_keys_are_ksorted_after_schema_keys() {
        let out = normalize(r#"{"zzz-ext":"z","name":"a/b","aaa-ext":"a"}"#, 4).unwrap();
        assert_eq!(
            out,
            "{\n    \"name\": \"a/b\",\n    \"aaa-ext\": \"a\",\n    \"zzz-ext\": \"z\"\n}\n"
        );
    }

    #[test]
    fn package_links_sort_platform_first_then_alphabetical() {
        let out = normalize(
            r#"{"require":{"psr/log":"^1.0","ext-json":"*","php":">=7.4","monolog/monolog":"^2.0","lib-icu":"*"}}"#,
            4,
        )
        .unwrap();
        assert_eq!(
            out,
            "{\n    \"require\": {\n        \"php\": \">=7.4\",\n        \"ext-json\": \"*\",\n        \"lib-icu\": \"*\",\n        \"monolog/monolog\": \"^2.0\",\n        \"psr/log\": \"^1.0\"\n    }\n}\n"
        );
    }

    #[test]
    fn bin_is_sorted() {
        let out = normalize(r#"{"bin":["bin/zeta","bin/alpha"]}"#, 4).unwrap();
        assert_eq!(
            out,
            "{\n    \"bin\": [\n        \"bin/alpha\",\n        \"bin/zeta\"\n    ]\n}\n"
        );
    }

    #[test]
    fn config_is_ksorted_and_allow_plugins_wildcard_sorted() {
        let out = normalize(
            r#"{"config":{"sort-packages":true,"allow-plugins":{"z/p":true,"a/p":true},"optimize-autoloader":true}}"#,
            4,
        )
        .unwrap();
        assert_eq!(
            out,
            "{\n    \"config\": {\n        \"allow-plugins\": {\n            \"a/p\": true,\n            \"z/p\": true\n        },\n        \"optimize-autoloader\": true,\n        \"sort-packages\": true\n    }\n}\n"
        );
    }

    #[test]
    fn version_constraint_spacing_is_collapsed() {
        let out = normalize(r#"{"require":{"psr/log":">=1.0.0  ||  >=2.0.0"}}"#, 4).unwrap();
        assert_eq!(
            out,
            "{\n    \"require\": {\n        \"psr/log\": \">=1.0.0 || >=2.0.0\"\n    }\n}\n"
        );
    }

    #[test]
    fn already_normalized_is_a_no_op() {
        let normalized = normalize(r#"{"name":"a/b"}"#, 4).unwrap();
        assert_eq!(normalize(&normalized, 4).unwrap(), normalized);
    }

    #[test]
    fn indent_size_is_configurable() {
        let out = normalize(r#"{"name":"a/b"}"#, 2).unwrap();
        assert_eq!(out, "{\n  \"name\": \"a/b\"\n}\n");
    }

    #[test]
    fn diff_lists_changed_lines() {
        let diff = unified_diff("a\nb\nc", "a\nx\nc");
        assert_eq!(diff, "-b\n+x");
    }

    #[test]
    fn maybe_normalize_rewrites_only_when_it_changes_bytes() {
        let messy = r#"{"license":"MIT","name":"a/b"}"#;
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(messy.as_bytes()).unwrap();

        assert!(maybe_normalize(file.path()).unwrap());
        let rewritten = fs_err::read_to_string(file.path()).unwrap();
        assert_eq!(rewritten, normalize(messy, DEFAULT_INDENT_SIZE).unwrap());

        assert!(
            !maybe_normalize(file.path()).unwrap(),
            "already-normalized bytes should not be rewritten"
        );
    }

    #[test]
    fn maybe_normalize_leaves_unparsable_json_untouched() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(b"not json").unwrap();

        assert!(!maybe_normalize(file.path()).unwrap());
        assert_eq!(fs_err::read_to_string(file.path()).unwrap(), "not json");
    }

    /// Task #14 follow-up's premise: normalizing only reorders keys and
    /// whitespace, and `Locker::getContentHash` is computed over a ksorted
    /// relevant subset, so normalizing a fixture must never change its
    /// content-hash.
    #[test]
    fn normalizing_does_not_change_the_content_hash() {
        let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let fixture = manifest.join("tests/fixtures/monolog");
        let original = fs_err::read_to_string(fixture.join("composer.json")).unwrap();
        let normalized = normalize(&original, DEFAULT_INDENT_SIZE).unwrap();

        let lock = crate::lock::read_lock(&fixture.join("composer.lock")).unwrap();
        crate::lock::validate_against_root(&lock, normalized.as_bytes())
            .expect("normalizing changed content-hash-relevant bytes");
    }
}
