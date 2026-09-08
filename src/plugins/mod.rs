//! Native adapters for the Composer plugins vivace ports
//! (`docs/plugin-strategy.md`'s rules 1, 2 and 3: `composer/installers` and the
//! `wordpress-core-installer` pair map install paths;
//! `dealerdirect/phpcodesniffer-composer-installer`, `phpstan/extension-installer`,
//! `tbachert/spi`, `php-http/discovery`, `yiisoft/yii2-composer`,
//! `craftcms/plugin-installer` and `codeception/c3` generate a file or run a
//! command after install; `cweagans/composer-patches` applies patches after
//! each package lands; `ffraenz/private-composer-installer` resolves a
//! dist URL's placeholders in `fetch::Fetcher` right before the download
//! request, the one adapter here with no apply phase of its own), plus rule
//! 3's refusal for every other `composer-plugin` in the lock.
//!
//! The path-mapping pair only ever changes *where* a package lands on disk:
//! this module computes a project-relative install directory per package;
//! every downstream consumer (`link_tree`'s target, `installed.json`/`.php`,
//! the autoload paths, `vendor/bin` proxies, the plan's keep/remove diff)
//! already renders whatever absolute or relative path it is given, vendor or
//! not, so none of them need to know a plugin was involved at all. The six
//! generator adapters (`phpcs`, `phpstan`, `spi`, `discovery`, `yii2`,
//! `craft`) instead run once, after every package has landed in its final
//! spot, from `src/install.rs`'s own `post-install-cmd`/`pre-autoload-dump`
//! hook points — `yii2`/`craft`'s own doc comments explain why that's a
//! stand-in for their real per-package hook. `patches` runs earlier still,
//! right after linking and before the autoloader is (re)generated, since a
//! patch can add or remove classes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::lock::{Lock, Package, Root};
use crate::store::Store;

mod c3;
mod craft;
mod discovery;
mod drupal_scaffold;
mod installers;
mod patches;
mod phpcs;
mod phpstan;
pub mod private_installer;
mod spi;
mod symfony_runtime;
mod wordpress_core;
mod yii2;

/// A PHP single-quoted string literal: only `\` and `'` need escaping.
pub(super) fn php_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// `yii2`/`craft` key their generated file's entries by Composer's own
/// install order, not `packages`' own (a fresh `craftcms/craft` install byte
/// -diffed 54 lines off `vendor/yiisoft/extensions.php` without this).
/// Reorders `packages` via [`crate::autoload::generator::install_order`],
/// keeping each entry's own install dir.
pub(super) fn in_install_order<'a>(
    packages: &'a [(&'a Package, PathBuf)],
) -> Vec<(&'a Package, &'a Path)> {
    let only: Vec<&Package> = packages.iter().map(|(p, _)| *p).collect();
    let mut by_name: HashMap<&str, &Path> = HashMap::new();
    for (package, dir) in packages {
        by_name
            .entry(package.name.as_str())
            .or_insert(dir.as_path());
    }
    crate::autoload::generator::install_order(&only)
        .into_iter()
        .filter_map(|p| by_name.get(p.name.as_str()).map(|&dir| (p, dir)))
        .collect()
}

/// `crate::lock::Package`'s side of [`crate::autoload::generator::Requires`]
/// (#130): its `require`/`provide`/`replace` are JSON maps, not `Vec<String>`
/// like `autoload::generator::Package`'s own, but the DFS only ever needs
/// their keys in insertion order (`serde_json`'s `preserve_order` feature).
impl crate::autoload::generator::Requires for Package {
    fn install_order_name(&self) -> &str {
        &self.name
    }

    fn install_order_requires(&self) -> impl Iterator<Item = &str> {
        self.require.keys().map(String::as_str)
    }

    fn install_order_provides(&self) -> impl Iterator<Item = &str> {
        self.provide
            .keys()
            .chain(self.replace.keys())
            .map(String::as_str)
    }
}

