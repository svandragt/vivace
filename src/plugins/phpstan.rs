//! `phpstan/extension-installer`'s `process` listener: writes
//! `vendor/phpstan/extension-installer/src/GeneratedConfig.php`, a `var_export`
//! dump of every installed `phpstan-extension`/`extra.phpstan` package.
//!
//! Ported from `phpstan/extension-installer` `1.4.3`'s `src/Plugin.php`,
//! fetched 2026-09-06.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result};
use semver_php::{
    Bound, Constraint, Interval, Intervals, MultiConstraint, Operator, VersionParser,
};
use serde_json::{Map, Value, json};

use super::php_string;
use super::phpcs::relative_path;
use super::{Adapter, Ctx};
use crate::lock::{Package, Root};

const PACKAGE_NAME: &str = "phpstan/extension-installer";

pub(super) struct Phpstan;

impl Adapter for Phpstan {
    fn plugin_names(&self) -> &'static [&'static str] {
        &[PACKAGE_NAME]
    }

    fn upstream_version(&self) -> &'static str {
        "1.4.3"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/plugins/phpstan"
    }

    fn post_install(&self, ctx: &Ctx<'_>, bin_packages: &[(&Package, PathBuf)]) -> Result<()> {
        apply(ctx.root, bin_packages)
    }
}

const PACKAGE_TYPE: &str = "phpstan-extension";
const PHPSTAN_REQUIRE: &str = "phpstan/phpstan";
/// `strpos($name, 'phpstan') !== false` but not one of these: excluded from
/// `NOT_INSTALLED` even though its name contains "phpstan".
const NOT_PHPSTAN_EXTENSIONS: &[&str] = &[
    "phpstan/phpstan",
    "phpstan/phpstan-shim",
    "phpstan/phpdoc-parser",
    "phpstan/extension-installer",
];

pub(super) fn apply(root: &Root, packages: &[(&Package, PathBuf)]) -> Result<()> {
    let Some((_, installer_dir)) = packages.iter().find(|(p, _)| p.name == PACKAGE_NAME) else {
        return Ok(());
    };
    let generated_config_dir = installer_dir.join("src");
    let ignore = ignored_packages(root);

    let mut data: BTreeMap<String, Value> = BTreeMap::new();
    let mut not_installed: BTreeMap<String, Value> = BTreeMap::new();
    let mut constraints: Vec<Box<dyn Constraint>> = Vec::new();

    for (package, install_dir) in packages {
        let extra = package.raw.pointer("/extra/phpstan").cloned();
        if package.r#type != PACKAGE_TYPE && extra.is_none() {
            if package.name.contains("phpstan")
                && !NOT_PHPSTAN_EXTENSIONS.contains(&package.name.as_str())
            {
                not_installed.insert(package.name.clone(), json!(package.version));
            }
            continue;
        }
        if ignore.contains(&package.name) {
            continue;
        }

        let mut constraint_str = None;
        if let Some(spec) = package.require.get(PHPSTAN_REQUIRE).and_then(Value::as_str) {
            let constraint = VersionParser::parse_constraints(spec).with_context(|| {
                format!("{}: parsing {PHPSTAN_REQUIRE} constraint", package.name)
            })?;
            let (lower, upper) = (constraint.lower_bound(), constraint.upper_bound());
            // `$phpstanConstraint->getLowerBound()->isZero()` / `isPositiveInfinity()`:
            // an unconstrained requirement drops the *whole package* from
            // `EXTENSIONS`, not just this field — a real quirk of the plugin.
            if lower.is_zero() || upper.is_positive_infinity() {
                continue;
            }
            constraint_str = Some(constraint_into_string(&lower, &upper));
            constraints.push(constraint);
        }

        data.insert(
            package.name.clone(),
            json!({
                "install_path": install_dir.display().to_string(),
                "relative_install_path": relative_path(&generated_config_dir, install_dir),
                "extra": extra,
                "version": package.version,
                "phpstanVersionConstraint": constraint_str,
            }),
        );
    }

    let global_constraint = merge_constraints(constraints);
    let body = render(
        &to_map(data),
        &to_map(not_installed),
        global_constraint.as_deref(),
    );
    let path = generated_config_dir.join("GeneratedConfig.php");
    fs_err::create_dir_all(&generated_config_dir)?;
    // The package ships this file as a stub, hardlinked read-only from the
    // store like every other file `link_tree` places (`src/link.rs`); remove
    // it first so overwriting doesn't need write permission on the file
    // itself, only on the directory, and never touches the store's own copy.
    let _ = fs_err::remove_file(&path);
    fs_err::write(&path, body)?;
    Ok(())
}

