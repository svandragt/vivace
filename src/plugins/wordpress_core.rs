//! `johnpbloch/wordpress-core-installer`/`roots/wordpress-core-installer`'s
//! `getInstallPath`: maps a `wordpress-core` package to the root's
//! `extra.wordpress-install-dir` (a string, or a map keyed by the package's
//! pretty name), falling back to the package's own
//! `extra.wordpress-install-dir`, then the literal `"wordpress"`. Version
//! pinned in `tests/fixtures/wordpress/composer.lock`.

use serde_json::Value;

use crate::lock::{Package, Root};

use super::Adapter;

pub(super) struct WordpressCore;

impl Adapter for WordpressCore {
    fn plugin_names(&self) -> &'static [&'static str] {
        &[
            "johnpbloch/wordpress-core-installer",
            "roots/wordpress-core-installer",
        ]
    }

    fn upstream_version(&self) -> &'static str {
        "2.0.0"
    }

    fn fixture(&self) -> &'static str {
        "tests/fixtures/wordpress"
    }

    fn install_dir(&self, root: &Root, package: &Package) -> Option<String> {
        (package.r#type == "wordpress-core").then(|| wordpress_install_dir(root, package))
    }
}

/// `johnpbloch/wordpress-core-installer`/`roots/wordpress-core-installer`'s
/// `getInstallPath`: the root's `extra.wordpress-install-dir` (a string, or a
/// map keyed by the package's pretty name), falling back to the package's
/// own `extra.wordpress-install-dir`, then the literal `"wordpress"`.
fn wordpress_install_dir(root: &Root, package: &Package) -> String {
    let from_root = root.extra.get("wordpress-install-dir").and_then(|v| {
        v.as_str().map(str::to_owned).or_else(|| {
            v.as_object()
                .and_then(|m| m.get(&package.name))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
    });
    from_root
        .or_else(|| {
            package
                .raw
                .pointer("/extra/wordpress-install-dir")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| "wordpress".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
    fn wordpress_core_default_dir() {
        let root = root(json!({}));
        let dir = WordpressCore
            .install_dir(
                &root,
                &package("johnpbloch/wordpress-core", "wordpress-core"),
            )
            .unwrap();
        assert_eq!(dir, "wordpress");
    }

    #[test]
    fn wordpress_core_install_dir_from_root_extra_string() {
        let root = root(json!({"extra": {"wordpress-install-dir": "wp"}}));
        let dir = WordpressCore
            .install_dir(
                &root,
                &package("johnpbloch/wordpress-core", "wordpress-core"),
            )
            .unwrap();
        assert_eq!(dir, "wp");
    }

    #[test]
    fn wordpress_core_install_dir_from_root_extra_map_by_package_name() {
        let root = root(json!({
            "extra": {"wordpress-install-dir": {"johnpbloch/wordpress-core": "web/wp"}}
        }));
        let dir = WordpressCore
            .install_dir(
                &root,
                &package("johnpbloch/wordpress-core", "wordpress-core"),
            )
            .unwrap();
        assert_eq!(dir, "web/wp");
    }

    #[test]
    fn wordpress_core_install_dir_falls_back_to_package_extra() {
        let raw = json!({
            "name": "acme/wp",
            "version": "1.0.0",
            "type": "wordpress-core",
            "extra": {"wordpress-install-dir": "own-dir"}
        });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        let root = root(json!({}));
        assert_eq!(
            WordpressCore.install_dir(&root, &package).unwrap(),
            "own-dir"
        );
    }
}