/// One native adapter for one Composer plugin (or, for the `WordPress` core
/// installer pair, both packages that ship it). Every method has a default
/// no-op so an adapter implements only the phases its plugin hooks — see
/// each submodule's own doc comment for which phase(s) it uses and why.
pub(crate) trait Adapter {
    /// Packagist name(s) this adapter stands in for.
    fn plugin_names(&self) -> &'static [&'static str];
    /// Upstream version the port was made from — the module doc comment's
    /// own pinned version, or, absent one, the version pinned in this
    /// adapter's own fixture `composer.lock`. Read by `viv diagnose
    /// --adapters` (#127 part 3's drift workflow).
    fn upstream_version(&self) -> &'static str;

    /// Install-path mapping (`composer/installers`, the `WordPress` core
    /// installer pair).
    fn install_dir(&self, _root: &Root, _package: &Package) -> Option<String> {
        None
    }
    /// Dist URL rewriting before fetch (`ffraenz/private-composer-installer`,
    /// the one adapter with no other phase of its own).
    fn fetch_env(&self, _root: &Root, _project_dir: &Path) -> Option<private_installer::Env> {
        None
    }
    /// Part of the install state that must trigger a rerun when it changes
    /// (`cweagans/composer-patches`).
    fn state_fingerprint(&self, _root: &Root, _project_dir: &Path) -> Result<Option<String>> {
        Ok(None)
    }
    /// After packages are linked, before the autoloader is (re)generated
    /// (`cweagans/composer-patches`, `drupal/core-composer-scaffold`).
    fn post_link(
        &self,
        _ctx: &Ctx<'_>,
        _newly_linked: &[Package],
        _kept: &[&Package],
        _store: &Store,
    ) -> Result<()> {
        Ok(())
    }
    /// Extra classmap entries for the root autoload
    /// (`drupal/core-composer-scaffold`'s `DrupalInstalled.php`).
    fn extra_classmap(&self, _ctx: &Ctx<'_>, _packages: &[&Package]) -> Result<Vec<String>> {
        Ok(Vec::new())
    }
    /// Generators that run before the autoload dump
    /// (`phpstan/extension-installer`, `tbachert/spi`, `php-http/discovery`,
    /// `yiisoft/yii2-composer`, `craftcms/plugin-installer`, `codeception/c3`).
    fn pre_autoload_dump(&self, _ctx: &Ctx<'_>, _packages: &[(&Package, PathBuf)]) -> Result<()> {
        Ok(())
    }
    /// After the autoload dump (`symfony/runtime`).
    fn post_autoload_dump(&self, _ctx: &Ctx<'_>) -> Result<()> {
        Ok(())
    }
    /// After install, once `vendor/bin` exists
    /// (`dealerdirect/phpcodesniffer-composer-installer`,
    /// `phpstan/extension-installer`).
    fn post_install(&self, _ctx: &Ctx<'_>, _bin_packages: &[(&Package, PathBuf)]) -> Result<()> {
        Ok(())
    }
}

/// What every phase past install-path mapping and the dist-URL rewrite (both
/// resolved before any of this exists) gets: no adapter reaches for globals
/// or re-derives a path one of its callers already has.
pub(crate) struct Ctx<'a> {
    pub root: &'a Root,
    pub project_dir: &'a Path,
    pub vendor_dir: &'a Path,
}

/// Composer plugins vivace applies the effect of natively, one variant per
/// adapter module. `NATIVE_ADAPTERS` is this enum's canonical registration
/// order: every [`Plugins`] method loops the enabled adapters in that order,
/// which output must never depend on
/// (`tests::adapter_order_does_not_affect_the_wordpress_fixture` shuffles it).
#[derive(Clone, Copy, PartialEq, Eq)]
enum AdapterId {
    Installers,
    WordpressCore,
    Phpcs,
    Phpstan,
    Spi,
    Patches,
    Discovery,
    Yii2,
    Craft,
    PrivateInstaller,
    C3,
    DrupalScaffold,
    SymfonyRuntime,
}

