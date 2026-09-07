//! Writes a `composer.lock`, a port of `Package\Locker::setLockData` (top-level
//! key order, `fixupJsonDataType`) and `lockPackages` (`ArrayDumper::dump`
//! reused for each package entry, then `version_normalized`/`installation-source`
//! dropped and `time` moved to the end).
use std::collections::HashMap;

use anyhow::{Context, Result};
use serde_json::{Map, Value};

use crate::lock::content_hash;
use crate::time::{civil_from_days, days_from_civil};

use crate::solver::transaction::{AliasEntry, ResolvedPackage};

/// `Locker::PLUGIN_API_VERSION`/`PluginInterface::PLUGIN_API_VERSION`, this
/// Composer's bundled plugin API version: hardcoded rather than detected,
/// since it changes only with a Composer release, not per project. Guarded
/// by `plugin_api_version_matches_composer` so a Composer upgrade in
/// `devbox.json` that bumps it fails a test loudly instead of silently
/// drifting.
const PLUGIN_API_VERSION: &str = "2.9.0";

/// `ArrayDumper::dump`'s key order, minus `version_normalized` and
/// `installation-source` (`lockPackages` drops both) and `time` (moved to
/// the very end by `lockPackages`, appended separately below rather than
/// listed here).
const KEY_ORDER: &[&str] = &[
    "name",
    "version",
    "target-dir",
    "source",
    "dist",
    "require",
    "conflict",
    "provide",
    "replace",
    "require-dev",
    "suggest",
    "default-branch",
    "bin",
    "type",
    "extra",
    "autoload",
    "autoload-dev",
    "notification-url",
    "include-path",
    "php-ext",
    "archive",
    "scripts",
    "license",
    "authors",
    "description",
    "homepage",
    "keywords",
    "repositories",
    "support",
    "funding",
    "abandoned",
    "transport-options",
];

/// `ArrayDumper::dump`'s `ksort($data[$type])` per link type, plus `suggest`.
const KSORTED: &[&str] = &[
    "require",
    "conflict",
    "provide",
    "replace",
    "require-dev",
    "suggest",
];

const SOURCE_KEY_ORDER: &[&str] = &["type", "url", "reference", "mirrors"];
const DIST_KEY_ORDER: &[&str] = &["type", "url", "reference", "shasum", "mirrors"];

/// Everything `Locker::setLockData` needs beyond the two package lists
/// (`solver::solve_update`'s `UpdateResult`, minus `non_dev`/`dev` which are
/// passed separately so a caller doing only the no-dev half doesn't have to
/// hand back the rest of the struct just to move it).
pub struct LockOptions<'a> {
    pub minimum_stability: &'a str,
    pub stability_flags: &'a HashMap<String, u8>,
    pub prefer_stable: bool,
    pub prefer_lowest: bool,
    pub platform_reqs: &'a Map<String, Value>,
    pub platform_dev_reqs: &'a Map<String, Value>,
    pub platform_overrides: &'a Map<String, Value>,
    pub aliases: &'a [AliasEntry],
}

