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
//! stand-in for their real per-package hook. [`patches`] runs earlier still,
//! right after linking and before the autoloader is (re)generated, since a
//! patch can add or remove classes.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::Value;

use crate::lock::{Lock, Package, Root};

mod c3;
mod craft;
mod discovery;
mod drupal_scaffold;
pub mod patches;
mod phpcs;
mod phpstan;
pub mod private_installer;
mod spi;
mod symfony_runtime;
mod yii2;

/// A PHP single-quoted string literal: only `\` and `'` need escaping.
pub(super) fn php_string(s: &str) -> String {
    format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'"))
}

/// `yii2`/`craft` key their generated file's entries by Composer's own
/// install order, not `packages`' own (a fresh `craftcms/craft` install byte
/// -diffed 54 lines off `vendor/yiisoft/extensions.php` without this).
/// Reorders `packages` via [`install_order`], keeping each entry's own
/// install dir.
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
    install_order(&only)
        .into_iter()
        .filter_map(|p| by_name.get(p.name.as_str()).map(|&dir| (p, dir)))
        .collect()
}

/// `Transaction::calculateOperations`'s install order, ported from
/// `autoload::generator::install_order` for `crate::lock::Package`'s
/// `require`/`provide`/`replace` maps — that module's own `Package` type
/// isn't reachable from here, so this is a second copy of the same DFS over
/// a different shape, not a shared helper widened; see the original's own
/// doc comment for the full algorithm notes (root selection order, cycle
/// handling, the at-most-one-provider ponytail this keeps too).
fn install_order<'a>(packages: &[&'a Package]) -> Vec<&'a Package> {
    let mut by_name: HashMap<&str, &'a Package> = HashMap::new();
    for package in packages {
        for name in std::iter::once(package.name.as_str())
            .chain(package.provide.keys().map(String::as_str))
            .chain(package.replace.keys().map(String::as_str))
        {
            by_name.entry(name).or_insert(package);
        }
    }
    let required_by_someone: HashSet<&str> = packages
        .iter()
        .flat_map(|p| p.require.keys())
        .filter_map(|name| by_name.get(name.as_str()))
        .map(|p| p.name.as_str())
        .collect();
    let mut roots: Vec<&Package> = packages
        .iter()
        .copied()
        .filter(|p| !required_by_someone.contains(p.name.as_str()))
        .collect();
    roots.sort_by(|a, b| a.name.cmp(&b.name));

    let mut visited = HashSet::new();
    let mut order = Vec::with_capacity(packages.len());
    for root in roots {
        install_order_visit(root, &by_name, &mut visited, &mut order);
    }
    // A require cycle with no true root would otherwise drop packages;
    // Composer's own stack never loses one, only reorders it.
    for package in packages {
        install_order_visit(package, &by_name, &mut visited, &mut order);
    }
    order
}

/// One step of `install_order`'s DFS: a package's own postorder visit,
/// pushing its still-unvisited `require` first, last-declared first (the
/// stack Composer's algorithm pops from is LIFO).
fn install_order_visit<'a>(
    package: &'a Package,
    by_name: &HashMap<&str, &'a Package>,
    visited: &mut HashSet<&'a str>,
    order: &mut Vec<&'a Package>,
) {
    if !visited.insert(&package.name) {
        return;
    }
    for requirement in package.require.keys().rev() {
        if let Some(&dep) = by_name.get(requirement.as_str()) {
            install_order_visit(dep, by_name, visited, order);
        }
    }
    order.push(package);
}

/// Composer plugins vivace applies the effect of natively.
const NATIVE_ADAPTERS: &[&str] = &[
    "composer/installers",
    "johnpbloch/wordpress-core-installer",
    "roots/wordpress-core-installer",
    "dealerdirect/phpcodesniffer-composer-installer",
    "phpstan/extension-installer",
    "tbachert/spi",
    "cweagans/composer-patches",
    "php-http/discovery",
    "yiisoft/yii2-composer",
    "craftcms/plugin-installer",
    "ffraenz/private-composer-installer",
    "codeception/c3",
    "drupal/core-composer-scaffold",
    "symfony/runtime",
];

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
/// lock and the root `composer.json` ([`resolve`]).
#[expect(
    clippy::struct_excessive_bools,
    reason = "one flag per native adapter, not a state machine"
)]
#[derive(Debug, Default)]
pub struct Plugins {
    installers: bool,
    wordpress_core: bool,
    phpcs: bool,
    phpstan: bool,
    spi: bool,
    patches: bool,
    discovery: bool,
    yii2: bool,
    craft: bool,
    c3: bool,
    private_installer: bool,
    drupal_scaffold: bool,
    symfony_runtime: bool,
}