const NATIVE_ADAPTERS: &[AdapterId] = &[
    AdapterId::Installers,
    AdapterId::WordpressCore,
    AdapterId::Phpcs,
    AdapterId::Phpstan,
    AdapterId::Spi,
    AdapterId::Patches,
    AdapterId::Discovery,
    AdapterId::Yii2,
    AdapterId::Craft,
    AdapterId::PrivateInstaller,
    AdapterId::C3,
    AdapterId::DrupalScaffold,
    AdapterId::SymfonyRuntime,
];

/// Every registered adapter, in `NATIVE_ADAPTERS` order, regardless of what
/// any one project's lock enables — `viv diagnose --adapters` (#127 part 3's
/// drift workflow) reports on the whole registry, not a resolved `Plugins`.
pub(crate) fn all_adapters() -> impl Iterator<Item = Box<dyn Adapter>> {
    NATIVE_ADAPTERS.iter().map(|id| make_adapter(*id))
}

fn make_adapter(id: AdapterId) -> Box<dyn Adapter> {
    match id {
        AdapterId::Installers => Box::new(installers::Installers),
        AdapterId::WordpressCore => Box::new(wordpress_core::WordpressCore),
        AdapterId::Phpcs => Box::new(phpcs::Phpcs),
        AdapterId::Phpstan => Box::new(phpstan::Phpstan),
        AdapterId::Spi => Box::new(spi::Spi),
        AdapterId::Patches => Box::new(patches::Patches),
        AdapterId::Discovery => Box::new(discovery::Discovery),
        AdapterId::Yii2 => Box::new(yii2::Yii2),
        AdapterId::Craft => Box::new(craft::Craft),
        AdapterId::PrivateInstaller => Box::new(private_installer::PrivateInstaller),
        AdapterId::C3 => Box::new(c3::C3),
        AdapterId::DrupalScaffold => Box::new(drupal_scaffold::DrupalScaffold),
        AdapterId::SymfonyRuntime => Box::new(symfony_runtime::SymfonyRuntime),
    }
}

/// Composer plugins that only affect commands vivace doesn't implement
/// (`composer normalize`), or that a fresh `install`/`update` never triggers
/// at all (`drupal/core-project-message` only prints a message on
/// `create-project`/`install`, no filesystem effect; `drupal/core-recipe-unpack`
/// only subscribes to `POST_UPDATE_CMD`/`POST_CREATE_PROJECT_CMD`, so a plain
/// `install` never reaches it either); ignored silently, same as Composer
/// ignores a plugin `allow-plugins` sets to `false`.
const KNOWN_INERT: &[&str] = &[
    "ergebnis/composer-normalize",
    "drupal/core-project-message",
    "drupal/core-recipe-unpack",
];

/// Which native adapters are active for this install, resolved once from the
/// lock and the root `composer.json` ([`resolve`]), in `NATIVE_ADAPTERS`
/// registration order — the order every method below loops them in.
pub struct Plugins {
    adapters: Vec<Box<dyn Adapter>>,
}

impl std::fmt::Debug for Plugins {
    /// Not derived: `Box<dyn Adapter>` has no `Debug` impl of its own — this
    /// prints each enabled adapter's Packagist name(s) instead.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugins")
            .field(
                "adapters",
                &self
                    .adapters
                    .iter()
                    .map(|adapter| adapter.plugin_names())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// Resolve which native adapters apply and check every other enabled
/// `composer-plugin` against the known-inert/refuse rule
/// (`docs/plugin-strategy.md`'s rule 3). `no_plugins` (`--no-plugins`)
/// disables every adapter, installing every package under `vendor/` as
/// Composer would with the same flag, and downgrades a refusal to a warning
/// line the caller should print.
pub fn resolve(lock: &Lock, root: &Root, no_plugins: bool) -> Result<(Plugins, Vec<String>)> {
    let allow = &root.config.allow_plugins;
    let mut enabled = vec![false; NATIVE_ADAPTERS.len()];
    let mut warnings = Vec::new();
    for package in &lock.packages {
        if package.r#type != "composer-plugin" || !allow.is_enabled(&package.name) {
            continue;
        }
        if KNOWN_INERT.contains(&package.name.as_str()) {
            continue;
        }
        let native = NATIVE_ADAPTERS.iter().position(|id| {
            make_adapter(*id)
                .plugin_names()
                .contains(&package.name.as_str())
        });
        if let Some(index) = native {
            if !no_plugins {
                enabled[index] = true;
            }
            continue;
        }
        if no_plugins {
            // Composer's own wording for the same situation.
            warnings.push(format!(
                "The \"{}\" plugin was not loaded as plugins are disabled.",
                package.name
            ));
        } else {
            bail!(
                "viv cannot run the Composer plugin {}; see docs/plugin-strategy.md. Pass \
                 --no-plugins to install without it, as Composer would.",
                package.name
            );
        }
    }
    let adapters = NATIVE_ADAPTERS
        .iter()
        .zip(enabled)
        .filter(|(_, on)| *on)
        .map(|(id, _)| make_adapter(*id))
        .collect();
    Ok((Plugins { adapters }, warnings))
}

impl Plugins {
    /// The project-root-relative install directory `package` maps to (no
    /// leading/trailing slash), or `None` to keep the default `vendor/<name>`.
    pub fn install_dir(&self, root: &Root, package: &Package) -> Option<String> {
        self.adapters
            .iter()
            .find_map(|adapter| adapter.install_dir(root, package))
    }