/// The full `composer.lock` body, `JsonFile::encode`'s default flags
/// (`JSON_PRETTY_PRINT | JSON_UNESCAPED_SLASHES | JSON_UNESCAPED_UNICODE`,
/// four-space indent) with the trailing newline `JsonFile::write` appends.
/// `serde_json`'s own pretty-printer never escapes `/` or non-ASCII, so
/// reaching for its defaults already matches those two flags; no
/// `php_json_encode`-style writer needed here (that one exists only for
/// `content_hash`'s *compact*, PHP-`json_encode`-exact encoding).
pub fn write(
    non_dev: &[ResolvedPackage],
    dev: Option<&[ResolvedPackage]>,
    options: &LockOptions,
    composer_json: &[u8],
) -> Result<String> {
    let mut lock = Map::new();
    lock.insert(
        "_readme".into(),
        Value::Array(
            [
                "This file locks the dependencies of your project to a known state",
                "Read more about it at https://getcomposer.org/doc/01-basic-usage.md#installing-dependencies",
                "This file is @generated automatically",
            ]
            .into_iter()
            .map(|s| Value::String(s.to_string()))
            .collect(),
        ),
    );
    lock.insert(
        "content-hash".into(),
        Value::String(content_hash(composer_json)?),
    );
    lock.insert("packages".into(), lock_packages(non_dev)?);
    lock.insert(
        "packages-dev".into(),
        match dev {
            Some(dev) => lock_packages(dev)?,
            None => Value::Null,
        },
    );
    lock.insert("aliases".into(), dump_aliases(options.aliases));
    lock.insert(
        "minimum-stability".into(),
        Value::String(options.minimum_stability.to_string()),
    );
    lock.insert(
        "stability-flags".into(),
        Value::Object(
            options
                .stability_flags
                .iter()
                .map(|(name, &rank)| (name.clone(), Value::from(rank)))
                .collect::<std::collections::BTreeMap<_, _>>()
                .into_iter()
                .collect(),
        ),
    );
    lock.insert("prefer-stable".into(), Value::Bool(options.prefer_stable));
    lock.insert("prefer-lowest".into(), Value::Bool(options.prefer_lowest));
    lock.insert(
        "platform".into(),
        Value::Object(options.platform_reqs.clone()),
    );
    lock.insert(
        "platform-dev".into(),
        Value::Object(options.platform_dev_reqs.clone()),
    );
    if !options.platform_overrides.is_empty() {
        lock.insert(
            "platform-overrides".into(),
            Value::Object(options.platform_overrides.clone()),
        );
    }
    lock.insert(
        "plugin-api-version".into(),
        Value::String(PLUGIN_API_VERSION.to_string()),
    );

    let mut buf = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(b"    ");
    let mut serializer = serde_json::Serializer::with_formatter(&mut buf, formatter);
    serde::Serialize::serialize(&Value::Object(lock), &mut serializer)?;
    buf.push(b'\n');
    Ok(String::from_utf8(buf)?)
}

