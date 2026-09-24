//! #305: a top-level `"type": "path"` repository — `Repository\PathRepository`,
//! discovering one package per matched directory rather than the single
//! inline `package` a `"package"`-type repository declares. Once
//! `discover` has built each directory's package JSON (the file's own
//! `composer.json`, plus `dist`/`transport-options`/`version` the way
//! `PathRepository::initialize` sets them), it's handed to
//! `repository::PackageSource::load` exactly like a `"package"` repository's
//! declared entries are — same solver path, same lock output, #13's
//! `dist.type: path` install already covers the rest.
//!
//! Left out: `Platform::expandPath`'s `~`/env-var expansion (`url` is used
//! as written), `GLOB_BRACE` (`{a,b}`) and `[...]` character classes (only
//! `*` is matched within a path segment, Windows separators, and
//! `options.reference`'s `"none"`/`"config"` values (only the `"auto"`
//! default — compute the sha1, override with the directory's own commit
//! when it has its own `.git` — is implemented).

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use sha1::{Digest, Sha1};

use crate::store::hex;
use crate::vcs;

/// `PathRepository::initialize`: glob `url` against `project_dir`, load each
/// matched directory's `composer.json`, and stamp `dist`/`transport-options`/
/// `version` onto it. Every returned [`Value`] is a full package object
/// ready for `repository::PackageSource::load`, the same shape a
/// `"package"` repository's declared `package` entries already are.
pub(crate) fn discover(
    project_dir: &Path,
    url: &str,
    options: &Map<String, Value>,
) -> Result<Vec<Value>> {
    let matches = glob_dirs(project_dir, url);
    if matches.is_empty() {
        if has_glob_chars(url) && plain_prefix_exists(project_dir, url) {
            // The parent directory before any wildcard exists but the glob
            // matched nothing under it — an empty repository, not an error
            // (`PathRepository::initialize`'s own `dirname()` walk-up).
            return Ok(Vec::new());
        }
        bail!("The `url` supplied for the path ({url}) repository does not exist");
    }

    // `$this->options['relative'] ??= !$filesystem->isAbsolutePath($this->url)`,
    // computed once per repository entry (not per matched directory) —
    // every match's `dist.reference` sha1 and `transport-options` are
    // derived from this same map.
    let mut effective_options = options.clone();
    effective_options
        .entry("relative".to_string())
        .or_insert(Value::Bool(!url.starts_with('/')));
    let transport_options: Map<String, Value> = effective_options
        .iter()
        .filter(|(key, _)| key.as_str() == "symlink" || key.as_str() == "relative")
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    let versions = effective_options
        .get("versions")
        .and_then(Value::as_object)
        .cloned();
    let serialized_options = php_serialize(&Value::Object(effective_options));

    let mut packages = Vec::new();
    for dir in matches {
        let composer_json_path = dir.join("composer.json");
        if !composer_json_path.is_file() {
            // `!file_exists($composerFilePath)` -> `continue`: a matched
            // directory with no `composer.json` is silently skipped, not an
            // error.
            continue;
        }
        let bytes = fs_err::read(&composer_json_path)
            .with_context(|| format!("reading {}", composer_json_path.display()))?;
        let mut package = serde_json::from_slice::<Value>(&bytes)
            .with_context(|| format!("{}: not valid JSON", composer_json_path.display()))?
            .as_object()
            .cloned()
            .with_context(|| format!("{}: must be a JSON object", composer_json_path.display()))?;

        let mut reference = hex(Sha1::digest(
            [bytes.as_slice(), serialized_options.as_bytes()].concat(),
        ));
        if dir.join(".git").is_dir()
            && let Some(commit) = git_head(&dir)
        {
            reference = commit;
        }
        package.insert(
            "dist".to_string(),
            Value::Object(Map::from_iter([
                ("type".to_string(), Value::String("path".to_string())),
                (
                    "url".to_string(),
                    Value::String(dist_url(project_dir, &dir, url)),
                ),
                ("reference".to_string(), Value::String(reference)),
            ])),
        );
        package.insert(
            "transport-options".to_string(),
            Value::Object(transport_options.clone()),
        );

        // `options.versions[name]` wins over the file's own `version`, even
        // if the file set one; a `version` already present (from either)
        // skips `VersionGuesser::guessVersion` entirely.
        let version_override = package
            .get("name")
            .and_then(Value::as_str)
            .and_then(|name| versions.as_ref().and_then(|v| v.get(name)))
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Some(version) = version_override {
            package.insert("version".to_string(), Value::String(version));
        } else if !package.contains_key("version") {
            let guessed = vcs::guess_version_in_git_tree(&dir).map(|v| v.pretty_version);
            package.insert(
                "version".to_string(),
                Value::String(guessed.unwrap_or_else(|| "dev-main".to_string())),
            );
        }

        packages.push(Value::Object(package));
    }
    Ok(packages)
}