/// Resolve which native adapters apply and check every other enabled
/// `composer-plugin` against the known-inert/refuse rule
/// (`docs/plugin-strategy.md`'s rule 3). `no_plugins` (`--no-plugins`)
/// disables both adapters, installing every package under `vendor/` as
/// Composer would with the same flag, and downgrades a refusal to a warning
/// line the caller should print.
pub fn resolve(lock: &Lock, root: &Root, no_plugins: bool) -> Result<(Plugins, Vec<String>)> {
    let allow = &root.config.allow_plugins;
    let mut plugins = Plugins::default();
    let mut warnings = Vec::new();
    for package in &lock.packages {
        if package.r#type != "composer-plugin" || !allow.is_enabled(&package.name) {
            continue;
        }
        if KNOWN_INERT.contains(&package.name.as_str()) {
            continue;
        }
        if NATIVE_ADAPTERS.contains(&package.name.as_str()) {
            if !no_plugins {
                match package.name.as_str() {
                    "composer/installers" => plugins.installers = true,
                    "johnpbloch/wordpress-core-installer" | "roots/wordpress-core-installer" => {
                        plugins.wordpress_core = true;
                    }
                    "dealerdirect/phpcodesniffer-composer-installer" => plugins.phpcs = true,
                    "phpstan/extension-installer" => plugins.phpstan = true,
                    "cweagans/composer-patches" => plugins.patches = true,
                    "tbachert/spi" => plugins.spi = true,
                    "php-http/discovery" => plugins.discovery = true,
                    "yiisoft/yii2-composer" => plugins.yii2 = true,
                    "craftcms/plugin-installer" => plugins.craft = true,
                    "ffraenz/private-composer-installer" => plugins.private_installer = true,
                    "codeception/c3" => plugins.c3 = true,
                    "drupal/core-composer-scaffold" => plugins.drupal_scaffold = true,
                    "symfony/runtime" => plugins.symfony_runtime = true,
                    other => unreachable!("{other} is in NATIVE_ADAPTERS but has no adapter arm"),
                }
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
    Ok((plugins, warnings))
}

impl Plugins {
    /// The project-root-relative install directory `package` maps to (no
    /// leading/trailing slash), or `None` to keep the default `vendor/<name>`.
    pub fn install_dir(&self, root: &Root, package: &Package) -> Option<String> {
        if self.wordpress_core && package.r#type == "wordpress-core" {
            return Some(wordpress_install_dir(root, package));
        }
        if self.installers {
            return installer_path(root, package);
        }
        None
    }

    /// `tbachert/spi`'s and `php-http/discovery`'s `PRE_AUTOLOAD_DUMP`
    /// listeners: run from both `install` and `dump-autoload`, since both
    /// regenerate the autoloader.
    pub fn apply_pre_autoload_dump(
        &self,
        root: &Root,
        vendor_dir: &std::path::Path,
        packages: &[(&Package, std::path::PathBuf)],
    ) -> Result<()> {
        if self.spi {
            spi::apply(root, vendor_dir, packages)?;
        }
        if self.discovery {
            discovery::apply(root, vendor_dir)?;
        }
        if self.yii2 {
            yii2::apply(vendor_dir, packages)?;
        }
        if self.craft {
            // ponytail: no `project_dir` reaches this call
            // (`src/install.rs`'s own call site only threads `root`/
            // `vendor_dir` through); the project root is approximated as
            // `vendor_dir`'s parent, right for every default `vendor/`
            // layout. Wrong only for a `config.vendor-dir` that isn't a
            // direct child of the project root — thread the real
            // `project_dir` through here if that ever surfaces.
            let project_dir = vendor_dir.parent().unwrap_or(vendor_dir);
            craft::apply(project_dir, vendor_dir, packages)?;
        }
        if self.c3 {
            // Same `project_dir` approximation as `craft`'s own call above.
            let project_dir = vendor_dir.parent().unwrap_or(vendor_dir);
            c3::apply(project_dir, packages)?;
        }
        Ok(())
    }

    /// The `POST_INSTALL_CMD`-only adapters (`dealerdirect/phpcodesniffer-composer-installer`,
    /// `phpstan/extension-installer`): both subscribe to `post-install-cmd`/
    /// `post-update-cmd` only, never to a bare `dump-autoload`, so this is
    /// called from `install::run` alone, not `install::dump_autoload`.
    pub fn apply_post_install(
        &self,
        root: &Root,
        packages: &[(&Package, std::path::PathBuf)],
    ) -> Result<()> {
        if self.phpcs {
            phpcs::apply(root, packages)?;
        }
        if self.phpstan {
            phpstan::apply(root, packages)?;
        }
        Ok(())
    }

    /// Whether `cweagans/composer-patches` is active for this install.
    pub fn has_patches(&self) -> bool {
        self.patches
    }

    /// Whether `ffraenz/private-composer-installer` is active for this
    /// install. Unlike every other adapter above, this one has no apply
    /// phase of its own: its whole effect is `fetch::Fetcher` resolving a
    /// dist URL's `{%NAME}` placeholders right before the download request,
    /// via `Fetcher::private_installer(private_installer::Env::load(root,
    /// project_dir))`. That builder call has to happen where the `Fetcher`
    /// for an install is constructed (`install.rs`), which this module
    /// doesn't own; this accessor is the seam a caller there checks first.
    pub fn has_private_installer(&self) -> bool {
        self.private_installer
    }

    /// `cweagans/composer-patches`' `POST_PACKAGE_INSTALL`/`POST_PACKAGE_UPDATE`
    /// listener: applies patches to every package that has one, right after
    /// `link_archives` and before the autoloader is (re)generated. `newly_linked`
    /// are the packages `link_archives` just wrote (pristine, unpatched);
    /// `kept` are the ones this run left alone, which may already carry a
    /// previous run's patches.
    pub fn apply_patches(
        &self,
        root: &Root,
        project_dir: &std::path::Path,
        vendor_dir: &std::path::Path,
        newly_linked: &[Package],
        kept: &[Package],
        store: &crate::store::Store,
    ) -> Result<()> {
        if self.patches {
            patches::apply(root, project_dir, vendor_dir, newly_linked, kept, store)?;
        }
        Ok(())
    }

    /// `drupal/core-composer-scaffold`'s `Handler::scaffold`
    /// (`POST_INSTALL_CMD`/`POST_UPDATE_CMD`): copies scaffold files from
    /// every allowed package into the project. Same call site and phase as
    /// [`Self::apply_patches`] (right after linking, while install
    /// directories are known) — `packages` is the same `keep`/`install`
    /// union `regenerate_vendor_metadata` already builds.
    pub fn apply_scaffold(
        &self,
        root: &Root,
        project_dir: &std::path::Path,
        vendor_dir: &std::path::Path,
        packages: &[&Package],
    ) -> Result<()> {
        if self.drupal_scaffold {
            drupal_scaffold::apply(root, project_dir, vendor_dir, packages)?;
        }
        Ok(())
    }

    /// `drupal/core-composer-scaffold`'s `Plugin::preAutoloadDump`: writes
    /// `vendor/drupal/DrupalInstalled.php` and returns the classmap entries
    /// (that file, plus a handful of framework classes conditional on their
    /// package being installed) the caller merges into the root package's
    /// autoload before the classmap scan runs. `packages` here is the full
    /// installed set including metapackages — unlike
    /// [`Self::apply_pre_autoload_dump`]'s `bin_packages`, since the real
    /// plugin's version hash is computed over Composer's own local
    /// repository, which carries metapackages too.
    pub fn scaffold_classmap(
        &self,
        root: &Root,
        project_dir: &std::path::Path,
        vendor_dir: &std::path::Path,
        packages: &[&Package],
    ) -> Result<Vec<String>> {
        if self.drupal_scaffold {
            drupal_scaffold::pre_autoload_dump(root, project_dir, vendor_dir, packages)
        } else {
            Ok(Vec::new())
        }
    }

    /// `symfony/runtime`'s `ComposerPlugin::updateAutoloadFile`
    /// (`POST_AUTOLOAD_DUMP`): writes `vendor/autoload_runtime.php`. Called
    /// after `install::write_autoload` dispatches that script event, the
    /// same phase the real plugin subscribes to.
    pub fn apply_post_autoload_dump(
        &self,
        root: &Root,
        project_dir: &std::path::Path,
        vendor_dir: &std::path::Path,
    ) -> Result<()> {
        if self.symfony_runtime {
            symfony_runtime::apply(root, project_dir, vendor_dir)?;
        }
        Ok(())
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

/// `composer/installers`' `BaseInstaller::getInstallPath`: the root's
/// `extra.installer-paths` (first pattern whose value list contains the
/// package's name, `type:<type>` or `vendor:<vendor>` wins), else the
/// package type's own default location from [`INSTALLER_TYPES`].
fn installer_path(root: &Root, package: &Package) -> Option<String> {
    let (vendor, name) = package
        .name
        .split_once('/')
        .unwrap_or(("", package.name.as_str()));
    let name = package
        .raw
        .pointer("/extra/installer-name")
        .and_then(Value::as_str)
        .unwrap_or(name);
    let vars = [
        ("name", name),
        ("vendor", vendor),
        ("type", package.r#type.as_str()),
    ];

    if let Some(paths) = root.extra.get("installer-paths").and_then(Value::as_object) {
        for (pattern, names) in paths {
            let names = string_or_vec(names);
            // Composer matches the *pretty* package name here
            // (`vendor/name`), not the `{$name}` template var above, which
            // may already be the `extra.installer-name` override.
            let hit = names.iter().any(|n| {
                n == &package.name
                    || n == &format!("type:{}", package.r#type)
                    || n == &format!("vendor:{vendor}")
            });
            if hit {
                return Some(template_path(pattern, &vars));
            }
        }
    }

    let (framework_type, locations) = INSTALLER_TYPES
        .iter()
        .filter(|(key, _)| {
            package.r#type.starts_with(key)
                && package.r#type.as_bytes().get(key.len()) == Some(&b'-')
        })
        .max_by_key(|(key, _)| key.len())?;
    let package_type = &package.r#type[framework_type.len() + 1..];
    let location = locations.iter().find(|(t, _)| *t == package_type)?.1;
    Some(template_path(location, &vars))
}

/// `extra.installer-paths`' value: a single string or an array of strings.
fn string_or_vec(value: &Value) -> Vec<String> {
    match value {
        Value::String(s) => vec![s.clone()],
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// `BaseInstaller::templatePath`: replace every `{$var}` with `vars`' value
/// for that name; a var this package type doesn't supply (a CMS-specific
/// one like `{$bitrix_dir}`) is left untouched, same as Composer's own
/// `extract`-based substitution would leave an undefined variable empty —
/// vivace only ever exercises the `WordPress` installers' vars (`name`,
/// `vendor`, `type`), so the difference never surfaces in practice.
fn template_path(path: &str, vars: &[(&str, &str)]) -> String {
    let mut path = path.to_string();
    for (name, value) in vars {
        path = path.replace(&format!("{{${name}}}"), value);
    }
    path
}

/// `composer/installers`' package-type default location table, ported
/// verbatim from every `Composer\Installers\*Installer::$locations` array
/// (composer/installers `main`, fetched 2026-09-06). Keyed by the "framework
/// type" — the part of a package's `type` before its first `-`, e.g.
/// `wordpress` in `wordpress-plugin` — each value list is `(package-type
/// suffix, default path)`. `ee2`/`ee3` (`ExpressionEngineInstaller`, whose
/// two locations tables are chosen by constructor argument rather than a
/// static property) are inlined here as if they were static too.
static INSTALLER_TYPES: &[(&str, &[(&str, &str)])] = &[
    ("agl", &[("module", "More/{$name}/")]),
    ("akaunting", &[("module", "modules/{$name}")]),
    (
        "annotatecms",
        &[
            ("module", "addons/modules/{$name}/"),
            ("component", "addons/components/{$name}/"),
            ("service", "addons/services/{$name}/"),
        ],
    ),
    (
        "asgard",
        &[("module", "Modules/{$name}/"), ("theme", "Themes/{$name}/")],
    ),
    ("attogram", &[("module", "modules/{$name}/")]),
    (
        "bitrix",
        &[
            ("module", "{$bitrix_dir}/modules/{$name}/"),
            ("component", "{$bitrix_dir}/components/{$name}/"),
            ("theme", "{$bitrix_dir}/templates/{$name}/"),
            ("d7-module", "{$bitrix_dir}/modules/{$vendor}.{$name}/"),
            (
                "d7-component",
                "{$bitrix_dir}/components/{$vendor}/{$name}/",
            ),
            ("d7-template", "{$bitrix_dir}/templates/{$vendor}_{$name}/"),
        ],
    ),
    ("bonefish", &[("package", "Packages/{$vendor}/{$name}/")]),
    (
        "botble",
        &[
            ("plugin", "platform/plugins/{$name}/"),
            ("theme", "platform/themes/{$name}/"),
        ],
    ),
    ("cakephp", &[("plugin", "Plugin/{$name}/")]),
    (
        "ccframework",
        &[
            ("ship", "CCF/orbit/{$name}/"),
            ("theme", "CCF/app/themes/{$name}/"),
        ],
    ),
    (
        "chef",
        &[
            ("cookbook", "Chef/{$vendor}/{$name}/"),
            ("role", "Chef/roles/{$name}/"),
        ],
    ),
    ("civicrm", &[("ext", "ext/{$name}/")]),
    ("cockpit", &[("module", "cockpit/modules/addons/{$name}/")]),
    (
        "codeigniter",
        &[
            ("library", "application/libraries/{$name}/"),
            ("third-party", "application/third_party/{$name}/"),
            ("module", "application/modules/{$name}/"),
        ],
    ),
    (
        "concrete5",
        &[
            ("core", "concrete/"),
            ("block", "application/blocks/{$name}/"),
            ("package", "packages/{$name}/"),
            ("theme", "application/themes/{$name}/"),
            ("update", "updates/{$name}/"),
        ],
    ),
    (
        "concretecms",
        &[
            ("core", "concrete/"),
            ("block", "application/blocks/{$name}/"),
            ("package", "packages/{$name}/"),
            ("theme", "application/themes/{$name}/"),
            ("update", "updates/{$name}/"),
        ],
    ),
    (
        "croogo",
        &[
            ("plugin", "Plugin/{$name}/"),
            ("theme", "View/Themed/{$name}/"),
        ],
    ),
    ("decibel", &[("app", "app/{$name}/")]),
    ("dframe", &[("module", "modules/{$vendor}/{$name}/")]),
    (
        "dokuwiki",
        &[
            ("plugin", "lib/plugins/{$name}/"),
            ("template", "lib/tpl/{$name}/"),
        ],
    ),
    ("dolibarr", &[("module", "htdocs/custom/{$name}/")]),
    (
        "drupal",
        &[
            ("core", "core/"),
            ("module", "modules/{$name}/"),
            ("theme", "themes/{$name}/"),
            ("library", "libraries/{$name}/"),
            ("profile", "profiles/{$name}/"),
            (
                "database-driver",
                "drivers/lib/Drupal/Driver/Database/{$name}/",
            ),
            ("drush", "drush/{$name}/"),
            ("custom-theme", "themes/custom/{$name}/"),
            ("custom-module", "modules/custom/{$name}/"),
            ("custom-profile", "profiles/custom/{$name}/"),
            ("drupal-multisite", "sites/{$name}/"),
            ("console", "console/{$name}/"),
            ("console-language", "console/language/{$name}/"),
            ("config", "config/sync/"),
            ("recipe", "recipes/{$name}"),
        ],
    ),
    (
        "ee2",
        &[
            ("addon", "system/expressionengine/third_party/{$name}/"),
            ("theme", "themes/third_party/{$name}/"),
        ],
    ),
    (
        "ee3",
        &[
            ("addon", "system/user/addons/{$name}/"),
            ("theme", "themes/user/{$name}/"),
        ],
    ),
    ("elgg", &[("plugin", "mod/{$name}/")]),
    (
        "eliasis",
        &[
            ("component", "components/{$name}/"),
            ("module", "modules/{$name}/"),
            ("plugin", "plugins/{$name}/"),
            ("template", "templates/{$name}/"),
        ],
    ),
    (
        "ezplatform",
        &[
            ("meta-assets", "web/assets/ezplatform/"),
            ("assets", "web/assets/ezplatform/{$name}/"),
        ],
    ),
    (
        "fork",
        &[
            ("module", "src/Modules/{$name}/"),
            ("theme", "src/Themes/{$name}/"),
        ],
    ),
    (
        "fuel",
        &[
            ("module", "fuel/app/modules/{$name}/"),
            ("package", "fuel/packages/{$name}/"),
            ("theme", "fuel/app/themes/{$name}/"),
        ],
    ),
    ("fuelphp", &[("component", "components/{$name}/")]),
    (
        "grav",
        &[
            ("plugin", "user/plugins/{$name}/"),
            ("theme", "user/themes/{$name}/"),
        ],
    ),
    (
        "hurad",
        &[
            ("plugin", "plugins/{$name}/"),
            ("theme", "plugins/{$name}/"),
        ],
    ),
    (
        "imagecms",
        &[
            ("template", "templates/{$name}/"),
            ("module", "application/modules/{$name}/"),
            ("library", "application/libraries/{$name}/"),
        ],
    ),
    ("itop", &[("extension", "extensions/{$name}/")]),
    ("kanboard", &[("plugin", "plugins/{$name}/")]),
    (
        "known",
        &[
            ("plugin", "IdnoPlugins/{$name}/"),
            ("theme", "Themes/{$name}/"),
            ("console", "ConsolePlugins/{$name}/"),
        ],
    ),
    (
        "kodicms",
        &[
            ("plugin", "cms/plugins/{$name}/"),
            ("media", "cms/media/vendor/{$name}/"),
        ],
    ),
    ("kohana", &[("module", "modules/{$name}/")]),
    ("laravel", &[("library", "libraries/{$name}/")]),
    (
        "lavalite",
        &[
            ("package", "packages/{$vendor}/{$name}/"),
            ("theme", "public/themes/{$name}/"),
        ],
    ),
    (
        "lithium",
        &[
            ("library", "libraries/{$name}/"),
            ("source", "libraries/_source/{$name}/"),
        ],
    ),
    (
        "lms",
        &[
            ("plugin", "plugins/{$name}/"),
            ("template", "templates/{$name}/"),
            ("document-template", "documents/templates/{$name}/"),
            ("userpanel-module", "userpanel/modules/{$name}/"),
        ],
    ),
    (
        "magento",
        &[
            ("theme", "app/design/frontend/{$name}/"),
            ("skin", "skin/frontend/default/{$name}/"),
            ("library", "lib/{$name}/"),
        ],
    ),
    ("majima", &[("plugin", "plugins/{$name}/")]),
    ("mako", &[("package", "app/packages/{$name}/")]),
    ("mantisbt", &[("plugin", "plugins/{$name}/")]),
    ("matomo", &[("plugin", "plugins/{$name}/")]),
    (
        "mautic",
        &[
            ("plugin", "plugins/{$name}/"),
            ("theme", "themes/{$name}/"),
            ("core", "app/"),
        ],
    ),
    ("maya", &[("module", "modules/{$name}/")]),
    (
        "mediawiki",
        &[
            ("core", "core/"),
            ("extension", "extensions/{$name}/"),
            ("skin", "skins/{$name}/"),
        ],
    ),
    ("miaoxing", &[("plugin", "plugins/{$name}/")]),
    (
        "microweber",
        &[
            ("module", "userfiles/modules/{$install_item_dir}/"),
            (
                "module-skin",
                "userfiles/modules/{$install_item_dir}/templates/",
            ),
            ("template", "userfiles/templates/{$install_item_dir}/"),
            ("element", "userfiles/elements/{$install_item_dir}/"),
            ("vendor", "vendor/{$install_item_dir}/"),
            ("components", "components/{$install_item_dir}/"),
        ],
    ),
    ("modulework", &[("module", "modules/{$name}/")]),
    ("modx", &[("extra", "core/packages/{$name}/")]),
    (
        "modxevo",
        &[
            ("snippet", "assets/snippets/{$name}/"),
            ("plugin", "assets/plugins/{$name}/"),
            ("module", "assets/modules/{$name}/"),
            ("template", "assets/templates/{$name}/"),
            ("lib", "assets/lib/{$name}/"),
        ],
    ),
    (
        "moodle",
        &[
            ("mod", "mod/{$name}/"),
            ("admin_report", "admin/report/{$name}/"),
            ("atto", "lib/editor/atto/plugins/{$name}/"),
            ("tool", "admin/tool/{$name}/"),
            ("assignment", "mod/assignment/type/{$name}/"),
            ("assignsubmission", "mod/assign/submission/{$name}/"),
            ("assignfeedback", "mod/assign/feedback/{$name}/"),
            ("antivirus", "lib/antivirus/{$name}/"),
            ("auth", "auth/{$name}/"),
            ("availability", "availability/condition/{$name}/"),
            ("block", "blocks/{$name}/"),
            ("booktool", "mod/book/tool/{$name}/"),
            ("cachestore", "cache/stores/{$name}/"),
            ("cachelock", "cache/locks/{$name}/"),
            ("calendartype", "calendar/type/{$name}/"),
            ("communication", "communication/provider/{$name}/"),
            ("customfield", "customfield/field/{$name}/"),
            ("fileconverter", "files/converter/{$name}/"),
            ("format", "course/format/{$name}/"),
            ("coursereport", "course/report/{$name}/"),
            ("contenttype", "contentbank/contenttype/{$name}/"),
            ("customcertelement", "mod/customcert/element/{$name}/"),
            ("datafield", "mod/data/field/{$name}/"),
            ("dataformat", "dataformat/{$name}/"),
            ("datapreset", "mod/data/preset/{$name}/"),
            ("editor", "lib/editor/{$name}/"),
            ("enrol", "enrol/{$name}/"),
            ("filter", "filter/{$name}/"),
            ("forumreport", "mod/forum/report/{$name}/"),
            ("gradeexport", "grade/export/{$name}/"),
            ("gradeimport", "grade/import/{$name}/"),
            ("gradereport", "grade/report/{$name}/"),
            ("gradingform", "grade/grading/form/{$name}/"),
            ("h5plib", "h5p/h5plib/{$name}/"),
            ("local", "local/{$name}/"),
            ("logstore", "admin/tool/log/store/{$name}/"),
            ("ltisource", "mod/lti/source/{$name}/"),
            ("ltiservice", "mod/lti/service/{$name}/"),
            ("media", "media/player/{$name}/"),
            ("message", "message/output/{$name}/"),
            ("mlbackend", "lib/mlbackend/{$name}/"),
            ("mnetservice", "mnet/service/{$name}/"),
            ("paygw", "payment/gateway/{$name}/"),
            ("plagiarism", "plagiarism/{$name}/"),
            ("portfolio", "portfolio/{$name}/"),
            ("qbank", "question/bank/{$name}/"),
            ("qbehaviour", "question/behaviour/{$name}/"),
            ("qformat", "question/format/{$name}/"),
            ("qtype", "question/type/{$name}/"),
            ("quizaccess", "mod/quiz/accessrule/{$name}/"),
            ("quiz", "mod/quiz/report/{$name}/"),
            ("report", "report/{$name}/"),
            ("repository", "repository/{$name}/"),
            ("scormreport", "mod/scorm/report/{$name}/"),
            ("search", "search/engine/{$name}/"),
            ("theme", "theme/{$name}/"),
            ("tiny", "lib/editor/tiny/plugins/{$name}/"),
            ("tinymce", "lib/editor/tinymce/plugins/{$name}/"),
            ("profilefield", "user/profile/field/{$name}/"),
            ("webservice", "webservice/{$name}/"),
            ("workshopallocation", "mod/workshop/allocation/{$name}/"),
            ("workshopeval", "mod/workshop/eval/{$name}/"),
            ("workshopform", "mod/workshop/form/{$name}/"),
        ],
    ),
    (
        "october",
        &[
            ("module", "modules/{$name}/"),
            ("plugin", "plugins/{$vendor}/{$name}/"),
            ("theme", "themes/{$vendor}-{$name}/"),
        ],
    ),
    (
        "ontowiki",
        &[
            ("extension", "extensions/{$name}/"),
            ("theme", "extensions/themes/{$name}/"),
            ("translation", "extensions/translations/{$name}/"),
        ],
    ),
    (
        "osclass",
        &[
            ("plugin", "oc-content/plugins/{$name}/"),
            ("theme", "oc-content/themes/{$name}/"),
            ("language", "oc-content/languages/{$name}/"),
        ],
    ),
    (
        "oxid",
        &[
            ("module", "modules/{$name}/"),
            ("theme", "application/views/{$name}/"),
            ("out", "out/{$name}/"),
        ],
    ),
    (
        "phifty",
        &[
            ("bundle", "bundles/{$name}/"),
            ("library", "libraries/{$name}/"),
            ("framework", "frameworks/{$name}/"),
        ],
    ),
    (
        "phpbb",
        &[
            ("extension", "ext/{$vendor}/{$name}/"),
            ("language", "language/{$name}/"),
            ("style", "styles/{$name}/"),
        ],
    ),
    ("piwik", &[("plugin", "plugins/{$name}/")]),
    ("plentymarkets", &[("plugin", "{$name}/")]),
    ("porto", &[("container", "app/Containers/{$name}/")]),
    ("ppi", &[("module", "modules/{$name}/")]),
    (
        "prestashop",
        &[("module", "modules/{$name}/"), ("theme", "themes/{$name}/")],
    ),
    ("processwire", &[("module", "site/modules/{$name}/")]),
    ("puppet", &[("module", "modules/{$name}/")]),
    (
        "pxcms",
        &[
            ("module", "app/Modules/{$name}/"),
            ("theme", "themes/{$name}/"),
        ],
    ),
    (
        "quicksilver",
        &[
            ("script", "web/private/scripts/quicksilver/{$name}"),
            ("module", "web/private/scripts/quicksilver/{$name}"),
        ],
    ),
    ("radphp", &[("bundle", "src/{$name}/")]),
    (
        "redaxo",
        &[
            ("addon", "redaxo/include/addons/{$name}/"),
            (
                "bestyle-plugin",
                "redaxo/include/addons/be_style/plugins/{$name}/",
            ),
        ],
    ),
    (
        "redaxo5",
        &[
            ("addon", "redaxo/src/addons/{$name}/"),
            (
                "bestyle-plugin",
                "redaxo/src/addons/be_style/plugins/{$name}/",
            ),
        ],
    ),
    (
        "reindex",
        &[("theme", "themes/{$name}/"), ("plugin", "plugins/{$name}/")],
    ),
    ("roundcube", &[("plugin", "plugins/{$name}/")]),
    (
        "shopware",
        &[
            (
                "backend-plugin",
                "engine/Shopware/Plugins/Local/Backend/{$name}/",
            ),
            ("core-plugin", "engine/Shopware/Plugins/Local/Core/{$name}/"),
            (
                "frontend-plugin",
                "engine/Shopware/Plugins/Local/Frontend/{$name}/",
            ),
            ("theme", "templates/{$name}/"),
            ("plugin", "custom/plugins/{$name}/"),
            ("frontend-theme", "themes/Frontend/{$name}/"),
        ],
    ),
    (
        "silverstripe",
        &[("module", "{$name}/"), ("theme", "themes/{$name}/")],
    ),
    (
        "sitedirect",
        &[
            ("module", "modules/{$vendor}/{$name}/"),
            ("plugin", "plugins/{$vendor}/{$name}/"),
        ],
    ),
    (
        "smf",
        &[("module", "Sources/{$name}/"), ("theme", "Themes/{$name}/")],
    ),
    (
        "starbug",
        &[
            ("module", "modules/{$name}/"),
            ("theme", "themes/{$name}/"),
            ("custom-module", "app/modules/{$name}/"),
            ("custom-theme", "app/themes/{$name}/"),
        ],
    ),
    (
        "sydes",
        &[
            ("module", "app/modules/{$name}/"),
            ("theme", "themes/{$name}/"),
        ],
    ),
    ("sylius", &[("theme", "themes/{$name}/")]),
    ("tao", &[("extension", "{$name}")]),
    (
        "tastyigniter",
        &[
            ("module", "app/{$name}/"),
            ("extension", "extensions/{$vendor}/{$name}/"),
            ("theme", "themes/{$name}/"),
        ],
    ),
    (
        "thelia",
        &[
            ("module", "local/modules/{$name}/"),
            ("frontoffice-template", "templates/frontOffice/{$name}/"),
            ("backoffice-template", "templates/backOffice/{$name}/"),
            ("email-template", "templates/email/{$name}/"),
        ],
    ),
    (
        "tusk",
        &[
            ("task", ".tusk/tasks/{$name}/"),
            ("command", ".tusk/commands/{$name}/"),
            ("asset", "assets/tusk/{$name}/"),
        ],
    ),
    ("userfrosting", &[("sprinkle", "app/sprinkles/{$name}/")]),
    (
        "vanilla",
        &[("plugin", "plugins/{$name}/"), ("theme", "themes/{$name}/")],
    ),
    (
        "whmcs",
        &[
            ("addons", "modules/addons/{$vendor}_{$name}/"),
            ("fraud", "modules/fraud/{$vendor}_{$name}/"),
            ("gateways", "modules/gateways/{$vendor}_{$name}/"),
            ("notifications", "modules/notifications/{$vendor}_{$name}/"),
            ("registrars", "modules/registrars/{$vendor}_{$name}/"),
            ("reports", "modules/reports/{$vendor}_{$name}/"),
            ("security", "modules/security/{$vendor}_{$name}/"),
            ("servers", "modules/servers/{$vendor}_{$name}/"),
            ("social", "modules/social/{$vendor}_{$name}/"),
            ("support", "modules/support/{$vendor}_{$name}/"),
            ("templates", "templates/{$vendor}_{$name}/"),
            ("includes", "includes/{$vendor}_{$name}/"),
        ],
    ),
    (
        "winter",
        &[
            ("module", "modules/{$name}/"),
            ("plugin", "plugins/{$vendor}/{$name}/"),
            ("theme", "themes/{$name}/"),
        ],
    ),
    ("wolfcms", &[("plugin", "wolf/plugins/{$name}/")]),
    (
        "wordpress",
        &[
            ("plugin", "wp-content/plugins/{$name}/"),
            ("theme", "wp-content/themes/{$name}/"),
            ("muplugin", "wp-content/mu-plugins/{$name}/"),
            ("dropin", "wp-content/{$name}/"),
        ],
    ),
    ("yawik", &[("module", "module/{$name}/")]),
    (
        "zend",
        &[
            ("library", "library/{$name}/"),
            ("extra", "extras/library/{$name}/"),
            ("module", "module/{$name}/"),
        ],
    ),
    (
        "zikula",
        &[
            ("module", "modules/{$vendor}-{$name}/"),
            ("theme", "themes/{$vendor}-{$name}/"),
        ],
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lock::read_lock;
    use serde_json::json;
    use std::io::Write as _;

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

    #[test]
    fn native_adapter_enabled_by_allow_plugins_activates() {
        let lock = lock_with(&[plugin_package("composer/installers")]);
        let root = root(json!({"config": {"allow-plugins": {"composer/installers": true}}}));
        let (plugins, warnings) = resolve(&lock, &root, false).unwrap();
        assert!(warnings.is_empty());
        assert!(plugins.installers);
    }

    #[test]
    fn native_adapter_not_allowed_stays_inactive_and_does_not_error() {
        let lock = lock_with(&[plugin_package("composer/installers")]);
        let root = root(json!({}));
        let (plugins, warnings) = resolve(&lock, &root, false).unwrap();
        assert!(warnings.is_empty());
        assert!(!plugins.installers);
    }

    #[test]
    fn known_inert_plugin_is_ignored() {
        let lock = lock_with(&[plugin_package("ergebnis/composer-normalize")]);
        let root =
            root(json!({"config": {"allow-plugins": {"ergebnis/composer-normalize": true}}}));
        let (plugins, warnings) = resolve(&lock, &root, false).unwrap();
        assert!(warnings.is_empty());
        assert!(!plugins.installers && !plugins.wordpress_core);
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
        assert!(!plugins.installers && !plugins.wordpress_core);
    }

    #[test]
    fn allow_plugins_wildcard_matches() {
        let lock = lock_with(&[plugin_package("composer/installers")]);
        let root = root(json!({"config": {"allow-plugins": {"composer/*": true}}}));
        let (plugins, _) = resolve(&lock, &root, false).unwrap();
        assert!(plugins.installers);
    }

    #[test]
    fn allow_plugins_true_enables_everything() {
        let lock = lock_with(&[plugin_package("composer/installers")]);
        let root = root(json!({"config": {"allow-plugins": true}}));
        let (plugins, _) = resolve(&lock, &root, false).unwrap();
        assert!(plugins.installers);
    }

    #[test]
    fn composer_patches_enabled_by_allow_plugins_activates() {
        let lock = lock_with(&[plugin_package("cweagans/composer-patches")]);
        let root = root(json!({"config": {"allow-plugins": {"cweagans/composer-patches": true}}}));
        let (plugins, warnings) = resolve(&lock, &root, false).unwrap();
        assert!(warnings.is_empty());
        assert!(plugins.has_patches());
    }

    fn package(name: &str, r#type: &str) -> Package {
        let raw = json!({ "name": name, "version": "1.0.0", "type": r#type });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        package
    }

    #[test]
    fn installer_type_default_location() {
        let plugins = Plugins {
            installers: true,
            wordpress_core: false,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let root = root(json!({}));
        let dir = plugins
            .install_dir(&root, &package("acme/hello", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "wp-content/plugins/hello/");
    }

    #[test]
    fn installer_paths_type_match_wins_over_default() {
        let plugins = Plugins {
            installers: true,
            wordpress_core: false,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let root = root(json!({
            "extra": {
                "installer-paths": {
                    "custom/{$name}/": ["type:wordpress-plugin"]
                }
            }
        }));
        let dir = plugins
            .install_dir(&root, &package("acme/hello", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "custom/hello/");
    }

    #[test]
    fn installer_paths_exact_name_beats_vendor_and_type() {
        let plugins = Plugins {
            installers: true,
            wordpress_core: false,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let root = root(json!({
            "extra": {
                "installer-paths": {
                    "by-name/{$name}/": ["acme/hello"],
                    "by-vendor/{$name}/": ["vendor:acme"],
                    "by-type/{$name}/": ["type:wordpress-plugin"]
                }
            }
        }));
        let dir = plugins
            .install_dir(&root, &package("acme/hello", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "by-name/hello/");
    }

    #[test]
    fn installer_paths_first_map_entry_wins_when_several_match() {
        let plugins = Plugins {
            installers: true,
            wordpress_core: false,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let root = root(json!({
            "extra": {
                "installer-paths": {
                    "first/{$name}/": ["vendor:acme"],
                    "second/{$name}/": ["type:wordpress-plugin"]
                }
            }
        }));
        let dir = plugins
            .install_dir(&root, &package("acme/hello", "wordpress-plugin"))
            .unwrap();
        assert_eq!(dir, "first/hello/");
    }

    #[test]
    fn unsupported_type_falls_back_to_default_vendor_placement() {
        let plugins = Plugins {
            installers: true,
            wordpress_core: false,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let root = root(json!({}));
        assert!(
            plugins
                .install_dir(&root, &package("acme/hello", "library"))
                .is_none()
        );
    }

    #[test]
    fn wordpress_core_default_dir() {
        let plugins = Plugins {
            installers: false,
            wordpress_core: true,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let root = root(json!({}));
        let dir = plugins
            .install_dir(
                &root,
                &package("johnpbloch/wordpress-core", "wordpress-core"),
            )
            .unwrap();
        assert_eq!(dir, "wordpress");
    }

    #[test]
    fn wordpress_core_install_dir_from_root_extra_string() {
        let plugins = Plugins {
            installers: false,
            wordpress_core: true,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let root = root(json!({"extra": {"wordpress-install-dir": "wp"}}));
        let dir = plugins
            .install_dir(
                &root,
                &package("johnpbloch/wordpress-core", "wordpress-core"),
            )
            .unwrap();
        assert_eq!(dir, "wp");
    }

    #[test]
    fn wordpress_core_install_dir_from_root_extra_map_by_package_name() {
        let plugins = Plugins {
            installers: false,
            wordpress_core: true,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let root = root(json!({
            "extra": {"wordpress-install-dir": {"johnpbloch/wordpress-core": "web/wp"}}
        }));
        let dir = plugins
            .install_dir(
                &root,
                &package("johnpbloch/wordpress-core", "wordpress-core"),
            )
            .unwrap();
        assert_eq!(dir, "web/wp");
    }

    #[test]
    fn wordpress_core_install_dir_falls_back_to_package_extra() {
        let plugins = Plugins {
            installers: false,
            wordpress_core: true,
            phpcs: false,
            phpstan: false,
            spi: false,
            patches: false,
            discovery: false,
            yii2: false,
            craft: false,
            c3: false,
            private_installer: false,
            drupal_scaffold: false,
            symfony_runtime: false,
        };
        let raw = json!({
            "name": "acme/wp",
            "version": "1.0.0",
            "type": "wordpress-core",
            "extra": {"wordpress-install-dir": "own-dir"}
        });
        let mut package: Package = serde_json::from_value(raw.clone()).unwrap();
        package.raw = raw;
        let root = root(json!({}));
        assert_eq!(plugins.install_dir(&root, &package).unwrap(), "own-dir");
    }
}