    /// `ffraenz/private-composer-installer`'s dist-URL environment, built
    /// once per install and threaded into `fetch::Fetcher` right before the
    /// download request — the seam a caller in `install.rs` checks first.
    pub fn fetch_env(&self, root: &Root, project_dir: &Path) -> Option<private_installer::Env> {
        self.adapters
            .iter()
            .find_map(|adapter| adapter.fetch_env(root, project_dir))
    }

    /// `State::patches_fingerprint`: `None` unless an active adapter has
    /// state of its own to fingerprint (`cweagans/composer-patches`).
    pub fn state_fingerprint(&self, root: &Root, project_dir: &Path) -> Result<Option<String>> {
        for adapter in &self.adapters {
            if let Some(fingerprint) = adapter.state_fingerprint(root, project_dir)? {
                return Ok(Some(fingerprint));
            }
        }
        Ok(None)
    }

    /// `cweagans/composer-patches`' `POST_PACKAGE_INSTALL`/`POST_PACKAGE_UPDATE`
    /// listener and `drupal/core-composer-scaffold`'s `Handler::scaffold`
    /// (`POST_INSTALL_CMD`/`POST_UPDATE_CMD`): runs right after
    /// `link_archives`, before the autoloader is (re)generated. `newly_linked`
    /// are the packages `link_archives` just wrote (pristine, unpatched);
    /// `kept` are the ones this run left alone, which may already carry a
    /// previous run's patches.
    pub(crate) fn post_link(
        &self,
        ctx: &Ctx<'_>,
        newly_linked: &[Package],
        kept: &[&Package],
        store: &Store,
    ) -> Result<()> {
        for adapter in &self.adapters {
            adapter.post_link(ctx, newly_linked, kept, store)?;
        }
        Ok(())
    }

    /// `drupal/core-composer-scaffold`'s `Plugin::preAutoloadDump`: extra
    /// classmap entries the caller merges into the root package's autoload
    /// before the classmap scan runs.
    pub(crate) fn extra_classmap(
        &self,
        ctx: &Ctx<'_>,
        packages: &[&Package],
    ) -> Result<Vec<String>> {
        let mut classmap = Vec::new();
        for adapter in &self.adapters {
            classmap.extend(adapter.extra_classmap(ctx, packages)?);
        }
        Ok(classmap)
    }

    /// `tbachert/spi` and `php-http/discovery`'s `PRE_AUTOLOAD_DUMP`
    /// listeners, plus `yiisoft/yii2-composer`, `craftcms/plugin-installer`
    /// and `codeception/c3`'s stand-in for their own per-package hook: run
    /// from both `install` and `dump-autoload`, since both regenerate the
    /// autoloader.
    pub(crate) fn pre_autoload_dump(
        &self,
        ctx: &Ctx<'_>,
        packages: &[(&Package, PathBuf)],
    ) -> Result<()> {
        for adapter in &self.adapters {
            adapter.pre_autoload_dump(ctx, packages)?;
        }
        Ok(())
    }