/// `dist.url`: the matched directory, in the same relative/absolute style
/// `url` was written in (Composer's `glob()` returns matches in the same
/// notation as the pattern, substituting only the wildcarded segments).
fn dist_url(project_dir: &Path, dir: &Path, url: &str) -> String {
    if url.starts_with('/') {
        dir.to_string_lossy().into_owned()
    } else {
        dir.strip_prefix(project_dir)
            .unwrap_or(dir)
            .to_string_lossy()
            .into_owned()
    }
}

fn git_head(dir: &Path) -> Option<String> {
    let output = vcs::git_command()
        .args(["log", "-n1", "--pretty=%H"])
        .current_dir(dir)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let commit = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!commit.is_empty()).then_some(commit)
}

fn has_glob_chars(pattern: &str) -> bool {
    pattern.contains('*') || pattern.contains('{') || pattern.contains('}')
}

/// `PathRepository::initialize`'s empty-match fallback: strip trailing
/// `/`-segments (PHP's `dirname()`) while the remainder still has a glob
/// char, then check that plain directory exists.
fn plain_prefix_exists(project_dir: &Path, pattern: &str) -> bool {
    let mut current = pattern;
    while has_glob_chars(current) {
        current = match current.rfind('/') {
            Some(0) => "/",
            Some(idx) => &current[..idx],
            None => ".",
        };
    }
    resolve_plain(project_dir, current).is_dir()
}

fn resolve_plain(project_dir: &Path, path: &str) -> PathBuf {
    if path == "." {
        project_dir.to_path_buf()
    } else if let Some(absolute) = path.strip_prefix('/') {
        Path::new("/").join(absolute)
    } else {
        project_dir.join(path)
    }
}

/// `getUrlMatches`'s `glob($this->url, GLOB_MARK|GLOB_ONLYDIR|GLOB_BRACE)`:
/// `*` matches within one `/`-separated segment (never crosses it, same as
/// PHP's glob), literal segments (including `..`) just navigate, and only
/// directories survive the final filter. Sorted for determinism — the
/// final lock output re-sorts packages by name/version regardless, so glob
/// order itself is never observable.
fn glob_dirs(project_dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let (base, pattern) = match pattern.strip_prefix('/') {
        Some(rest) => (PathBuf::from("/"), rest),
        None => (project_dir.to_path_buf(), pattern),
    };
    let mut current = vec![base];
    for segment in pattern.split('/').filter(|s| !s.is_empty()) {
        if !segment.contains('*') {
            for dir in &mut current {
                *dir = dir.join(segment);
            }
            continue;
        }
        let mut next = Vec::new();
        for dir in &current {
            let Ok(entries) = fs_err::read_dir(dir) else {
                continue;
            };
            let mut names: Vec<String> = entries
                .filter_map(Result::ok)
                .filter_map(|entry| entry.file_name().into_string().ok())
                .filter(|name| segment_matches(segment, name))
                .collect();
            names.sort();
            next.extend(names.into_iter().map(|name| dir.join(name)));
        }
        current = next;
    }
    current.retain(|dir| dir.is_dir());
    current.sort();
    current
}

