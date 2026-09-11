//! `dealerdirect/phpcodesniffer-composer-installer`'s
//! `onDependenciesChangedEvent`: finds every installed `phpcodesniffer-standard`
//! package's `ruleset.xml` files and points `squizlabs/php_codesniffer`'s own
//! `installed_paths` config at their parent directories, via
//! `phpcs --config-set` (writes `vendor/squizlabs/php_codesniffer/CodeSniffer.conf`).
//!
//! Ported from `PHPCSStandards/composer-installer` (Packagist still lists the
//! package as `dealerdirect/phpcodesniffer-composer-installer`) `v1.2.1`'s
//! `src/Plugin.php`, fetched 2026-09-06.
//!
//! ponytail: always recomputes `installed_paths` from the current locked
//! package set rather than merging into whatever `CodeSniffer.conf` already
//! has (the real plugin's `loadInstalledPaths`/`cleanInstalledPaths`); a path
//! a developer added to that file by hand, outside Composer, would be
//! overwritten. Add a merge step if that ever bites.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

use super::{Adapter, Ctx};
use crate::lock::{Package, Root};

const PACKAGE_NAME: &str = "squizlabs/php_codesniffer";
const PACKAGE_TYPE: &str = "phpcodesniffer-standard";
const SEARCH_DEPTH_KEY: &str = "phpcodesniffer-search-depth";
const DEFAULT_MAX_DEPTH: u32 = 3;

pub(super) struct Phpcs;

impl Adapter for Phpcs {
    fn plugin_names(&self) -> &'static [&'static str] {
        &["dealerdirect/phpcodesniffer-composer-installer"]
    }

    fn upstream_version(&self) -> &'static str {
        "v1.2.1"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/plugins/phpcs"
    }

    fn post_install(&self, ctx: &Ctx<'_>, bin_packages: &[(&Package, PathBuf)]) -> Result<()> {
        apply(ctx.root, bin_packages)
    }
}

fn apply(root: &Root, packages: &[(&Package, PathBuf)]) -> Result<()> {
    let Some((_, phpcs_dir)) = packages.iter().find(|(p, _)| p.name == PACKAGE_NAME) else {
        // `MESSAGE_NOT_INSTALLED`/`MESSAGE_PLUGIN_UNINSTALLED`: phpcs itself
        // isn't part of this install, nothing to configure.
        return Ok(());
    };
    let min_depth = min_depth(packages);
    let max_depth = max_depth(root)?;

    let mut standards_paths = Vec::new();
    for (package, install_dir) in packages {
        if package.r#type != PACKAGE_TYPE {
            continue;
        }
        find_rulesets(install_dir, 0, max_depth, min_depth, &mut standards_paths)?;
    }
    if standards_paths.is_empty() {
        // `MESSAGE_NOTHING_TO_INSTALL`: no change from an (assumed empty)
        // starting config, so `saveInstalledPaths` never runs at all.
        return Ok(());
    }

    let installed_paths: std::collections::BTreeSet<String> = standards_paths
        .into_iter()
        // `if ($standardsPath !== $this->cwd) { $standardsPath = dirname($standardsPath); }`:
        // never equal to `cwd` here, root-package standards aren't ported.
        .filter_map(|dir| dir.parent().map(|p| relative_path(phpcs_dir, p)))
        .collect();
    // `sort($this->installedPaths)`, applied after Composer's own de-dup
    // (`in_array` before push); a `BTreeSet` already gives both.
    let paths = installed_paths.into_iter().collect::<Vec<_>>().join(",");

    run_phpcs(phpcs_dir, &["--config-set", "installed_paths", &paths])
}

/// `getMinDepth`: `0` for `PHP_CodeSniffer` >= 3, `1` for the ancient 1.x/2.x
/// line (whose standards nest one directory deeper). Falls back to the
/// modern default when the version can't be parsed.
fn min_depth(packages: &[(&Package, PathBuf)]) -> u32 {
    let major = packages
        .iter()
        .find(|(p, _)| p.name == PACKAGE_NAME)
        .and_then(|(p, _)| p.version.split(['.', '-']).next())
        .and_then(|s| s.parse::<u32>().ok());
    u32::from(major.is_some_and(|m| m < 3))
}

/// `getMaxDepth`: `extra.phpcodesniffer-search-depth`, an integer strictly
/// larger than the min depth, defaulting to 3.
fn max_depth(root: &Root) -> Result<u32> {
    let Some(value) = root.extra.get(SEARCH_DEPTH_KEY) else {
        return Ok(DEFAULT_MAX_DEPTH);
    };
    value
        .as_u64()
        .and_then(|v| u32::try_from(v).ok())
        .with_context(|| {
            format!(
                "The value of \"{SEARCH_DEPTH_KEY}\" (in the composer.json \"extra\" section) must \
             be an integer larger than 0, {value} given."
            )
        })
}

