//! `php-http/discovery`'s `preAutoloadDump` listener: writes
//! `vendor/composer/GeneratedDiscoveryStrategy.php`, a `switch` mapping each
//! pinned interface to the implementation class chosen in the root
//! `composer.json`'s `extra.discovery` map.
//!
//! Ported from `php-http/discovery` `1.20.0`'s `src/Composer/Plugin.php`,
//! fetched 2026-09-06 — `preAutoloadDump` only, per `docs/plugin-strategy.md`.
//! The plugin's other listener, `postUpdate` (adding an `*-implementation`
//! virtual package's chosen provider to `composer.json`/`composer.lock` when
//! it's missing, by re-entering Composer's own installer), needs `viv update`
//! to treat the discovery as an additional requirement, not just a file to
//! write, and is out of scope.
//!
//! ponytail: the real plugin also appends the generated file to the root
//! package's autoload `classmap` before the dump, so Composer's own
//! autoloader can find `GeneratedDiscoveryStrategy` without a `require`.
//! Wiring that into vivace's autoload generation is a `src/install.rs`
//! change, not a `src/plugins` one — out of scope here; the file is written,
//! but nothing yet points the generated autoloader at it.

use std::path::Path;

use anyhow::{Result, bail};
use serde_json::Value;

use crate::lock::Root;

/// `Plugin::INTERFACE_MAP`: for every supported virtual `*-implementation`
/// package, the interfaces `ClassDiscovery` looks up an implementation for.
const INTERFACE_MAP: &[(&str, &[&str])] = &[
    (
        "php-http/async-client-implementation",
        &["Http\\Client\\HttpAsyncClient"],
    ),
    (
        "php-http/client-implementation",
        &["Http\\Client\\HttpClient"],
    ),
    (
        "psr/http-client-implementation",
        &["Psr\\Http\\Client\\ClientInterface"],
    ),
    (
        "psr/http-factory-implementation",
        &[
            "Psr\\Http\\Message\\RequestFactoryInterface",
            "Psr\\Http\\Message\\ResponseFactoryInterface",
            "Psr\\Http\\Message\\ServerRequestFactoryInterface",
            "Psr\\Http\\Message\\StreamFactoryInterface",
            "Psr\\Http\\Message\\UploadedFileFactoryInterface",
            "Psr\\Http\\Message\\UriFactoryInterface",
        ],
    ),
];

pub(super) fn apply(root: &Root, vendor_dir: &Path) -> Result<()> {
    let path = vendor_dir.join("composer/GeneratedDiscoveryStrategy.php");
    let pinned = root.extra.get("discovery").and_then(Value::as_object);

    let mut entries = Vec::new();
    if let Some(pinned) = pinned {
        for (abstraction, class) in pinned {
            let Some(class) = class.as_str() else {
                bail!("extra.discovery.{abstraction}: expected a class name string");
            };
            for interface in interfaces_for(abstraction)? {
                entries.push(format!(
                    "case {}: return [['class' => {}]];\n",
                    php_string(&interface),
                    php_string(class)
                ));
            }
        }
    }

    // No pin resolves to a candidate: Composer deletes a stale file from a
    // previous run rather than leave an empty switch behind.
    if entries.is_empty() {
        let _ = fs_err::remove_file(&path);
        return Ok(());
    }

    let candidates = entries.join("            ");
    let code = format!(
        "<?php\n\nnamespace Http\\Discovery\\Strategy;\n\n\
         class GeneratedDiscoveryStrategy implements DiscoveryStrategy\n{{\n\
         \x20\x20\x20\x20public static function getCandidates($type)\n\
         \x20\x20\x20\x20{{\n\
         \x20\x20\x20\x20\x20\x20\x20\x20switch ($type) {{\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20{candidates}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20default: return [];\n\
         \x20\x20\x20\x20\x20\x20\x20\x20}}\n\x20\x20\x20\x20}}\n}}\n"
    );

    fs_err::create_dir_all(vendor_dir.join("composer"))?;
    fs_err::write(&path, code)?;
    Ok(())
}