/// `*` within one path segment, case-sensitive (a directory name, unlike
/// `lock::glob_match`'s package/plugin names, is compared exactly as the
/// filesystem has it). Same first/middle/last split as `lock::glob_match`,
/// minus its lowercasing.
fn segment_matches(pattern: &str, name: &str) -> bool {
    let mut segments = pattern.split('*');
    let first = segments.next().unwrap_or("");
    let Some(mut rest) = name.strip_prefix(first) else {
        return false;
    };
    let mut segments: Vec<&str> = segments.collect();
    let Some(last) = segments.pop() else {
        return rest.is_empty();
    };
    for middle in segments {
        let Some(at) = rest.find(middle) else {
            return false;
        };
        rest = &rest[at + middle.len()..];
    }
    rest.ends_with(last)
}

/// PHP's `serialize()`, restricted to the shapes a JSON `options` map can
/// hold (bool/int/float/string/null/array): `dist.reference`'s sha1 is
/// `sha1($json . serialize($this->options))`, so this has to match PHP's
/// byte output exactly, string lengths included (byte length, not char
/// count — same as PHP's `strlen`).
fn php_serialize(value: &Value) -> String {
    use std::fmt::Write as _;

    match value {
        Value::Null => "N;".to_string(),
        Value::Bool(b) => format!("b:{};", u8::from(*b)),
        Value::Number(n) => n.as_i64().map_or_else(
            || format!("d:{};", n.as_f64().unwrap_or_default()),
            |i| format!("i:{i};"),
        ),
        Value::String(s) => format!("s:{}:\"{s}\";", s.len()),
        Value::Array(items) => {
            let mut out = format!("a:{}:{{", items.len());
            for (index, item) in items.iter().enumerate() {
                let _ = write!(out, "i:{index};");
                out.push_str(&php_serialize(item));
            }
            out.push('}');
            out
        }
        Value::Object(map) => {
            let mut out = format!("a:{}:{{", map.len());
            for (key, item) in map {
                let _ = write!(out, "s:{}:\"{key}\";", key.len());
                out.push_str(&php_serialize(item));
            }
            out.push('}');
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn php_serialize_matches_composer_for_a_relative_only_options_map() {
        let options = serde_json::json!({"relative": true});
        assert_eq!(php_serialize(&options), r#"a:1:{s:8:"relative";b:1;}"#);
    }

    #[test]
    fn php_serialize_preserves_declared_key_order_then_the_appended_default() {
        let mut options = Map::new();
        options.insert("symlink".to_string(), Value::Bool(false));
        options.insert("relative".to_string(), Value::Bool(true));
        assert_eq!(
            php_serialize(&Value::Object(options)),
            r#"a:2:{s:7:"symlink";b:0;s:8:"relative";b:1;}"#
        );
    }

    #[test]
    fn segment_matches_is_case_sensitive_unlike_lock_glob_match() {
        assert!(segment_matches("*", "Foo"));
        assert!(segment_matches("fo*", "foo"));
        assert!(!segment_matches("fo*", "Foo"));
    }

    #[test]
    fn glob_dirs_finds_directories_one_level_under_a_wildcard() {
        let root = tempfile::tempdir().unwrap();
        fs_err::create_dir_all(root.path().join("packages/foo")).unwrap();
        fs_err::create_dir_all(root.path().join("packages/bar")).unwrap();
        fs_err::write(root.path().join("packages/not-a-dir"), "").unwrap();
        let mut found = glob_dirs(root.path(), "packages/*");
        found.sort();
        assert_eq!(
            found,
            vec![
                root.path().join("packages/bar"),
                root.path().join("packages/foo"),
            ]
        );
    }

    #[test]
    fn discover_errors_with_composers_own_wording_for_a_missing_plain_url() {
        let root = tempfile::tempdir().unwrap();
        let err = discover(root.path(), "missing", &Map::new())
            .unwrap_err()
            .to_string();
        assert_eq!(
            err,
            "The `url` supplied for the path (missing) repository does not exist"
        );
    }

    #[test]
    fn discover_is_silent_when_a_glob_matches_nothing_but_its_parent_exists() {
        let root = tempfile::tempdir().unwrap();
        fs_err::create_dir_all(root.path().join("packages")).unwrap();
        let packages = discover(root.path(), "packages/*", &Map::new()).unwrap();
        assert!(packages.is_empty());
    }
}
