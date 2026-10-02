//! `altis/dev-tools-command`'s `Plugin::install_files`
//! (`inc/command/class-plugin.php`, `post-autoload-dump`): seeds two CI
//! config files from `vendor/altis/dev-tools/travis/` the first time a
//! project has neither (`.config/travis.yml` from `travis/tests.yml`,
//! `.travis.yml` from `travis/project.yml`), then keeps `.travis.yml`'s
//! `altis.yml@<ref>` pin in sync with the installed `altis/dev-tools`
//! version — but only while `.travis.yml`, with that ref stripped back to
//! the bundled template's own placeholder, still matches the bundled
//! template byte-for-byte; a project whose `.travis.yml` has otherwise
//! diverged gets a console warning instead, same as the real plugin's
//! `echo`.
//!
//! Ported from `altis/dev-tools-command` `0.8.2` (`722f05c`, fetched
//! 2026-10-02, issue #355). Not command-only, unlike the issue's own
//! starting guess: `getSubscribedEvents` also wires `post-autoload-dump` to
//! this file-seeding/ref-sync pass, which a bare "adds a `composer
//! dev-tools` command" classification would have silently skipped on a
//! project missing `.travis.yml`/`.config/travis.yml` — the same
//! conditional-write shape `c3.rs`'s own doc comment calls out for
//! `codeception/c3`, so this gets a real adapter for the same reason.
//!
//! Byte equality stands in for the real plugin's `md5`/`md5_file` pair,
//! same shortcut `c3.rs` takes for its own comparison.
//!
//! Real Composer only reaches the ref-sync branch while `altis/dev-tools`
//! itself is also installed (`findPackage` would hand back `null`
//! otherwise and crash on `getPrettyVersion()`); this port treats that
//! package being absent from the install the same way — nothing to sync,
//! not an error — since every known Altis project requires it alongside
//! this plugin.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::lock::Package;

use super::{Adapter, Ctx};

const PACKAGE_NAME: &str = "altis/dev-tools-command";
const DEV_TOOLS_PACKAGE: &str = "altis/dev-tools";
const REF_MARKER: &str = "altis.yml@";
const REF_PLACEHOLDER: &str = "altis.yml@__ref_replace_me__";

pub(super) struct AltisDevToolsCommand;

impl Adapter for AltisDevToolsCommand {
    fn plugin_names(&self) -> &'static [&'static str] {
        &[PACKAGE_NAME]
    }

    fn upstream_version(&self) -> &'static str {
        "0.8.2"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/plugins/altis"
    }

    fn pre_autoload_dump(&self, ctx: &Ctx<'_>, packages: &[(&Package, PathBuf)]) -> Result<()> {
        apply(ctx.project_dir, ctx.vendor_dir, packages)
    }
}

fn apply(project_dir: &Path, vendor_dir: &Path, packages: &[(&Package, PathBuf)]) -> Result<()> {
    let Some((dev_tools, _)) = packages.iter().find(|(p, _)| p.name == DEV_TOOLS_PACKAGE) else {
        // Not required alongside this plugin: nothing to copy or sync.
        return Ok(());
    };
    let source = vendor_dir.join("altis/dev-tools/travis");

    let config_travis = project_dir.join(".config/travis.yml");
    if !config_travis.exists() {
        fs_err::create_dir_all(project_dir.join(".config"))
            .context("altis/dev-tools-command: creating .config")?;
        copy_if_source_exists(&source.join("tests.yml"), &config_travis)?;
    }

    let root_travis = project_dir.join(".travis.yml");
    if !root_travis.exists() {
        copy_if_source_exists(&source.join("project.yml"), &root_travis)?;
    }

    let Ok(existing) = fs_err::read_to_string(&root_travis) else {
        return Ok(());
    };
    let Ok(template) = fs_err::read_to_string(source.join("project.yml")) else {
        return Ok(());
    };

    let stripped = strip_ref(&existing);
    if stripped != template {
        // stderr via `writeln!`, not `eprintln!`, to satisfy the
        // `print_stderr` lint (`install.rs`'s own `warn_out` takes the same
        // route, not reusable here since it's private to that module).
        let _ = writeln!(
            std::io::stderr().lock(),
            "\nThe file .travis.yml does not match that required by Altis.\n\
             See the file at: {}/travis/project.yml\n\
             For more information follow this guide:\n\
             https://www.altis-dxp.com/resources/docs/dev-tools/continuous-integration/ \n",
            vendor_dir.join("altis/dev-tools").display()
        );
        return Ok(());
    }

    let version = dev_tools.version.trim_start_matches("dev-");
    let updated = stripped.replacen(REF_PLACEHOLDER, &format!("{REF_MARKER}{version}"), 1);
    fs_err::write(&root_travis, updated).context("altis/dev-tools-command: writing .travis.yml")?;
    Ok(())
}

