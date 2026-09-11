//! `composer/installers`' `BaseInstaller::getInstallPath`: maps a package to
//! the root's `extra.installer-paths` (first pattern whose value list
//! contains the package's name, `type:<type>` or `vendor:<vendor>` wins), or
//! its package type's own default location from `INSTALLER_TYPES`. Version
//! pinned in `tests/fixtures/wordpress/composer.lock`.

use serde_json::Value;

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
        installer_path(root, package)
    }
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
