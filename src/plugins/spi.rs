//! `tbachert/spi`'s `preAutoloadDump` listener: writes
//! `vendor/composer/GeneratedServiceProviderData.php`, a `service => providers`
//! lookup table built from every package's `extra.spi`.
//!
//! Ported from `Nevay/spi` `v1.0.5`'s `src/Composer/Plugin.php`, fetched
//! 2026-09-06 — only the `extra.spi` source, per `docs/plugin-strategy.md`.
//! The plugin's other source, `extra.spi-config.autoload-files` (executing a
//! package's own autoload `files` entries at dump time to call
//! `ServiceLoader::collectProviders`), needs a live PHP runtime to run
//! arbitrary package code and is out of scope.
//!
//! ponytail: a provider/service is included whenever its name matches a
//! class-like pattern; the real plugin also checks `class_exists`/
//! `ReflectionClass` availability (dropping a provider whose class isn't
//! actually autoloadable, or whose `ServiceProviderRequirement` attributes
//! aren't satisfied) before emitting it. vivace has no PHP runtime to run
//! that check, so a service/provider declared in `extra.spi` but never
//! actually defined would diverge from Composer's output here.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde_json::Value;

use crate::lock::{Package, Root};

static FQCN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\\?[a-zA-Z_\x80-\xff][a-zA-Z0-9_\x80-\xff]*(?:\\[a-zA-Z_\x80-\xff][a-zA-Z0-9_\x80-\xff]*)*$")
        .expect("valid regex")
});

pub(super) fn apply(
    root: &Root,
    vendor_dir: &Path,
    packages: &[(&Package, PathBuf)],
) -> Result<()> {
    // `$mappings[$service] ??= []` preserves first-seen order: the root
    // package's own `extra.spi` first, then locked packages in lock order.
    let mut mappings: Vec<(String, Vec<(String, String)>)> = Vec::new();
    let root_name = root.name.clone().unwrap_or_else(|| "__root__".into());
    let root_version = root.version.clone().unwrap_or_else(|| "dev-master".into());
    from_extra_spi(
        &root.extra,
        &format!("{root_name} {root_version}"),
        &mut mappings,
    );
    for (package, _) in packages {
        let extra = package
            .raw
            .pointer("/extra")
            .cloned()
            .unwrap_or(Value::Null);
        from_extra_spi(
            &extra,
            &format!("{} {}", package.name, package.version),
            &mut mappings,
        );
    }

    let mut body = String::new();
    for (service, providers) in &mappings {
        if !FQCN.is_match(service) {
            continue;
        }
        let service = qualify(service);
        let _ = write!(body, "\n            {service}::class => [");
        for (provider, source) in providers {
            if !FQCN.is_match(provider) {
                continue;
            }
            let provider = qualify(provider);
            let _ = write!(
                body,
                "\n                {provider}::class, // {source} (extra.spi)"
            );
        }
        body.push_str("\n            ],");
    }

    let code = format!(
        "<?php declare(strict_types=1);\nnamespace Nevay\\SPI;\n\n\
         /**\n * @internal \n */\n\
         final class GeneratedServiceProviderData {{\n\n\
         \x20\x20\x20\x20public const VERSION = 1;\n\n\
         \x20\x20\x20\x20/**\n\
         \x20\x20\x20\x20 * @param class-string $service\n\
         \x20\x20\x20\x20 * @return list<class-string>\n\
         \x20\x20\x20\x20 */\n\
         \x20\x20\x20\x20public static function providers(string $service): array {{\n\
         \x20\x20\x20\x20\x20\x20\x20\x20return match ($service) {{\n\
         \x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20\x20default => [],{body}\n\
         \x20\x20\x20\x20\x20\x20\x20\x20}};\n\x20\x20\x20\x20}}\n}}"
    );

    let path = vendor_dir.join("composer/GeneratedServiceProviderData.php");
    fs_err::create_dir_all(vendor_dir.join("composer"))?;
    fs_err::write(&path, code)?;
    Ok(())
}