/// `Locker::lockPackages`: dump each package in `ArrayDumper` order, drop
/// `version_normalized`/`installation-source`, move `time` to the end, sort
/// by name then version.
fn lock_packages(packages: &[ResolvedPackage]) -> Result<Value> {
    let mut dumped = packages
        .iter()
        .map(|p| dump_package(&p.raw))
        .collect::<Result<Vec<_>>>()?;
    dumped.sort_by(|a, b| {
        let name_cmp = a["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["name"].as_str().unwrap_or_default());
        if name_cmp != std::cmp::Ordering::Equal {
            return name_cmp;
        }
        a["version"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["version"].as_str().unwrap_or_default())
    });
    Ok(Value::Array(dumped))
}

fn dump_package(raw: &Value) -> Result<Value> {
    let raw = raw.as_object().cloned().unwrap_or_default();
    let mut out = Map::new();
    for &key in KEY_ORDER {
        let value = match key {
            "name" | "version" => raw
                .get(key)
                .cloned()
                .with_context(|| format!("provider entry missing {key:?}"))?,
            "source" => match raw.get("source") {
                Some(Value::Object(s)) => reorder(s, SOURCE_KEY_ORDER),
                _ => continue,
            },
            "dist" => match raw.get("dist") {
                Some(Value::Object(d)) => reorder(d, DIST_KEY_ORDER),
                _ => continue,
            },
            // `ArrayLoader::configureObject`'s `!empty($config[$key])` gate
            // (rather than the plain `isset()` most keys get): PHP's
            // `empty()` treats `""`/`"0"`/`0`/`false` the same as absent, so
            // a provider file's `"homepage": ""` (real Packagist data) never
            // reaches `CompletePackage::setHomepage`, and the getter
            // `ArrayDumper::dumpValues` reads back is never anything but
            // its unset default.
            "time" | "notification-url" | "description" | "homepage" | "keywords" | "license"
            | "authors" | "funding"
                if raw.get(key).is_some_and(is_empty_for_composer) =>
            {
                continue;
            }
            _ => match raw.get(key) {
                None | Some(Value::Null | Value::Bool(false)) => continue,
                Some(Value::Array(a)) if a.is_empty() => continue,
                Some(Value::Object(o)) if o.is_empty() => continue,
                Some(v) => v.clone(),
            },
        };
        let value = match (key, value) {
            (k, Value::Object(map)) if KSORTED.contains(&k) => {
                let mut entries: Vec<(String, Value)> = map.into_iter().collect();
                entries.sort_by(|a, b| a.0.cmp(&b.0));
                Value::Object(entries.into_iter().collect())
            }
            ("keywords", Value::Array(mut words)) => {
                words.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                Value::Array(words)
            }
            (_, v) => v,
        };
        if (key == "source" || key == "dist")
            && let Value::Object(o) = &value
            && o.is_empty()
        {
            continue;
        }
        out.insert(key.into(), value);
    }
    // `lockPackages`: `time` is unset then re-added, moving it to the end.
    if let Some(time) = raw
        .get("time")
        .filter(|v| !is_empty_for_composer(v))
        .and_then(Value::as_str)
        .and_then(normalize_time)
    {
        out.insert("time".into(), Value::String(time));
    }
    Ok(Value::Object(out))
}

/// `ArrayLoader::load`'s `time` handling (`ctype_digit($config['time']) ?
/// '@'.$config['time'] : $config['time']`, then `new \DateTime($time, new
/// \DateTimeZone('UTC'))`) and `ArrayDumper::dump`'s `$data['time'] =
/// $package->getReleaseDate()->format(DATE_RFC3339)` (`Y-m-d\TH:i:sP`):
/// PHP only falls back to the `UTC` zone argument when
/// the string carries none of its own, so a `Z` or `±HH:MM` suffix shifts
/// the clock and `P` always renders UTC as `+00:00`. Handles the shapes
/// real repositories emit: RFC 3339 with `Z` or an offset (optional
/// fractional seconds), the space- or `T`-separated `Y-m-d H:i:s` shape
/// with no zone (taken as UTC, like `ArrayLoader`), and a bare unix
/// timestamp. Returns `None` for anything else, matching `ArrayLoader`'s
/// `catch` leaving the release date, and so the dumped `time` key, unset.
#[allow(clippy::many_single_char_names)]
fn normalize_time(value: &str) -> Option<String> {
    if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) {
        return Some(format_utc(value.parse().ok()?));
    }
    let (date, rest) = value.split_once(['T', ' '])?;
    let mut parts = date.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    if rest.len() < 8 || rest.as_bytes()[2] != b':' || rest.as_bytes()[5] != b':' {
        return None;
    }
    let h: i64 = rest[0..2].parse().ok()?;
    let mi: i64 = rest[3..5].parse().ok()?;
    let s: i64 = rest[6..8].parse().ok()?;
    let mut tail = &rest[8..];
    if let Some(frac) = tail.strip_prefix('.') {
        let digits = frac.bytes().take_while(u8::is_ascii_digit).count();
        tail = &frac[digits..];
    }
    let offset_seconds: i64 = match tail {
        "" | "Z" => 0,
        _ => {
            let sign = match tail.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let offset = &tail[1..];
            if offset.len() != 5 || offset.as_bytes()[2] != b':' {
                return None;
            }
            let oh: i64 = offset[0..2].parse().ok()?;
            let om: i64 = offset[3..5].parse().ok()?;
            sign * (oh * 3600 + om * 60)
        }
    };
    let total = days_from_civil(y, m, d) * 86_400 + h * 3600 + mi * 60 + s - offset_seconds;
    Some(format_utc(total))
}

/// Renders epoch seconds as `DATE_RFC3339`'s `Y-m-d\TH:i:sP`, always
/// `+00:00` since the seconds passed in are already UTC.
fn format_utc(epoch_seconds: i64) -> String {
    let days = epoch_seconds.div_euclid(86_400);
    let secs_of_day = epoch_seconds.rem_euclid(86_400);
    let date = civil_from_days(days);
    let (h, m, s) = (
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60,
    );
    format!(
        "{:04}-{:02}-{:02}T{h:02}:{m:02}:{s:02}+00:00",
        date.y, date.m, date.d
    )
}