fn copy_if_source_exists(source: &Path, dest: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    fs_err::copy(source, dest)
        .with_context(|| format!("altis/dev-tools-command: copying {}", source.display()))?;
    Ok(())
}

/// `preg_replace('#altis\.yml@.*#', 'altis.yml@__ref_replace_me__', ...)`:
/// `.` doesn't match a newline, so each matching line is truncated right
/// after `altis.yml@` and the placeholder appended, line terminator kept.
fn strip_ref(content: &str) -> String {
    content
        .split_inclusive('\n')
        .map(|line| match line.find(REF_MARKER) {
            Some(idx) => {
                let newline = if line.ends_with('\n') { "\n" } else { "" };
                format!("{}{REF_PLACEHOLDER}{newline}", &line[..idx])
            }
            None => line.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, version: &str) -> Package {
        let raw = serde_json::json!({ "name": name, "version": version, "type": "library" });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    #[test]
    fn seeds_both_files_and_syncs_the_ref_on_a_fresh_project() {
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let travis = vendor_dir.join("altis/dev-tools/travis");
        fs_err::create_dir_all(&travis).unwrap();
        fs_err::write(travis.join("tests.yml"), "tests: bundled\n").unwrap();
        fs_err::write(
            travis.join("project.yml"),
            "template: altis.yml@__ref_replace_me__\n",
        )
        .unwrap();

        let dev_tools = package(DEV_TOOLS_PACKAGE, "25.0.1");
        let packages: Vec<(&Package, PathBuf)> = vec![(&dev_tools, PathBuf::new())];
        apply(project_dir.path(), &vendor_dir, &packages).unwrap();

        assert_eq!(
            fs_err::read_to_string(project_dir.path().join(".config/travis.yml")).unwrap(),
            "tests: bundled\n"
        );
        assert_eq!(
            fs_err::read_to_string(project_dir.path().join(".travis.yml")).unwrap(),
            "template: altis.yml@25.0.1\n"
        );
    }

    #[test]
    fn leaves_a_diverged_travis_yml_untouched() {
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        let travis = vendor_dir.join("altis/dev-tools/travis");
        fs_err::create_dir_all(&travis).unwrap();
        fs_err::write(
            travis.join("project.yml"),
            "template: altis.yml@__ref_replace_me__\n",
        )
        .unwrap();
        fs_err::write(
            project_dir.path().join(".travis.yml"),
            "template: altis.yml@11.x\ncustom: true\n",
        )
        .unwrap();

        let dev_tools = package(DEV_TOOLS_PACKAGE, "25.0.1");
        let packages: Vec<(&Package, PathBuf)> = vec![(&dev_tools, PathBuf::new())];
        apply(project_dir.path(), &vendor_dir, &packages).unwrap();

        assert_eq!(
            fs_err::read_to_string(project_dir.path().join(".travis.yml")).unwrap(),
            "template: altis.yml@11.x\ncustom: true\n"
        );
    }

    #[test]
    fn is_a_no_op_without_altis_dev_tools_in_this_install() {
        let project_dir = tempfile::tempdir().unwrap();
        let vendor_dir = project_dir.path().join("vendor");
        apply(project_dir.path(), &vendor_dir, &[]).unwrap();
        assert!(!project_dir.path().join(".travis.yml").exists());
    }
}