/// `serviceProvidersFromExtraSpi`: `extra.spi` is `{service: providers}`,
/// `providers` a single string or a list; a repeat mapping for the same
/// provider keeps its first source (`+=` on the mappings array).
fn from_extra_spi(
    extra: &Value,
    source: &str,
    mappings: &mut Vec<(String, Vec<(String, String)>)>,
) {
    let Some(spi) = extra.get("spi").and_then(Value::as_object) else {
        return;
    };
    for (service, value) in spi {
        let providers: Vec<String> = match value {
            Value::String(s) => vec![s.clone()],
            Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect(),
            _ => continue,
        };
        let index = if let Some(i) = mappings.iter().position(|(s, _)| s == service) {
            i
        } else {
            mappings.push((service.clone(), Vec::new()));
            mappings.len() - 1
        };
        let list = &mut mappings[index].1;
        for provider in providers {
            if !list.iter().any(|(p, _)| *p == provider) {
                list.push((provider, source.to_string()));
            }
        }
    }
}

/// `if ($x[0] !== '\\') { $x = '\\' . $x; }`.
fn qualify(name: &str) -> String {
    if name.starts_with('\\') {
        name.to_string()
    } else {
        format!("\\{name}")
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::lock::{read_lock, read_root};

    fn root(extra: &Value) -> Root {
        serde_json::from_value(json!({"extra": extra})).unwrap()
    }

    #[test]
    fn qualify_prepends_a_leading_backslash_once() {
        assert_eq!(qualify("Acme\\Thing"), "\\Acme\\Thing");
        assert_eq!(qualify("\\Acme\\Thing"), "\\Acme\\Thing");
    }

    #[test]
    fn fqcn_accepts_a_namespaced_class_and_rejects_junk() {
        assert!(FQCN.is_match("Acme\\Spi\\ServiceInterface"));
        assert!(FQCN.is_match("Plain"));
        assert!(!FQCN.is_match("not a class"));
        assert!(!FQCN.is_match(""));
    }

    #[test]
    fn from_extra_spi_reads_a_single_string_provider() {
        let extra = json!({"spi": {"Acme\\Service": "Acme\\Provider"}});
        let mut mappings = Vec::new();
        from_extra_spi(&extra, "acme/pkg 1.0.0", &mut mappings);
        assert_eq!(
            mappings,
            vec![(
                "Acme\\Service".to_string(),
                vec![("Acme\\Provider".to_string(), "acme/pkg 1.0.0".to_string())]
            )]
        );
    }

    #[test]
    fn from_extra_spi_dedupes_a_repeated_provider_keeping_the_first_source() {
        let extra = json!({"spi": {"Acme\\Service": ["Acme\\Provider"]}});
        let mut mappings = Vec::new();
        from_extra_spi(&extra, "first 1.0.0", &mut mappings);
        from_extra_spi(&extra, "second 1.0.0", &mut mappings);
        assert_eq!(mappings.len(), 1);
        assert_eq!(
            mappings[0].1,
            vec![("Acme\\Provider".to_string(), "first 1.0.0".to_string())]
        );
    }

    #[test]
    fn from_extra_spi_is_a_no_op_without_a_spi_key() {
        let mut mappings = Vec::new();
        from_extra_spi(&json!({}), "acme/pkg 1.0.0", &mut mappings);
        assert!(mappings.is_empty());
    }

    #[test]
    fn apply_writes_an_empty_match_when_nothing_declares_spi() {
        let root = root(&json!({}));
        let vendor_dir = tempfile::tempdir().unwrap();
        apply(&root, vendor_dir.path(), &[]).unwrap();
        let got = fs_err::read_to_string(
            vendor_dir
                .path()
                .join("composer/GeneratedServiceProviderData.php"),
        )
        .unwrap();
        assert!(got.contains("default => [],\n        };"));
    }

    /// #52's fixture, byte-diffed against real `tbachert/spi` v1.0.5's own
    /// output (`tests/fixtures/plugins/spi/expected`); no absolute path here,
    /// so unlike the `phpstan` golden this needs no placeholder substitution.
    #[test]
    fn spi_fixture_matches_composers_golden() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/spi");
        let root = read_root(&dir.join("composer.json")).unwrap();
        let lock = read_lock(&dir.join("composer.lock")).unwrap();
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let packages: Vec<(&Package, PathBuf)> = lock
            .packages(true)
            .map(|p| (p, vendor_dir.join(&p.name)))
            .collect();

        apply(&root, &vendor_dir, &packages).unwrap();

        let got =
            fs_err::read_to_string(vendor_dir.join("composer/GeneratedServiceProviderData.php"))
                .unwrap();
        let want =
            fs_err::read_to_string(dir.join("expected/GeneratedServiceProviderData.php")).unwrap();
        assert_eq!(got, want);
    }
}
