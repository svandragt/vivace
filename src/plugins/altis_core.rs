//! `altis/core`'s `Override_Installer` (`inc/composer/class-override-installer.php`,
//! registered by `inc/composer/class-plugin.php`'s `activate`/`init` and kept
//! current on `pre-operations-exec`/`post-dependencies-solving`/
//! `post-package-install`/`-update`): a `wordpress-plugin`/`wordpress-muplugin`
//! package named in the union of every installed package's
//! `extra.altis.install-overrides` list installs at the default
//! `vendor/<name>` instead of wherever `composer/installers`' own
//! type-template rule (or the root's `installer-paths` override) would
//! otherwise place it. Altis modules bundle these packages as internal
//! dependencies, `require`d through `vendor/modules.php`
//! (`altis_cms_installer.rs`'s `generate_module_manifest` port), not
//! activated as top-level `WordPress` plugins.
//!
//! A package outside that set returns `None` unchanged, so
//! `composer/installers`' own adapter answers exactly as if `altis/core`
//! weren't installed — matching `getInstallPath`'s own `parent::getInstallPath()`
//! fallback (Composer's "last-added installer wins" only lets
//! `Override_Installer` *add* a special case, never remove
//! `composer/installers`' own table).
//!
//! Ported from `altis/core` `25.0.8` (`cba6b99`, fetched 2026-10-02, issue
//! #355). The override set is lock-wide (every package's own
//! `extra.altis.install-overrides`, mirroring `init`'s own
//! `getLockedRepository( true )` — dev packages included, same as every
//! other native adapter here ignores the dev/no-dev split for install-path
//! purposes), so `super::resolve` computes it once from the whole lock and
//! threads it into this adapter's constructor: the one adapter that isn't a
//! unit struct, since no other port here needs data from packages other
//! than the one it is deciding for.

use std::collections::HashSet;

use crate::lock::{Package, Root};

use super::Adapter;

pub(super) struct AltisCore {
    overrides: HashSet<String>,
}

impl AltisCore {
    pub(super) fn new(overrides: HashSet<String>) -> Self {
        Self { overrides }
    }
}

impl Adapter for AltisCore {
    fn plugin_names(&self) -> &'static [&'static str] {
        &["altis/core"]
    }

    fn upstream_version(&self) -> &'static str {
        "25.0.8"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/plugins/altis"
    }

    fn install_dir(&self, root: &Root, package: &Package) -> Option<String> {
        if !matches!(
            package.r#type.as_str(),
            "wordpress-plugin" | "wordpress-muplugin"
        ) {
            return None;
        }
        if !self.overrides.contains(&package.name) {
            return None;
        }
        Some(format!(
            "{}/{}",
            root.config.vendor_dir,
            package.pretty_name()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn root() -> Root {
        serde_json::from_value(json!({})).unwrap()
    }

    fn package(name: &str, r#type: &str) -> Package {
        let raw = json!({ "name": name, "version": "1.0.0", "type": r#type });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    #[test]
    fn overridden_wordpress_plugin_installs_at_the_default_vendor_path() {
        let overrides: HashSet<String> = ["acme/bundled-plugin".to_string()].into();
        let altis_core = AltisCore::new(overrides);
        let dir = altis_core
            .install_dir(&root(), &package("acme/bundled-plugin", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "vendor/acme/bundled-plugin");
    }

    #[test]
    fn non_overridden_wordpress_plugin_falls_through() {
        let overrides: HashSet<String> = ["acme/bundled-plugin".to_string()].into();
        let altis_core = AltisCore::new(overrides);
        assert!(
            altis_core
                .install_dir(&root(), &package("acme/normal-plugin", "wordpress-plugin"))
                .is_none()
        );
    }

    #[test]
    fn overridden_name_with_an_unrelated_type_falls_through() {
        let overrides: HashSet<String> = ["acme/bundled-plugin".to_string()].into();
        let altis_core = AltisCore::new(overrides);
        assert!(
            altis_core
                .install_dir(&root(), &package("acme/bundled-plugin", "library"))
                .is_none()
        );
    }
}