fn to_map(tree: BTreeMap<String, Value>) -> Map<String, Value> {
    tree.into_iter().collect()
}

/// `$packageExtra['phpstan/extension-installer']['ignore']`: note the root
/// key itself contains a literal `/`, so this is a plain map lookup, not a
/// JSON pointer.
fn ignored_packages(root: &Root) -> Vec<String> {
    root.extra
        .get(PACKAGE_NAME)
        .and_then(|v| v.get("ignore"))
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn constraint_into_string(lower: &Bound, upper: &Bound) -> String {
    format!(
        "{}{}, {}{}",
        if lower.is_inclusive() { ">=" } else { ">" },
        lower.version(),
        if upper.is_inclusive() { "<=" } else { "<" },
        upper.version(),
    )
}

/// `Intervals::compactConstraint(new MultiConstraint($phpstanVersionConstraints))`:
/// the real plugin ANDs every extension's `phpstan/phpstan` requirement
/// together and compacts the result, rather than taking the overall lowest
/// lower bound and highest upper bound (which is what `MultiConstraint`'s own
/// `getLowerBound`/`getUpperBound` return, and diverges from the true
/// intersection once two extensions' ranges overlap only partially). An AND
/// of contiguous ranges stays contiguous, so `semver_php`'s `Intervals::get`
/// — the crate's own port of `compactConstraint`'s interval algebra — always
/// yields at most one interval here.
fn merge_constraints(constraints: Vec<Box<dyn Constraint>>) -> Option<String> {
    if constraints.is_empty() {
        return None;
    }
    let conjunction: Box<dyn Constraint> = if constraints.len() == 1 {
        constraints.into_iter().next().unwrap()
    } else {
        Box::new(MultiConstraint::new(constraints, true))
    };
    Intervals::new()
        .get(conjunction.as_ref())
        .numeric
        .first()
        .map(interval_into_string)
}

fn interval_into_string(interval: &Interval) -> String {
    let (lower, upper) = (interval.start(), interval.end());
    format!(
        "{}{}, {}{}",
        if lower.operator() == Operator::Ge {
            ">="
        } else {
            ">"
        },
        lower.version(),
        if upper.operator() == Operator::Le {
            "<="
        } else {
            "<"
        },
        upper.version(),
    )
}

/// `Plugin::$generatedFileTemplate`, filled in with `var_export`-shaped dumps
/// of `$data`/`$notInstalledPackages`/`$phpstanVersionConstraint`.
fn render(
    data: &Map<String, Value>,
    not_installed: &Map<String, Value>,
    constraint: Option<&str>,
) -> String {
    format!(
        "<?php declare(strict_types = 1);\n\nnamespace PHPStan\\ExtensionInstaller;\n\n\
         /**\n * This class is generated by phpstan/extension-installer.\n * @internal\n */\n\
         final class GeneratedConfig\n{{\n\n\
         \tpublic const EXTENSIONS = {};\n\n\
         \tpublic const NOT_INSTALLED = {};\n\n\
         \t/** @var string|null */\n\
         \tpublic const PHPSTAN_VERSION_CONSTRAINT = {};\n\n\
         \tprivate function __construct()\n\t{{\n\t}}\n\n}}\n",
        var_export(&Value::Object(data.clone()), 0),
        var_export(&Value::Object(not_installed.clone()), 0),
        constraint.map_or_else(|| "NULL".to_string(), php_string),
    )
}

/// PHP's `var_export`, for the JSON shapes this file ever needs: `null`,
/// bool, number, string, and both list (`Value::Array`) and associative
/// (`Value::Object`) arrays, indented two spaces per nesting level. Shared
/// with [`super::yii2`]/[`super::craft`], the other two `var_export`-shaped
/// generators.
pub(super) fn var_export(value: &Value, indent: usize) -> String {
    match value {
        Value::Null => "NULL".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => php_string(s),
        Value::Array(items) => {
            let entries: Vec<(String, &Value)> = items
                .iter()
                .enumerate()
                .map(|(i, v)| (i.to_string(), v))
                .collect();
            render_array(&entries, indent, false)
        }
        Value::Object(map) => {
            let entries: Vec<(String, &Value)> = map.iter().map(|(k, v)| (k.clone(), v)).collect();
            render_array(&entries, indent, true)
        }
    }
}

fn render_array(entries: &[(String, &Value)], indent: usize, quote_keys: bool) -> String {
    let mut out = String::from("array (\n");
    let child_indent = indent + 2;
    let pad = " ".repeat(child_indent);
    for (key, value) in entries {
        out.push_str(&pad);
        out.push_str(&if quote_keys {
            php_string(key)
        } else {
            key.clone()
        });
        out.push_str(" => ");
        if matches!(value, Value::Array(_) | Value::Object(_)) {
            out.push('\n');
            out.push_str(&pad);
        }
        out.push_str(&var_export(value, child_indent));
        out.push_str(",\n");
    }
    out.push_str(&" ".repeat(indent));
    out.push(')');
    out
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::lock::{read_lock, read_root};

    fn root(extra: &Value) -> Root {
        serde_json::from_value(json!({"extra": extra})).unwrap()
    }

    #[test]
    fn php_string_escapes_backslash_and_quote() {
        assert_eq!(php_string(r"Acme\Pkg"), r"'Acme\\Pkg'");
        assert_eq!(php_string("it's"), r"'it\'s'");
    }

    #[test]
    fn var_export_renders_null_and_scalars() {
        assert_eq!(var_export(&Value::Null, 0), "NULL");
        assert_eq!(var_export(&json!(true), 0), "true");
        assert_eq!(var_export(&json!(3), 0), "3");
        assert_eq!(var_export(&json!("hi"), 0), "'hi'");
    }

    #[test]
    fn var_export_matches_phpstans_own_nested_shape() {
        let value = json!({"includes": ["rules.neon"]});
        assert_eq!(
            var_export(&value, 2),
            "array (\n    'includes' => \n    array (\n      0 => 'rules.neon',\n    ),\n  )"
        );
    }

    #[test]
    fn var_export_empty_object_has_no_blank_line() {
        assert_eq!(var_export(&json!({}), 0), "array (\n)");
    }

    #[test]
    fn constraint_into_string_matches_composers_format() {
        let lower = Bound::new("1.12.4.0-dev", true);
        let upper = Bound::new("2.0.0.0-dev", false);
        assert_eq!(
            constraint_into_string(&lower, &upper),
            ">=1.12.4.0-dev, <2.0.0.0-dev"
        );
    }

    #[test]
    fn merge_constraints_intersects_rather_than_spans() {
        // `^1.10.3 || ^2.0` and `>=1.12.26 <2.0`: the envelope would be
        // `>=1.10.3.0-dev, <3.0.0.0-dev`, but the true intersection — what
        // the real plugin's `Intervals::compactConstraint` returns — is the
        // narrower `>=1.12.26.0-dev, <2.0.0.0-dev`.
        let a = VersionParser::parse_constraints("^1.10.3 || ^2.0").unwrap();
        let b = VersionParser::parse_constraints(">=1.12.26 <2.0").unwrap();
        let merged = merge_constraints(vec![a, b]).unwrap();
        assert_eq!(merged, ">=1.12.26.0-dev, <2.0.0.0-dev");
    }

    #[test]
    fn merge_constraints_is_none_when_empty() {
        assert_eq!(merge_constraints(Vec::new()), None);
    }

    #[test]
    fn apply_is_a_no_op_when_the_installer_itself_is_not_present() {
        let root = root(&json!({}));
        let packages: Vec<(&Package, PathBuf)> = Vec::new();
        apply(&root, &packages).unwrap();
    }

    /// #52's fixture, byte-diffed against real `phpstan/extension-installer`
    /// 1.4.3's own output (`tests/fixtures/plugins/phpstan/expected`):
    /// `install_path` embeds the project's own absolute path, so the fixture
    /// is a template with a `{{PROJECT_DIR}}` placeholder.
    #[test]
    fn phpstan_fixture_matches_composers_golden() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/phpstan");
        let root = read_root(&dir.join("composer.json")).unwrap();
        let lock = read_lock(&dir.join("composer.lock")).unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let packages: Vec<(&Package, PathBuf)> = lock
            .packages(true)
            .map(|p| (p, vendor_dir.join(&p.name)))
            .collect();

        apply(&root, &packages).unwrap();

        let got = fs_err::read_to_string(
            vendor_dir.join("phpstan/extension-installer/src/GeneratedConfig.php"),
        )
        .unwrap();
        let template = fs_err::read_to_string(dir.join("expected/GeneratedConfig.php")).unwrap();
        let want = template.replace("{{PROJECT_DIR}}", &project_dir.path().display().to_string());
        assert_eq!(got, want);
    }
}