/// `self::INTERFACE_MAP[$abstraction]`, or — when `$abstraction` is itself
/// one of the interfaces in that map rather than a virtual package name — a
/// single-interface list of just that name; anything else is the plugin's
/// own `UnexpectedValueException`.
fn interfaces_for(abstraction: &str) -> Result<Vec<String>> {
    if let Some((_, interfaces)) = INTERFACE_MAP.iter().find(|(name, _)| *name == abstraction) {
        return Ok(interfaces.iter().map(|s| (*s).to_string()).collect());
    }
    if INTERFACE_MAP
        .iter()
        .any(|(_, interfaces)| interfaces.contains(&abstraction))
    {
        return Ok(vec![abstraction.to_string()]);
    }
    let known: Vec<&str> = INTERFACE_MAP.iter().map(|(name, _)| *name).collect();
    bail!(
        "Invalid \"extra.discovery\" pinned in composer.json: \"{abstraction}\" is not one of [\"{}\"].",
        known.join("\", \"")
    )
}

/// A PHP single-quoted string literal: only `\` and `'` need escaping.
fn php_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::*;
    use crate::lock::read_root;

    fn root(extra: &Value) -> Root {
        serde_json::from_value(json!({"extra": extra})).unwrap()
    }

    #[test]
    fn apply_is_a_no_op_without_any_pin() {
        let root = root(&json!({}));
        let vendor_dir = tempfile::tempdir().unwrap();
        apply(&root, vendor_dir.path()).unwrap();
        assert!(
            !vendor_dir
                .path()
                .join("composer/GeneratedDiscoveryStrategy.php")
                .exists()
        );
    }

    #[test]
    fn apply_removes_a_stale_file_once_the_pin_is_gone() {
        let root = root(&json!({}));
        let vendor_dir = tempfile::tempdir().unwrap();
        let composer_dir = vendor_dir.path().join("composer");
        fs_err::create_dir_all(&composer_dir).unwrap();
        let path = composer_dir.join("GeneratedDiscoveryStrategy.php");
        fs_err::write(&path, "stale").unwrap();
        apply(&root, vendor_dir.path()).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn interfaces_for_expands_a_known_abstraction() {
        let interfaces = interfaces_for("psr/http-client-implementation").unwrap();
        assert_eq!(interfaces, vec!["Psr\\Http\\Client\\ClientInterface"]);
    }

    #[test]
    fn interfaces_for_accepts_a_bare_interface_pin() {
        let interfaces = interfaces_for("Http\\Client\\HttpClient").unwrap();
        assert_eq!(interfaces, vec!["Http\\Client\\HttpClient"]);
    }

    #[test]
    fn interfaces_for_rejects_an_unknown_name() {
        let err = interfaces_for("acme/not-a-thing").unwrap_err();
        assert!(err.to_string().contains("acme/not-a-thing"), "{err}");
    }

    #[test]
    fn apply_writes_a_case_per_pinned_interface() {
        let root = root(&json!({
            "discovery": {"psr/http-client-implementation": "Acme\\Client"}
        }));
        let vendor_dir = tempfile::tempdir().unwrap();
        apply(&root, vendor_dir.path()).unwrap();
        let got = fs_err::read_to_string(
            vendor_dir
                .path()
                .join("composer/GeneratedDiscoveryStrategy.php"),
        )
        .unwrap();
        assert!(got.contains(
            "case 'Psr\\\\Http\\\\Client\\\\ClientInterface': return [['class' => 'Acme\\\\Client']];"
        ));
    }

    /// #101's fixture, byte-diffed against real `php-http/discovery` 1.20.0's
    /// own output (`tests/fixtures/plugins/discovery/expected`).
    #[test]
    fn discovery_fixture_matches_composers_golden() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/discovery");
        let root = read_root(&dir.join("composer.json")).unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");

        apply(&root, &vendor_dir).unwrap();

        let got =
            fs_err::read_to_string(vendor_dir.join("composer/GeneratedDiscoveryStrategy.php"))
                .unwrap();
        let want =
            fs_err::read_to_string(dir.join("expected/GeneratedDiscoveryStrategy.php")).unwrap();
        assert_eq!(got, want);
    }
}
