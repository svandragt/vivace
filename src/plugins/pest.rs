//! `pestphp/pest-plugin`'s `Manager::registerPlugins` (`post-autoload-dump`):
//! runs the plugin's own `pest:dump-plugins` command, whose `execute` writes
//! `vendor/pest-plugins.json` — `json_encode($plugins, JSON_PRETTY_PRINT)` of
//! every installed package's `extra.pest.plugins` array, `array_merge`d in
//! `getCanonicalPackages()` order (keyed on that `extra` key, not on package
//! `type`; no de-duplication), with the root package's own
//! `extra.pest.plugins` appended last.
//!
//! Ported from `pestphp/pest-plugin` `v5.0.0`'s `src/Manager.php`/
//! `src/Commands/DumpCommand.php`, fetched 2026-09-14.
//!
//! ponytail: runs from [`Adapter::pre_autoload_dump`], not
//! `post_autoload_dump`, even though the real plugin's only listener is
//! `post-autoload-dump`. vivace's [`Ctx`] for that phase carries no package
//! list, while `pre_autoload_dump` already hands every installed package
//! `install::write_autoload` is about to dump anyway; `pest-plugins.json`'s
//! content never depends on the generated autoloader, so dispatching one
//! phase earlier is unobservable. Move to `post_autoload_dump` if `Ctx` ever
//! grows a package list of its own for some other adapter's sake.
//!
//! `getCapabilities()`'s `composer pest:dump-plugins` command provider isn't
//! ported: viv never runs arbitrary Composer commands.

use std::path::PathBuf;

use anyhow::Result;
use serde_json::Value;

use super::{Adapter, Ctx};
use crate::lock::{Package, Root};

const PACKAGE_NAME: &str = "pestphp/pest-plugin";

pub(super) struct Pest;

impl Adapter for Pest {
    fn plugin_names(&self) -> &'static [&'static str] {
        &[PACKAGE_NAME]
    }

    fn upstream_version(&self) -> &'static str {
        "v5.0.0"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/plugins/pest"
    }

    fn pre_autoload_dump(&self, ctx: &Ctx<'_>, packages: &[(&Package, PathBuf)]) -> Result<()> {
        apply(ctx.root, ctx.vendor_dir, packages)
    }
}

/// `DumpCommand::execute`: a plain `array_merge` (list concatenation, no
/// de-duplication) of every package's `extra.pest.plugins`, root last.
pub(super) fn apply(
    root: &Root,
    vendor_dir: &std::path::Path,
    packages: &[(&Package, PathBuf)],
) -> Result<()> {
    let mut plugins: Vec<Value> = Vec::new();
    for (package, _) in packages {
        plugins.extend(pest_plugins(package.raw.pointer("/extra/pest/plugins")));
    }
    plugins.extend(pest_plugins(root.extra.pointer("/pest/plugins")));

    fs_err::write(
        vendor_dir.join("pest-plugins.json"),
        pretty_json(&Value::Array(plugins)),
    )?;
    Ok(())
}

/// `$extra['pest']['plugins'] ?? []`.
fn pest_plugins(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
}

/// PHP's `json_encode($value, JSON_PRETTY_PRINT)`: four-space indent, `/`
/// escaped as `\/`, non-ASCII escaped as `\uXXXX` (UTF-16 code units,
/// surrogate pairs above `U+FFFF`) — the opposite of `serde_json`'s own
/// pretty-printer, which never escapes either. No trailing newline: Composer
/// writes this file with a plain `file_put_contents`, unlike `composer.lock`/
/// `installed.json`'s own writers.
fn pretty_json(value: &Value) -> String {
    let mut out = String::new();
    write_pretty(value, 0, &mut out);
    out
}

fn write_pretty(value: &Value, indent: usize, out: &mut String) {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Value::Number(n) => out.push_str(&n.to_string()),
        Value::String(s) => write_json_string(s, out),
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Array(items) => {
            let child_indent = indent + 4;
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&" ".repeat(child_indent));
                write_pretty(item, child_indent, out);
                if i + 1 < items.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&" ".repeat(indent));
            out.push(']');
        }
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Object(map) => {
            let child_indent = indent + 4;
            out.push_str("{\n");
            for (i, (key, item)) in map.iter().enumerate() {
                out.push_str(&" ".repeat(child_indent));
                write_json_string(key, out);
                out.push_str(": ");
                write_pretty(item, child_indent, out);
                if i + 1 < map.len() {
                    out.push(',');
                }
                out.push('\n');
            }
            out.push_str(&" ".repeat(indent));
            out.push('}');
        }
    }
}

fn write_json_string(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '/' => out.push_str("\\/"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                use std::fmt::Write as _;
                write!(out, "\\u{:04x}", c as u32).expect("write! to String never fails");
            }
            c if c.is_ascii() => out.push(c),
            c => {
                let mut units = [0u16; 2];
                for unit in c.encode_utf16(&mut units) {
                    use std::fmt::Write as _;
                    write!(out, "\\u{unit:04x}").expect("write! to String never fails");
                }
            }
        }
    }
    out.push('"');
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
    fn apply_is_a_no_op_list_when_nothing_declares_pest_plugins() {
        let root = root(&json!({}));
        let vendor_dir = tempfile::tempdir().unwrap();
        let packages: Vec<(&Package, PathBuf)> = Vec::new();
        apply(&root, vendor_dir.path(), &packages).unwrap();
        let got = fs_err::read_to_string(vendor_dir.path().join("pest-plugins.json")).unwrap();
        assert_eq!(got, "[]");
    }

    #[test]
    fn write_json_string_escapes_forward_slash() {
        let mut out = String::new();
        write_json_string("Acme/Slash/Test", &mut out);
        assert_eq!(out, r#""Acme\/Slash\/Test""#);
    }

    /// #131's fixture, byte-diffed against real `pestphp/pest-plugin`
    /// v5.0.0's own output (`tests/fixtures/plugins/pest/expected`): a
    /// dependency's `extra.pest.plugins` comes first, in lock order, then the
    /// root package's own last.
    #[test]
    fn pest_fixture_matches_composers_golden() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/pest");
        let root = read_root(&dir.join("composer.json")).unwrap();
        let lock = read_lock(&dir.join("composer.lock")).unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        fs_err::create_dir_all(&vendor_dir).unwrap();
        let packages: Vec<(&Package, PathBuf)> = lock
            .packages(true)
            .map(|p| (p, vendor_dir.join(&p.name)))
            .collect();

        apply(&root, &vendor_dir, &packages).unwrap();

        let got = fs_err::read_to_string(vendor_dir.join("pest-plugins.json")).unwrap();
        let want = fs_err::read_to_string(dir.join("expected/pest-plugins.json")).unwrap();
        assert_eq!(got, want);
    }
}