/// PHP's `empty($value)`: null, `false`, `0`, `"0"`, `""` and an empty
/// array/object are all "empty", unlike this module's usual null/empty-array
/// check.
fn is_empty_for_composer(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Bool(b) => !b,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(s) => s.is_empty() || s == "0",
        Value::Array(a) => a.is_empty(),
        Value::Object(o) => o.is_empty(),
    }
}

/// Picks `keys` from `map` in order, skipping any that are absent or null.
fn reorder(map: &Map<String, Value>, keys: &[&str]) -> Value {
    let mut out = Map::new();
    for &key in keys {
        match map.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Array(a)) if a.is_empty() => {}
            Some(v) => {
                out.insert(key.into(), v.clone());
            }
        }
    }
    Value::Object(out)
}

/// `LockTransaction::getAliases`'s result, then `Locker::setLockData`'s
/// legacy `dev-master`/`dev-trunk`/`dev-default` -> `9999999-dev` BC fixup
/// on the `version` field (Composer 1's lock shape; harmless to keep since
/// `VersionParser::normalize` never actually produces those three literals
/// for `dev-*` input today, but ported anyway for fidelity).
fn dump_aliases(aliases: &[AliasEntry]) -> Value {
    Value::Array(
        aliases
            .iter()
            .map(|alias| {
                let version = match alias.version.as_str() {
                    "dev-master" | "dev-trunk" | "dev-default" => "9999999-dev".to_string(),
                    _ => alias.version.clone(),
                };
                let mut entry = Map::new();
                entry.insert("package".into(), Value::String(alias.package.clone()));
                entry.insert("version".into(), Value::String(version));
                entry.insert("alias".into(), Value::String(alias.alias.clone()));
                entry.insert(
                    "alias_normalized".into(),
                    Value::String(alias.alias_normalized.clone()),
                );
                Value::Object(entry)
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[allow(clippy::print_stderr)]
    fn plugin_api_version_matches_composer() {
        // `devbox run -- composer show --platform` prints the bundled
        // `composer-plugin-api`'s version; if a Composer upgrade in
        // devbox.json bumps it, this constant must move too.
        let Ok(output) = std::process::Command::new("composer")
            .args(["show", "--platform", "--no-ansi"])
            .current_dir(std::env::temp_dir())
            .output()
        else {
            eprintln!("skipping plugin_api_version_matches_composer: composer is not on PATH");
            return;
        };
        if !output.status.success() {
            eprintln!("skipping plugin_api_version_matches_composer: composer show failed");
            return;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| l.trim_start().starts_with("composer-plugin-api"))
            .unwrap_or_else(|| {
                panic!("composer-plugin-api not in `composer show --platform`: {stdout}")
            });
        assert!(
            line.contains(PLUGIN_API_VERSION),
            "PLUGIN_API_VERSION {PLUGIN_API_VERSION:?} does not match the installed Composer's \
             bundled plugin API: {line:?}"
        );
    }

    #[test]
    fn normalize_time_rewrites_z_to_offset() {
        assert_eq!(
            normalize_time("2026-05-20T21:56:34Z").as_deref(),
            Some("2026-05-20T21:56:34+00:00")
        );
    }

    #[test]
    fn normalize_time_shifts_a_non_utc_offset_to_utc() {
        // 03:04:05+02:00 is 01:04:05Z: the hour must move, not just the
        // suffix, otherwise this would still read `03:04:05`.
        assert_eq!(
            normalize_time("2020-01-02T03:04:05+02:00").as_deref(),
            Some("2020-01-02T01:04:05+00:00")
        );
    }

    #[test]
    fn normalize_time_treats_a_space_separated_zoneless_stamp_as_utc() {
        assert_eq!(
            normalize_time("2020-01-02 03:04:05").as_deref(),
            Some("2020-01-02T03:04:05+00:00")
        );
    }

    #[test]
    fn normalize_time_parses_a_unix_timestamp() {
        assert_eq!(
            normalize_time("1700000000").as_deref(),
            Some("2023-11-14T22:13:20+00:00")
        );
    }

    #[test]
    fn normalize_time_is_none_for_unparseable_input() {
        assert_eq!(normalize_time("not a date"), None);
    }
}
