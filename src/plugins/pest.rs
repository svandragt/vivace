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

use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::Value;

use super::{Adapter, Ctx};
use crate::lock::{Package, Root, php_json_encode_pretty};

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
///
/// `getCanonicalPackages()` hands the plugin the *local repository's* order,
/// which is Composer's install order — `installed.json` is a by-name-sorted
/// view written from it, not the order itself. Reading the lock in file order
/// put `pestphp/pest`'s nineteen entries ahead of the three sibling plugin
/// packages on a `roots/bedrock` install, where Composer emits them last.
///
/// Written via [`php_json_encode_pretty`] (PHP's `JSON_PRETTY_PRINT`) with no
/// trailing newline: Composer writes this file with a plain
/// `file_put_contents`, unlike `composer.lock`/`installed.json`'s own
/// writers, which append one.
fn apply(root: &Root, vendor_dir: &Path, packages: &[(&Package, PathBuf)]) -> Result<()> {
    // The plugin only runs when it is itself installed, so a `--no-dev`
    // install of a project that only needs pest for tests writes no file at
    // all — Composer's `vendor/` has none to compare against.
    if !packages.iter().any(|(p, _)| p.name == PACKAGE_NAME) {
        return Ok(());
    }

    let mut plugins: Vec<Value> = Vec::new();
    for (package, _) in super::in_install_order(packages) {
        plugins.extend(pest_plugins(package.raw.pointer("/extra/pest/plugins")));
    }
    plugins.extend(pest_plugins(root.extra.pointer("/pest/plugins")));

    fs_err::write(
        vendor_dir.join("pest-plugins.json"),
        php_json_encode_pretty(&Value::Array(plugins)),
    )?;
    Ok(())
}

/// `$extra['pest']['plugins'] ?? []`.
fn pest_plugins(value: Option<&Value>) -> Vec<Value> {
    value.and_then(Value::as_array).cloned().unwrap_or_default()
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
    /// A `--no-dev` install of a project that only needs pest for tests
    /// leaves the plugin uninstalled, so it never runs and Composer's
    /// `vendor/` holds no `pest-plugins.json` to compare against. Writing an
    /// empty one made `roots/bedrock` differ in the sweep.
    fn apply_writes_nothing_when_the_plugin_itself_is_not_installed() {
        let root = root(&json!({"pest": {"plugins": ["Acme\\Root\\RootPlugin"]}}));
        let vendor_dir = tempfile::tempdir().unwrap();
        let packages: Vec<(&Package, PathBuf)> = Vec::new();
        apply(&root, vendor_dir.path(), &packages).unwrap();
        assert!(!vendor_dir.path().join("pest-plugins.json").exists());
    }

    #[test]
    fn php_json_encode_pretty_escapes_forward_slash() {
        let got = php_json_encode_pretty(&json!("Acme/Slash/Test"));
        assert_eq!(got, r#""Acme\/Slash\/Test""#);
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
