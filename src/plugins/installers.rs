//! `composer/installers`' `BaseInstaller::getInstallPath`: maps a package to
//! the root's `extra.installer-paths` (first pattern whose value list
//! contains the package's name, `type:<type>` or `vendor:<vendor>` wins), or
//! its package type's own default location. Both the location table and the
//! matching rule are data now (#340): `src/plugins/data/composer-installers.toml`,
//! loaded once by `super::data::install_path`. Version pinned in
//! `tests/fixtures/wordpress/composer.lock`.

use crate::lock::{Package, Root};

use super::Adapter;

pub(super) struct Installers;

impl Adapter for Installers {
    fn plugin_names(&self) -> &'static [&'static str] {
        &["composer/installers"]
    }

    fn upstream_version(&self) -> &'static str {
        "v2.3.0"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/wordpress"
    }

    fn install_dir(&self, root: &Root, package: &Package) -> Option<String> {
        super::data::install_path(package, &root.extra).map(|p| p.to_string_lossy().into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    fn root(json: Value) -> Root {
        serde_json::from_value(json).unwrap()
    }

    fn package(name: &str, r#type: &str) -> Package {
        let raw = json!({ "name": name, "version": "1.0.0", "type": r#type });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    #[test]
    fn installer_type_default_location() {
        let root = root(json!({}));
        let dir = Installers
            .install_dir(&root, &package("acme/hello", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "wp-content/plugins/hello/");
    }

    #[test]
    fn installer_paths_type_match_wins_over_default() {
        let root = root(json!({
            "extra": {
                "installer-paths": {
                    "custom/{$name}/": ["type:wordpress-plugin"]
                }
            }
        }));
        let dir = Installers
            .install_dir(&root, &package("acme/hello", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "custom/hello/");
    }

    #[test]
    fn installer_paths_exact_name_beats_vendor_and_type() {
        let root = root(json!({
            "extra": {
                "installer-paths": {
                    "by-name/{$name}/": ["acme/hello"],
                    "by-vendor/{$name}/": ["vendor:acme"],
                    "by-type/{$name}/": ["type:wordpress-plugin"]
                }
            }
        }));
        let dir = Installers
            .install_dir(&root, &package("acme/hello", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "by-name/hello/");
    }

    #[test]
    fn installer_paths_first_map_entry_wins_when_several_match() {
        let root = root(json!({
            "extra": {
                "installer-paths": {
                    "first/{$name}/": ["vendor:acme"],
                    "second/{$name}/": ["type:wordpress-plugin"]
                }
            }
        }));
        let dir = Installers
            .install_dir(&root, &package("acme/hello", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "first/hello/");
    }

    #[test]
    fn unsupported_type_falls_back_to_default_vendor_placement() {
        let root = root(json!({}));
        assert!(
            Installers
                .install_dir(&root, &package("acme/hello", "library"))
                .is_none()
        );
    }
}