/// `updateInstalledPaths`'s `Finder`: every `ruleset.xml` under `dir`,
/// `depth` levels down (0 = directly in `dir`), collecting the directory that
/// contains each match.
fn find_rulesets(
    dir: &Path,
    depth: u32,
    max_depth: u32,
    min_depth: u32,
    out: &mut Vec<PathBuf>,
) -> Result<()> {
    // `ignoreUnreadableDirs`: a directory this process can't read is skipped,
    // not an error.
    let Ok(entries) = fs_err::read_dir(dir) else {
        return Ok(());
    };
    let mut subdirs = Vec::new();
    for entry in entries {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            subdirs.push(entry.path());
        } else if depth >= min_depth && depth <= max_depth && entry.file_name() == "ruleset.xml" {
            out.push(dir.to_path_buf());
        }
    }
    if depth < max_depth {
        for subdir in subdirs {
            find_rulesets(&subdir, depth + 1, max_depth, min_depth, out)?;
        }
    }
    Ok(())
}

/// `Filesystem::findShortestPath($from, $to, true)` for two already-absolute,
/// already-canonical paths: strip the common prefix, then `..` back up from
/// `from` and back down into `to`. No trailing separator, matching the
/// plugin's own output (`../../phpcsstandards/phpcsextra`, not `.../`).
pub(super) fn relative_path(from: &Path, to: &Path) -> String {
    let from_parts: Vec<_> = from.components().collect();
    let to_parts: Vec<_> = to.components().collect();
    let common = from_parts
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    let mut parts: Vec<String> = vec!["..".to_string(); from_parts.len() - common];
    parts.extend(
        to_parts[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

/// Runs `php <phpcs_dir>/bin/phpcs <args>` with `phpcs_dir` as the working
/// directory, matching `getPhpcsCommand`/`getPHPCodeSnifferInstallPath`.
/// Skipped with a warning when `php` itself isn't on `PATH`, the same
/// tolerance `src/scripts.rs` gives a missing interpreter.
fn run_phpcs(phpcs_dir: &Path, args: &[&str]) -> Result<()> {
    let bin = phpcs_dir.join("bin/phpcs");
    match Command::new("php")
        .arg(&bin)
        .args(args)
        .current_dir(phpcs_dir)
        .output()
    {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => bail!(
            "phpcs {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(err) if err.kind() == ErrorKind::NotFound => {
            tracing::warn!(
                "dealerdirect/phpcodesniffer-composer-installer: php is not on PATH, skipping \
                 `phpcs --config-set installed_paths`"
            );
            Ok(())
        }
        Err(err) => Err(err).with_context(|| format!("running {}", bin.display())),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;

    fn root(extra: &Value) -> Root {
        serde_json::from_value(json!({"extra": extra})).unwrap()
    }

    #[test]
    fn relative_path_strips_common_prefix_and_climbs_back_down() {
        let from = Path::new("/vendor/squizlabs/php_codesniffer");
        let to = Path::new("/vendor/phpcsstandards/phpcsextra");
        assert_eq!(relative_path(from, to), "../../phpcsstandards/phpcsextra");
    }

    #[test]
    fn relative_path_is_empty_between_identical_paths() {
        let dir = Path::new("/vendor/acme/pkg");
        assert_eq!(relative_path(dir, dir), "");
    }

    #[test]
    fn max_depth_defaults_to_three() {
        assert_eq!(max_depth(&root(&json!({}))).unwrap(), 3);
    }

    #[test]
    fn max_depth_reads_the_extra_key() {
        assert_eq!(
            max_depth(&root(&json!({"phpcodesniffer-search-depth": 5}))).unwrap(),
            5
        );
    }

    #[test]
    fn max_depth_rejects_a_non_integer() {
        let err = max_depth(&root(&json!({"phpcodesniffer-search-depth": "deep"}))).unwrap_err();
        assert!(err.to_string().contains("phpcodesniffer-search-depth"));
    }

    #[test]
    fn find_rulesets_respects_min_and_max_depth() {
        let dir = tempfile::tempdir().unwrap();
        // depth 0: directly in the search root.
        fs_err::write(dir.path().join("ruleset.xml"), "").unwrap();
        // depth 1: one directory down, the common shape for a standard.
        fs_err::create_dir(dir.path().join("Standard")).unwrap();
        fs_err::write(dir.path().join("Standard/ruleset.xml"), "").unwrap();
        // depth 2: past max_depth 1, must not be found.
        fs_err::create_dir(dir.path().join("Standard/Sub")).unwrap();
        fs_err::write(dir.path().join("Standard/Sub/ruleset.xml"), "").unwrap();

        let mut found = Vec::new();
        find_rulesets(dir.path(), 0, 1, 1, &mut found).unwrap();
        assert_eq!(found, vec![dir.path().join("Standard")]);
    }

    #[test]
    fn find_rulesets_on_a_missing_dir_is_a_no_op() {
        let mut found = Vec::new();
        find_rulesets(Path::new("/does/not/exist"), 0, 3, 0, &mut found).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn apply_is_a_no_op_when_phpcs_itself_is_not_installed() {
        let root = root(&json!({}));
        let packages: Vec<(&Package, PathBuf)> = Vec::new();
        apply(&root, &packages).unwrap();
    }
}