    /// `symfony/runtime`'s `ComposerPlugin::updateAutoloadFile`
    /// (`POST_AUTOLOAD_DUMP`): writes `vendor/autoload_runtime.php`, called
    /// after `install::write_autoload` dispatches that script event, the
    /// same phase the real plugin subscribes to.
    pub(crate) fn post_autoload_dump(&self, ctx: &Ctx<'_>) -> Result<()> {
        for adapter in &self.adapters {
            adapter.post_autoload_dump(ctx)?;
        }
        Ok(())
    }

    /// The `POST_INSTALL_CMD`-only adapters (`dealerdirect/phpcodesniffer-composer-installer`,
    /// `phpstan/extension-installer`): both subscribe to `post-install-cmd`/
    /// `post-update-cmd` only, never to a bare `dump-autoload`, so this is
    /// called from `install::run` alone, not `install::dump_autoload`.
    pub(crate) fn post_install(
        &self,
        ctx: &Ctx<'_>,
        bin_packages: &[(&Package, PathBuf)],
    ) -> Result<()> {
        for adapter in &self.adapters {
            adapter.post_install(ctx, bin_packages)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::read_lock;
    use serde_json::{Value, json};
    use std::io::Write as _;
    use std::path::Path;

    fn root(json: Value) -> Root {
        serde_json::from_value(json).unwrap()
    }

    fn lock_with(packages: &[Value]) -> Lock {
        let lock_json = json!({ "packages": packages, "packages-dev": [] });
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(lock_json.to_string().as_bytes()).unwrap();
        read_lock(file.path()).unwrap()
    }

    fn plugin_package(name: &str) -> Value {
        json!({ "name": name, "version": "1.0.0", "type": "composer-plugin" })
    }

    /// Whether some enabled adapter stands in for `name` — the replacement
    /// for the old one-bool-per-adapter `Plugins` struct's own fields, which
    /// a test could set directly; a `Vec<Box<dyn Adapter>>` can't, so this
    /// asks the same question through `plugin_names()` instead.
    fn has(plugins: &Plugins, name: &str) -> bool {
        plugins
            .adapters
            .iter()
            .any(|adapter| adapter.plugin_names().contains(&name))
    }

    #[test]
    fn native_adapter_enabled_by_allow_plugins_activates() {
        let lock = lock_with(&[plugin_package("composer/installers")]);
        let root = root(json!({"config": {"allow-plugins": {"composer/installers": true}}}));
        let (plugins, warnings) = resolve(&lock, &root, false).unwrap();
        assert!(warnings.is_empty());
        assert!(has(&plugins, "composer/installers"));
    }

    #[test]
    fn native_adapter_not_allowed_stays_inactive_and_does_not_error() {
        let lock = lock_with(&[plugin_package("composer/installers")]);
        let root = root(json!({}));
        let (plugins, warnings) = resolve(&lock, &root, false).unwrap();
        assert!(warnings.is_empty());
        assert!(!has(&plugins, "composer/installers"));
    }

    #[test]
    fn known_inert_plugin_is_ignored() {
        let lock = lock_with(&[plugin_package("ergebnis/composer-normalize")]);
        let root =
            root(json!({"config": {"allow-plugins": {"ergebnis/composer-normalize": true}}}));
        let (plugins, warnings) = resolve(&lock, &root, false).unwrap();
        assert!(warnings.is_empty());
        assert!(plugins.adapters.is_empty());
    }

    #[test]
    fn unknown_enabled_plugin_errors_naming_it() {
        let lock = lock_with(&[plugin_package("acme/mystery-plugin")]);
        let root = root(json!({"config": {"allow-plugins": {"acme/mystery-plugin": true}}}));
        let err = resolve(&lock, &root, false).unwrap_err();
        assert!(err.to_string().contains("acme/mystery-plugin"), "{err}");
        assert!(err.to_string().contains("docs/plugin-strategy.md"), "{err}");
    }

    #[test]
    fn unknown_enabled_plugin_with_no_plugins_only_warns() {
        let lock = lock_with(&[plugin_package("acme/mystery-plugin")]);
        let root = root(json!({"config": {"allow-plugins": {"acme/mystery-plugin": true}}}));
        let (plugins, warnings) = resolve(&lock, &root, true).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("acme/mystery-plugin"));
        assert!(plugins.adapters.is_empty());
    }

    #[test]
    fn allow_plugins_wildcard_matches() {
        let lock = lock_with(&[plugin_package("composer/installers")]);
        let root = root(json!({"config": {"allow-plugins": {"composer/*": true}}}));
        let (plugins, _) = resolve(&lock, &root, false).unwrap();
        assert!(has(&plugins, "composer/installers"));
    }

    #[test]
    fn allow_plugins_true_enables_everything() {
        let lock = lock_with(&[plugin_package("composer/installers")]);
        let root = root(json!({"config": {"allow-plugins": true}}));
        let (plugins, _) = resolve(&lock, &root, false).unwrap();
        assert!(has(&plugins, "composer/installers"));
    }

    #[test]
    fn composer_patches_enabled_by_allow_plugins_activates() {
        let lock = lock_with(&[plugin_package("cweagans/composer-patches")]);
        let root = root(json!({"config": {"allow-plugins": {"cweagans/composer-patches": true}}}));
        let (plugins, warnings) = resolve(&lock, &root, false).unwrap();
        assert!(warnings.is_empty());
        assert!(has(&plugins, "cweagans/composer-patches"));
    }

    #[test]
    fn resolve_registers_enabled_adapters_in_native_adapters_order_regardless_of_lock_order() {
        // `tbachert/spi` is after `cweagans/composer-patches` in
        // `NATIVE_ADAPTERS`, but listed first in the lock here — the
        // registration order must come from `NATIVE_ADAPTERS`, never from
        // lock order.
        let lock = lock_with(&[
            plugin_package("tbachert/spi"),
            plugin_package("cweagans/composer-patches"),
        ]);
        let root = root(json!({"config": {"allow-plugins": true}}));
        let (plugins, _) = resolve(&lock, &root, false).unwrap();
        let names: Vec<_> = plugins
            .adapters
            .iter()
            .map(|a| a.plugin_names()[0])
            .collect();
        assert_eq!(names, ["tbachert/spi", "cweagans/composer-patches"]);
    }

    /// #127 part 1: `composer/installers` and the `WordPress` core installer
    /// pair — the `wordpress` fixture's own two native adapters, its own
    /// `composer.lock` (`tests/fixtures/wordpress/composer.lock`) ties
    /// `drupal`'s for "most adapters enabled", and is the one pair that
    /// shares a single phase (`install_dir`) where a registration-order bug
    /// would actually be observable. `install_dir` is a pure function of
    /// `root`/`package`, so this drives it directly rather than a full `viv
    /// install`.
    #[test]
    fn adapter_order_does_not_affect_the_wordpress_fixture() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wordpress");
        let root =
            crate::lock::parse_root(&fs_err::read(fixture.join("composer.json")).unwrap()).unwrap();
        let lock = read_lock(&fixture.join("composer.lock")).unwrap();

        let (default_order, _) = resolve(&lock, &root, false).unwrap();
        assert_eq!(
            default_order.adapters.len(),
            2,
            "fixture should enable both adapters"
        );
        let enabled_ids: Vec<AdapterId> = NATIVE_ADAPTERS
            .iter()
            .copied()
            .filter(|id| {
                let names = make_adapter(*id).plugin_names();
                default_order
                    .adapters
                    .iter()
                    .any(|a| a.plugin_names() == names)
            })
            .collect();
        let shuffled = Plugins {
            adapters: enabled_ids
                .iter()
                .rev()
                .map(|id| make_adapter(*id))
                .collect(),
        };

        for package in &lock.packages {
            assert_eq!(
                default_order.install_dir(&root, package),
                shuffled.install_dir(&root, package),
                "{} differs by adapter registration order",
                package.name
            );
        }
    }
}
