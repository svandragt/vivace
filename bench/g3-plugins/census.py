#!/usr/bin/env python3
r"""Candidate 3.4 (#332), the measurement: what do Composer plugins do at
install, and how many corpus projects run any code (a plugin or a script) at
`viv install` today.

Part 1: the 40 most-downloaded `type: composer-plugin` packages on
Packagist -- fetched live (`search.json`, paged until >=300 results, ranked
by `downloads`, top 40 kept), then per package: latest stable version,
`source.url` and `extra.class` from `repo.packagist.org/p2/<name>.json`, a
shallow clone of that source at that version's tag into a scratch dir
(skipped and recorded on failure), and a classification of what the plugin's
class does at install. The classification itself (category, `file:line`
evidence, one-sentence reason, `declarable: yes|partly|no|n/a`, whether one
of viv's own `src/plugins/*.rs` adapters already covers it) is a
hand-curated table below, `CLASSIFICATION`, read from each plugin's own
class file (and, for six of them, one file it delegates to) at the version
named in the table -- not recomputed at runtime, the same technique
`bench/g3-rules/features.py`'s Part 2 uses for its closed-issue
classification. `CLASSIFICATION` is keyed by package name: a rerun whose
top-40 ranking has shifted since the table was written (2026-09-28) reports
any newly-arrived package as "not classified since census" rather than
guessing.

Part 2: static, no installs -- reuses `compat/platform-drift.py`'s CORPUS,
NO_CHECKOUT, resolve_repo_candidates, clone_at and analyse (loaded by path,
a hyphenated filename can't be `import`ed -- the same technique
`bench/g3-rules/features.py` and `bench/g3-toolchain/static.py` use), the
same project selection those scripts already made, narrowed to the ones
that commit a composer.lock. For each: every locked package (packages +
packages-dev) whose own `type` is `composer-plugin` (name and version),
whether root `config.allow-plugins` allows any package, and which of the
nine script events (`pre-install-cmd`, `post-install-cmd`, `pre-update-cmd`,
`post-update-cmd`, `post-autoload-dump`, `pre-autoload-dump`,
`post-package-install`, `post-root-package-install`,
`post-create-project-cmd`) root `scripts` defines, classifying each
handler string as `@php` (starts with `@php `), a static PHP callback
(`Vendor\Class::method`, no shell metacharacters) or a shell command
(anything else).

What is assumed: `declarable` is this census's own judgement call, not a
fact Composer records anywhere -- marked `unsure` rather than guessed where
the plugin's own effect is genuinely mixed. A locked package's `type` is
read as committed; a plugin installed under an alias or a fork keeps its
own package name, so a lock that pins a fork under the same name still
counts, and one renamed away from the Packagist name in `CLASSIFICATION`
would not match it (none of the corpus does this for any of the 40).

Env:
  G3_PLUGINS_SCRATCH   scratch dir for clones, default a mktemp -d
  G3_PLUGINS_ONLY      comma-separated corpus project names, Part 2 only,
                        skip the rest

Output: a Markdown report on stdout.

Usage (from a clean checkout, needs network: Packagist + GitHub):
    python3 bench/g3-plugins/census.py
"""
from __future__ import annotations

import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent

_spec = importlib.util.spec_from_file_location("platform_drift", REPO_ROOT / "compat" / "platform-drift.py")
platform_drift = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
sys.modules["platform_drift"] = platform_drift
_spec.loader.exec_module(platform_drift)

UA = {"User-Agent": "vivace-g3-plugins-census"}

# --- viv's own adapters (src/plugins/*.rs), by the Packagist name(s) each
# adapter's `plugin_names()` stands in for -- read directly from the source,
# not guessed. ------------------------------------------------------------

VIV_ADAPTERS = {
    "composer/installers": "installers.rs",
    "johnpbloch/wordpress-core-installer": "wordpress_core.rs",
    "roots/wordpress-core-installer": "wordpress_core.rs",
    "drupal/core-composer-scaffold": "drupal_scaffold.rs",
    "cweagans/composer-patches": "patches.rs",
    "php-http/discovery": "discovery.rs",
    "symfony/runtime": "symfony_runtime.rs",
    "craftcms/plugin-installer": "craft.rs",
    "yiisoft/yii2-composer": "yii2.rs",
    "pestphp/pest-plugin": "pest.rs",
    "dealerdirect/phpcodesniffer-composer-installer": "phpcs.rs",
    "phpstan/extension-installer": "phpstan.rs",
    "tbachert/spi": "spi.rs",
    "codeception/c3": "c3.rs",
    "ffraenz/private-composer-installer": "private_installer.rs",
}

# --- Part 1: hand-curated classification, one row per package, read from
# the clone at the version named here (fetched/cloned fresh below; this
# table records what the census author read on 2026-09-28) ---------------
# category: path-mapping | scaffolding | patching | code-generation |
#           download-or-auth | check-or-audit | command-only | other
# declarable: yes | partly | no | n/a (n/a: no install-time disk effect at
#             all, so there's nothing to declare)

CLASSIFICATION: dict[str, dict] = {
    "php-http/discovery": dict(
        version="1.20.0", category="code-generation", secondary=None,
        evidence="src/Composer/Plugin.php:369-403 (preAutoloadDump writes vendor/composer/GeneratedDiscoveryStrategy.php)",
        note="Writes a generated PHP switch mapping each pinned `*-implementation` interface to its chosen class; a second listener (postUpdate) also edits composer.json to add the missing implementation package, out of scope for viv's own port too.",
        declarable="yes", reason="the pinned interface->class map is exactly `extra.discovery`, already data; viv's own discovery.rs proves the generated file is derivable from it.",
    ),
    "symfony/flex": dict(
        version="v2.11.0", category="scaffolding", secondary="download-or-auth",
        evidence="src/Flex.php:400-430 (install() calls fetchRecipes()/Configurator to apply a fetched recipe)",
        note="Fetches a \"recipe\" (files + config edits) per newly required package from symfony/recipes (or a private endpoint) and applies it via a Configurator that can create files, edit .env/services.yaml/bundles.php, and add composer scripts.",
        declarable="partly", reason="the recipe manifest itself (files + config directives) is already close to data, but recipes are fetched from a remote server at install time and a recipe can carry an arbitrary post-install hook.",
    ),
    "composer/package-versions-deprecated": dict(
        version="1.11.99.5", category="code-generation", secondary=None,
        evidence="src/PackageVersions/Installer.php:36-108 (generated class template with a VERSIONS const, written on post-install/update)",
        note="Writes a generated PHP class whose VERSIONS const maps every installed package to its resolved version string; deprecated upstream in favour of Composer 2's own InstalledVersions.",
        declarable="yes", reason="the VERSIONS map is derived entirely from the lock -- Composer 2 already generates the same information as data (installed.php).",
    ),
    "dealerdirect/phpcodesniffer-composer-installer": dict(
        version="v1.2.1", category="code-generation", secondary=None,
        evidence="src/Plugin.php:263-297 (saveInstalledPaths shells out to `phpcs --config-set installed_paths ...`)",
        note="Collects every installed PHP_CodeSniffer coding-standard package's path and writes them into phpcs's own CodeSniffer.conf via `phpcs --config-set`.",
        declarable="yes", reason="the installed_paths value is just a sorted list of package install paths; viv's own phpcs.rs writes the same config without shelling out.",
    ),
    "composer/installers": dict(
        version="v2.3.0", category="path-mapping", secondary=None,
        evidence="src/Composer/Installers/Plugin.php:14-18 (activate() registers a custom Installer via addInstaller)",
        note="Registers an Installer that maps ~200 known package `type` values (wordpress-plugin, drupal-module, ...) to an install path template.",
        declarable="yes", reason="the type->path template map is a static table; viv's own installers.rs is that table as Rust data.",
    ),
    "phpstan/extension-installer": dict(
        version="1.4.3", category="code-generation", secondary=None,
        evidence="src/Plugin.php:91-197 (process() writes vendor/phpstan/extension-installer/src/GeneratedConfig.php)",
        note="Scans installed packages of type phpstan-extension (or carrying extra.phpstan) and writes a generated PHP class listing each one's install path and PHPStan version constraint.",
        declarable="yes", reason="derived entirely from the lock's package list and each package's own extra.phpstan; viv's own phpstan.rs proves it.",
    ),
    "symfony/runtime": dict(
        version="v8.1.0", category="code-generation", secondary=None,
        evidence="Internal/ComposerPlugin.php:52-108 (updateAutoloadFile writes vendor/autoload_runtime.php from a template)",
        note="Renders an autoload_runtime.php from a template, substituting the project dir and the runtime class/options named in root extra.runtime.",
        declarable="yes", reason="template + substitution values are static once extra.runtime is read; viv's own symfony_runtime.rs proves it.",
    ),
    "cweagans/composer-patches": dict(
        version="2.0.0", category="patching", secondary=None,
        evidence="src/Plugin/Patches.php:162-173 (getSubscribedEvents: PRE/POST_PACKAGE_INSTALL/UPDATE -> loadLockedPatches/patchPackage)",
        note="Applies a per-package list of patches (from extra.patches, a patches file, or a dependency's own extra.patches) with `patch`/`git apply` right after each package installs.",
        declarable="yes", reason="a patch list (URL or path + target package) is exactly data; viv's own patches.rs proves it.",
    ),
    "pestphp/pest-plugin": dict(
        version="v5.0.0", category="code-generation", secondary="command-only",
        evidence="src/Manager.php:59-78 (post-autoload-dump runs DumpCommand, which writes vendor/pest-plugins.json)",
        note="On post-autoload-dump, runs an internal command that writes a manifest of installed pest-plugin-type packages; also adds a CommandProvider capability for `pest` CLI commands.",
        declarable="yes", reason="the manifest is just the set of installed pest-plugin packages; viv's own pest.rs proves it.",
    ),
    "kylekatarnls/update-helper": dict(
        version="1.2.1", category="other", secondary=None,
        evidence="src/UpdateHelper/UpdateHelper.php:132-151 (check() instantiates every class named in any installed package's extra['update-helper'] and calls ->check())",
        note="On post-autoload-dump, scans every installed package's own composer.json for an extra.update-helper class name and instantiates + runs it -- a generic \"run this class after install\" hook any dependency can register.",
        declarable="no", reason="the whole point is running an arbitrary class a dependency names; there is no fixed effect to express as data.",
    ),
    "drupal/core-composer-scaffold": dict(
        version="11.4.8", category="scaffolding", secondary=None,
        evidence="Plugin.php:104-107,155-157 (postCmd/postPackage call Handler::scaffold()/onPostPackageEvent, which copy scaffold files into the web root)",
        note="Copies each dependency's declared scaffold files (extra.drupal-scaffold.file-mapping) into the project, honouring per-file append/replace and allowed-package rules.",
        declarable="yes", reason="the file-mapping is already a declared manifest key; viv's own drupal_scaffold.rs proves it.",
    ),
    "ergebnis/composer-normalize": dict(
        version="2.54.0", category="command-only", secondary=None,
        evidence="src/NormalizePlugin.php:29-52 (activate/deactivate/uninstall are all empty; only getCapabilities/getCommands add a `composer normalize` command)",
        note="Adds a `composer normalize` command that reorders/reformats composer.json; no event subscription at all, so nothing runs at install unless the command is invoked by hand.",
        declarable="n/a", reason="no install-time effect to declare.",
    ),
    "oomphinc/composer-installers-extender": dict(
        version="2.0.1", category="path-mapping", secondary=None,
        evidence="src/Plugin.php:17-21 (activate() registers an Installer via addInstaller)",
        note="Extends composer/installers so any package `type` can map to an install path via root extra.installer-types + extra.installer-paths, without needing a new installer package per type.",
        declarable="yes", reason="the type list and path map are both already root manifest data.",
    ),
    "yiisoft/yii2-composer": dict(
        version="2.0.11", category="path-mapping", secondary="code-generation",
        evidence="Plugin.php:46-56 (activate() addInstaller + seeds vendor/yiisoft/extensions.php); Installer.php:39-63 (install/update append to it)",
        note="Registers a yii2-extension installer and maintains a generated vendor/yiisoft/extensions.php mapping each extension's alias, class map and bootstrap entries.",
        declarable="yes", reason="both the install-path type and the generated extensions map are derived from each package's own extra.yii2 entries; viv's own yii2.rs proves it.",
    ),
    "infection/extension-installer": dict(
        version="0.1.2", category="code-generation", secondary=None,
        evidence="src/Plugin.php:99-140 (process() writes GeneratedExtensionsConfig.php)",
        note="Same shape as phpstan/extension-installer: scans installed infection-extension-type packages and writes a generated PHP class listing them.",
        declarable="yes", reason="derived entirely from the lock's package list and each package's extra.infection.",
    ),
    "wikimedia/composer-merge-plugin": dict(
        version="v2.1.0", category="other", secondary=None,
        evidence="src/MergePlugin.php:153-171 (getSubscribedEvents: PluginEvents::INIT -> onInit merges other composer.json fragments into the root package before resolution)",
        note="Merges the require/autoload/repositories/etc of every composer.json matched by extra.merge-plugin.include (a glob) into the in-memory root package before the solve runs -- affects what gets resolved, writes no file itself.",
        declarable="partly", reason="the include-glob and merge-mode are already root manifest data, but the effect (folding a second manifest into the solve) needs the resolver itself to accept multiple inputs, not just a data section.",
    ),
    "bamarni/composer-bin-plugin": dict(
        version="1.9.1", category="other", secondary="command-only",
        evidence="src/BamarniBinPlugin.php:78-141 (onCommandEvent/onPostAutoloadDump forward install/update to a nested `composer bin <ns> install`, when isCommandForwarded())",
        note="Adds a `composer bin` command family for installing dev tools into sibling vendor directories; when extra.bamarni-bin.forward-command is enabled, a plain install/update also runs a full nested Composer install per configured namespace.",
        declarable="no", reason="when forwarding is enabled it runs a further, independent Composer install per namespace -- an install, not data.",
    ),
    "tbachert/spi": dict(
        version="v1.0.5", category="code-generation", secondary=None,
        evidence="src/Composer/Plugin.php:58-151 (preAutoloadDump writes vendor/composer/GeneratedServiceProviderData.php)",
        note="Collects every installed package's extra.spi service->provider map (plus provider classes registered via autoload.files) and writes a generated PHP class serving them by class name.",
        declarable="yes", reason="the service/provider map is exactly extra.spi, already data; viv's own spi.rs proves it.",
    ),
    "drupal/core-project-message": dict(
        version="11.4.8", category="check-or-audit", secondary=None,
        evidence="MessagePlugin.php:63-75 (displayPostCreateMessage writes a message to the console; no file touched)",
        note="Prints a post-create/post-install message (from a package's own extra config); changes nothing on disk.",
        declarable="n/a", reason="no install-time disk effect to declare, it only writes to the console.",
    ),
    "magento/magento-composer-installer": dict(
        version="0.5.0", category="path-mapping", secondary="scaffolding",
        evidence="src/.../Plugin.php:84-98,206-269 (activate() addInstaller; deployLibraries() copies libraries into a fixed library path and shells out to phpab to regenerate an autoload.php)",
        note="Registers a Magento-module installer (deploy strategy per module), then copies configured library packages into a fixed target path and regenerates their autoload map via the external `phpab` binary.",
        declarable="partly", reason="the deploy paths and extra.chmod list are data, but the library autoload regeneration shells out to an external tool rather than deriving a fixed template.",
    ),
    "hirak/prestissimo": dict(
        version="0.3.10", category="download-or-auth", secondary=None,
        evidence="src/Plugin.php:98-129 (onPreFileDownload swaps in a CurlRemoteFilesystem for parallel/keep-alive downloads)",
        note="Replaces Composer 1's file downloader with a curl-multi-based one and pre-fetches Composer-type repository metadata in parallel; purely a download-speed strategy, Composer-1-only (2.0 does this natively).",
        declarable="no", reason="a runtime network strategy, not project data.",
    ),
    "roots/wordpress-core-installer": dict(
        version="v4.0.0", category="path-mapping", secondary=None,
        evidence="src/WordPressCorePlugin.php:36-39 (activate() addInstaller)",
        note="Registers an installer that places johnpbloch/wordpress-core-style WordPress core checkouts at the configured webroot.",
        declarable="yes", reason="a single fixed install-path rule; viv's own wordpress_core.rs proves it.",
    ),
    "phpro/grumphp": dict(
        version="v2.24.0", category="other", secondary=None,
        evidence="src/Composer/GrumPHPPlugin.php:108-158,210-234 (detectGrumphpAction schedules init/configure; runGrumPhpCommand shells out via proc_open to the grumphp binary)",
        note="On install/update, runs the installed `grumphp` binary's own `init`/`configure` commands, which write git hooks and a grumphp.yml config.",
        declarable="no", reason="invokes an external binary that runs arbitrary configured quality-check tasks; not a fixed effect to declare.",
    ),
    "symfony/thanks": dict(
        version="v1.4.1", category="command-only", secondary=None,
        evidence="src/Thanks.php:32-118 (activate adds `thanks`/`fund` commands; displayReminder only writes to the console)",
        note="Adds `composer thanks`/`composer fund` commands and, after an update, a console reminder to run them (queries the GitHub API for star status); no file is ever written.",
        declarable="n/a", reason="no install-time disk effect to declare.",
    ),
    "typo3/cms-composer-installers": dict(
        version="v5.0.2", category="code-generation", secondary="other",
        evidence="src/Plugin/PluginImplementation.php:82-94 (preAutoloadDump writes an include file via IncludeFile::register); Core/ScriptDispatcher.php:60-80 (executeScripts runs registered InstallerScript classes)",
        note="Writes a generated bootstrap include file with a handful of directory-path tokens substituted, then runs every InstallerScript class other TYPO3 packages have registered via extra.",
        declarable="partly", reason="the directory tokens are simple data, but the registered InstallerScript classes are arbitrary code by design.",
    ),
    "typo3/class-alias-loader": dict(
        version="v2.0.1", category="code-generation", secondary=None,
        evidence="src/Plugin.php:38-71 (onPreAutoloadDump calls ClassAliasMapGenerator::generateAliasMapFiles())",
        note="Writes a generated class-alias map and loader files from each package's own extra['typo3/class-alias-loader']['class-alias-maps'].",
        declarable="yes", reason="the alias map is already declared as extra data per package.",
    ),
    "laminas/laminas-dependency-plugin": dict(
        version="2.7.0", category="other", secondary=None,
        evidence="src/DependencyRewriterPluginDelegator.php:39-58 (getSubscribedEvents wires onPrePoolCreate/onPrePackageInstallOrUpdate to a rewriter); src/Replacements.php (the zendframework/zend-* -> laminas/* rename table)",
        note="Rewrites zendframework/zend-*/zfcampus-* requirements to their laminas/* successor before the solve, via a fixed rename table.",
        declarable="yes", reason="the rename table is static (no per-project configuration at all); a manifest-level migration list would express the same thing.",
    ),
    "drupal/console-extend-plugin": dict(
        version="0.9.5", category="path-mapping", secondary="code-generation",
        evidence="src/Extender.php:37-46,63-141 (activate() addInstaller; processPackages aggregates every drupal-console-library package's console.services.yml/console.config.yml into generated cache files)",
        note="Registers a drupal-console-library installer, then aggregates each installed library's fixed-named services/config YAML files into generated cache files consumed by Drupal Console.",
        declarable="yes", reason="both the install path and the two well-known filenames aggregated per package are fixed conventions, not per-project logic.",
    ),
    "cakephp/plugin-installer": dict(
        version="2.0.2", category="code-generation", secondary=None,
        evidence="src/Plugin.php:99-118,274-326 (postAutoloadDump writes vendor/cakephp-plugins.php mapping plugin name to path)",
        note="Scans installed cakephp-plugin-type packages (plus an app plugins/ directory) and writes a generated plugin-name-to-path map.",
        declarable="yes", reason="derived entirely from package type + a fixed directory convention.",
    ),
    "endroid/installer": dict(
        version="1.5.2", category="scaffolding", secondary=None,
        evidence="src/Installer.php:83-127,155-167 (installProjectType copies each package's .install/<project-type>/ tree into the project root)",
        note="Copies a fixed `.install/<detected-project-type>/` directory from each installed package into the project root, skipping files already present.",
        declarable="yes", reason="a fixed source-directory convention plus a root exclude list, both data.",
    ),
    "johnpbloch/wordpress-core-installer": dict(
        version="2.0.0", category="path-mapping", secondary=None,
        evidence="src/johnpbloch/Composer/WordPressCorePlugin.php:36-39 (activate() addInstaller)",
        note="Same shape as roots/wordpress-core-installer: registers an installer placing WordPress core at the configured webroot.",
        declarable="yes", reason="a single fixed install-path rule; viv's own wordpress_core.rs covers both this and roots' package under one adapter.",
    ),
    "simplesamlphp/composer-module-installer": dict(
        version="v2.0.0", category="path-mapping", secondary=None,
        evidence="src/ModuleInstallerPlugin.php:27-32 (activate() addInstaller)",
        note="Registers an installer placing simplesamlphp-module-type packages under modules/<name>.",
        declarable="yes", reason="a single fixed install-path rule.",
    ),
    "zaporylie/composer-drupal-optimizations": dict(
        version="1.2.0", category="download-or-auth", secondary=None,
        evidence="src/Plugin.php:45-61 (activate() swaps in a TruncatedComposerRepository-backed RepositoryManager)",
        note="Replaces Composer 1's repository manager with one that truncates the Packagist metadata Composer downloads to versions matching a (derived-or-declared) drupal/core constraint, to speed up solves; disabled outright on Composer 2.",
        declarable="no", reason="a runtime metadata-fetch optimisation, not project data.",
    ),
    "mglaman/composer-drupal-lenient": dict(
        version="2.0.0", category="other", secondary=None,
        evidence="src/Plugin.php:20-29,66-72 (modifyPackages, on PRE_POOL_CREATE, widens require.drupal/core on every package matching extra.drupal-lenient.allowed-list)",
        note="Widens the drupal/core (and similar) version constraint inside third-party contrib packages' own require, for a project-declared allow-list, so they resolve against an unreleased core.",
        declarable="partly", reason="the allow-list is data, but rewriting another package's own constraint at pre-pool-create is solver behaviour, not a fixed manifest effect.",
    ),
    "drupal-composer/drupal-scaffold": dict(
        version="2.6.1", category="scaffolding", secondary=None,
        evidence="src/Plugin.php:73-101 (postPackage/postCmd call Handler::onPostPackageEvent/onPostCmdEvent, which download and copy scaffold files)",
        note="The predecessor to drupal/core-composer-scaffold (deprecated in its own README in favour of it); same file-mapping-driven scaffold copy.",
        declarable="yes", reason="same file-mapping-as-data shape as its successor, which viv's drupal_scaffold.rs already proves declarable.",
    ),
    "robloach/component-installer": dict(
        version="0.2.3", category="path-mapping", secondary=None,
        evidence="src/ComponentInstaller/ComponentInstallerPlugin.php:30-34 (activate() addInstaller)",
        note="Registers an installer for `component`-type (bower/component.io-style) packages under a components/ directory.",
        declarable="yes", reason="a single fixed install-path rule.",
    ),
    "automattic/jetpack-autoloader": dict(
        version="v6.0.1", category="code-generation", secondary=None,
        evidence="src/CustomAutoloaderPlugin.php:93-119 (postAutoloadDump runs a custom AutoloadGenerator writing vendor/autoload_packages.php)",
        note="Replaces Composer's own autoloader with one that resolves version conflicts at runtime, for independently-shipped WordPress plugins/themes that may each bundle a different version of the same library.",
        declarable="partly", reason="the package list is data, but the runtime version-conflict resolution it generates is real branching logic, not a static map.",
    ),
    "codeception/c3": dict(
        version="2.9.0", category="scaffolding", secondary=None,
        evidence="Installer.php:88-104 (copyC3V2 copies a single fixed c3.php into the project root)",
        note="Copies its own bundled c3.php (a code-coverage collection endpoint) to the project root on install/update, asking before overwriting a locally modified copy.",
        declarable="yes", reason="a single fixed file copy with no per-project configuration; viv's own c3.rs proves it.",
    ),
    "mnsami/composer-custom-directory-installer": dict(
        version="2.2.1", category="path-mapping", secondary=None,
        evidence="src/Composer/CustomDirectoryInstaller/LibraryPlugin.php:26-30 (activate() addInstaller; one of 3 near-identical installer classes -- Library/Pear/Plugin -- for different legacy package types)",
        note="Extends library-type packages to honour root extra.installer-paths the way composer/installers does for typed packages.",
        declarable="yes", reason="a path map from root manifest data, same shape as composer/installers.",
    ),
    "acquia/blt": dict(
        version="13.7.4", category="other", secondary=None,
        evidence="src/Composer/Plugin.php:115-178 (onPostCmdEvent shells out to `blt internal:add-to-project` / `blt blt:update` via ProcessExecutor)",
        note="On install/update, runs its own `blt` CLI (a Robo task runner) to scaffold or update a Drupal project's BLT template files.",
        declarable="no", reason="invokes an external Robo-based CLI that runs arbitrary configured tasks, not a fixed file list.",
    ),
}

CATEGORIES = [
    "path-mapping", "scaffolding", "patching", "code-generation",
    "download-or-auth", "check-or-audit", "command-only", "other",
]


# --- Part 1: fetch + clone -------------------------------------------------

def scratch_dir() -> Path:
    d = os.environ.get("G3_PLUGINS_SCRATCH")
    path = Path(d) if d else Path(tempfile.mkdtemp(prefix="g3-plugins-"))
    path.mkdir(parents=True, exist_ok=True)
    return path


def fetch_json(url: str, timeout: int = 30) -> dict:
    req = urllib.request.Request(url, headers=UA)
    with urllib.request.urlopen(req, timeout=timeout) as resp:
        return json.load(resp)


def fetch_top(min_results: int = 300) -> list[dict]:
    """Every `search.json` result of type composer-plugin, paged until at
    least `min_results` are collected (or the registry runs out)."""
    results: list[dict] = []
    url = "https://packagist.org/search.json?type=composer-plugin&per_page=100"
    while url and len(results) < min_results:
        data = fetch_json(url)
        results.extend(data["results"])
        url = data.get("next")
    return results


def is_stable(version: dict) -> bool:
    v = version.get("version_normalized", "") or version.get("version", "")
    return not re.search(r"(dev|alpha|beta|RC|patch)", v, re.I)


def fetch_p2(name: str) -> dict | None:
    try:
        data = fetch_json(f"https://repo.packagist.org/p2/{name.lower()}.json", timeout=20)
    except (urllib.error.HTTPError, urllib.error.URLError, json.JSONDecodeError, TimeoutError):
        return None
    versions = (data.get("packages") or {}).get(name.lower()) or []
    stable = [v for v in versions if is_stable(v)]
    return (stable or versions or [None])[0]


def clone_tag(source: str, version: str, dest: Path, timeout: int = 60) -> tuple[bool, str]:
    if (dest / ".git").exists():
        return True, "reused"
    tags = [version, version[1:] if version.startswith("v") else "v" + version]
    last = "no tag"
    for tag in tags:
        r = subprocess.run(
            ["git", "clone", "--depth", "1", "--branch", tag, "-c", "advice.detachedHead=false", source, str(dest)],
            capture_output=True, text=True, timeout=timeout,
        )
        if r.returncode == 0:
            return True, f"tag {tag}"
        last = (r.stderr.strip().splitlines() or ["unknown error"])[-1]
    return False, last


def run_part1() -> list[dict]:
    scratch = scratch_dir() / "clones"
    scratch.mkdir(parents=True, exist_ok=True)
    top = fetch_top()
    top.sort(key=lambda r: -r["downloads"])
    top40 = top[:40]

    rows = []
    for rank, entry in enumerate(top40, 1):
        name = entry["name"]
        downloads = entry["downloads"]
        p2 = fetch_p2(name)
        row = {"rank": rank, "name": name, "downloads": downloads}
        if not p2:
            row["skip"] = "no p2 metadata"
            rows.append(row)
            continue
        source = (p2.get("source") or {}).get("url")
        version = p2.get("version")
        row["version"] = version
        row["source"] = source
        row["class"] = p2.get("extra", {}).get("class")
        req = dict(p2.get("require") or {})
        for k in ("php", "composer-plugin-api", "composer-runtime-api"):
            req.pop(k, None)
        row["extra_deps"] = sorted(req.keys())
        if not source or not version:
            row["skip"] = "missing source url or version"
            rows.append(row)
            continue
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", name)
        ok, label = clone_tag(source, version, scratch / safe)
        row["clone"] = label if ok else f"failed: {label}"
        if not ok:
            row["skip"] = f"clone failed: {label}"
        row["classified"] = CLASSIFICATION.get(name)
        row["viv_adapter"] = VIV_ADAPTERS.get(name)
        rows.append(row)
    return rows


def render_part1(rows: list[dict]) -> list[str]:
    lines = ["## Part 1: what the 40 most-downloaded Composer plugins do at install", ""]
    n_clone_ok = sum(1 for r in rows if "skip" not in r)
    n_classified = sum(1 for r in rows if r.get("classified"))
    lines.append(
        f"{len(rows)} packages ranked by Packagist `downloads` (out of >=300 composer-plugin packages paged through), "
        f"{n_clone_ok} cloned at their latest stable tag, {n_classified} matched against the hand-curated classification below."
    )
    lines.append("")
    header = ["#", "package", "downloads", "version", "category", "secondary", "declarable", "viv adapter"]
    lines.append("| " + " | ".join(header) + " |")
    lines.append("|" + "---|" * len(header))
    for r in rows:
        c = r.get("classified")
        if not c:
            note = "not classified since census" if "skip" not in r else r["skip"]
            lines.append(f"| {r['rank']} | {r['name']} | {r['downloads']:,} | {r.get('version','')} | {note} | | | |")
            continue
        lines.append(
            f"| {r['rank']} | {r['name']} | {r['downloads']:,} | {r.get('version','')} | "
            f"{c['category']} | {c.get('secondary') or ''} | {c['declarable']} | {r.get('viv_adapter') or ''} |"
        )
    lines.append("")
    lines.append("### Evidence")
    lines.append("")
    for r in rows:
        c = r.get("classified")
        if not c:
            continue
        lines.append(f"- **{r['name']}** (`{c['version']}`, `{c.get('evidence','')}`): {c['note']} Declarable: {c['declarable']} -- {c['reason']}")
    lines.append("")

    lines.append("### Aggregate")
    lines.append("")
    classified_rows = [(r, r["classified"]) for r in rows if r.get("classified")]
    cat_count: dict[str, int] = {c: 0 for c in CATEGORIES}
    cat_downloads: dict[str, int] = {c: 0 for c in CATEGORIES}
    decl_count = {"yes": 0, "partly": 0, "no": 0, "n/a": 0}
    for r, c in classified_rows:
        cat_count[c["category"]] += 1
        cat_downloads[c["category"]] += r["downloads"]
        decl_count[c["declarable"]] += 1
    lines.append(f"Of {len(classified_rows)} classified packages ({len(rows) - len(classified_rows)} not classified):")
    lines.append("")
    lines.append("| category | packages | total downloads |")
    lines.append("|---|---|---|")
    for c in CATEGORIES:
        if cat_count[c]:
            lines.append(f"| {c} | {cat_count[c]} | {cat_downloads[c]:,} |")
    lines.append("")
    lines.append("| declarable | packages |")
    lines.append("|---|---|")
    for k in ("yes", "partly", "no", "n/a"):
        if decl_count[k]:
            lines.append(f"| {k} | {decl_count[k]} |")
    lines.append("")
    n_viv = sum(1 for r in rows if r.get("viv_adapter"))
    lines.append(f"viv already has a native adapter for {n_viv} of the 40 (`src/plugins/*.rs`).")
    lines.append("")
    return lines


# --- Part 2: corpus, who runs code at install -----------------------------

SCRIPT_EVENTS = [
    "pre-install-cmd", "post-install-cmd", "pre-update-cmd", "post-update-cmd",
    "post-autoload-dump", "pre-autoload-dump", "post-package-install",
    "post-root-package-install", "post-create-project-cmd",
]

_STATIC_CALLBACK_RE = re.compile(r"^\\?[A-Za-z0-9_\\]+::[A-Za-z0-9_]+$")


def classify_handler(handler: str) -> str:
    if handler.startswith("@php "):
        return "@php"
    if _STATIC_CALLBACK_RE.match(handler.strip()):
        return "static PHP callback"
    return "shell command"


def run_part2() -> list[dict]:
    scratch = Path(os.environ.get("G3_PLUGINS_SCRATCH") or tempfile.mkdtemp(prefix="g3-plugins-corpus-")) / "corpus"
    scratch.mkdir(parents=True, exist_ok=True)
    only = {n.strip() for n in os.environ["G3_PLUGINS_ONLY"].split(",")} if os.environ.get("G3_PLUGINS_ONLY") else None
    rows: list[dict] = []
    for name, repo, commit, source in platform_drift.CORPUS:
        if only and name not in only:
            continue
        if repo == platform_drift.NO_CHECKOUT:
            rows.append({"name": name, "source": source, "skip": "no installable git checkout"})
            continue
        candidates = [repo] if repo is not None else platform_drift.resolve_repo_candidates(name)
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", name)
        dest = scratch / safe
        ok, label = False, "no candidate repo URL"
        for candidate in candidates:
            if not (dest / ".git").exists():
                ok, label = platform_drift.clone_at(candidate, commit, dest)
                if ok:
                    break
            else:
                ok, label = True, "reused"
                break
        if not ok:
            rows.append({"name": name, "source": source, "skip": f"clone failed: {label}"})
            continue
        pr = platform_drift.analyse(name, source, dest, label)
        if pr.skip:
            rows.append({"name": name, "source": source, "skip": pr.skip})
            continue
        lock = json.loads((dest / "composer.lock").read_text(errors="replace"))
        root_path = dest / "composer.json"
        root = json.loads(root_path.read_text(errors="replace")) if root_path.is_file() else {}

        packages = (lock.get("packages") or []) + (lock.get("packages-dev") or [])
        plugins = sorted(
            f"{p.get('name')} {p.get('version')}" for p in packages if p.get("type") == "composer-plugin"
        )
        allow_plugins = root.get("config", {}).get("allow-plugins")
        allows_any = bool(allow_plugins) and (allow_plugins is True or any(allow_plugins.values()))

        scripts = root.get("scripts") or {}
        events = {}
        for ev in SCRIPT_EVENTS:
            if ev not in scripts:
                continue
            handlers = scripts[ev] if isinstance(scripts[ev], list) else [scripts[ev]]
            events[ev] = [classify_handler(h) for h in handlers if isinstance(h, str)]

        runs_code = bool(plugins and allows_any) or bool(events)
        rows.append(
            {
                "name": name, "source": source, "commit": label,
                "plugins": plugins, "allow_plugins": allow_plugins, "allows_any": allows_any,
                "events": events, "runs_code": runs_code,
            }
        )
    return rows


def render_part2(rows: list[dict]) -> list[str]:
    lines = ["## Part 2: corpus, who runs code at install", ""]
    analysed = [r for r in rows if "skip" not in r]
    n = len(analysed)
    lines.append(f"{n} of {len(rows)} corpus projects analysed ({len(rows) - n} skipped, no committed lock or no checkout).")
    lines.append("")
    header = ["project", "composer-plugin packages (locked)", "allow-plugins allows any", "script events defined", "runs code at install"]
    lines.append("| " + " | ".join(header) + " |")
    lines.append("|" + "---|" * len(header))
    for r in rows:
        if "skip" in r:
            lines.append(f"| {r['name']} | skip: {r['skip']} | | | |")
            continue
        plugin_cell = ", ".join(r["plugins"]) if r["plugins"] else ""
        events_cell = ", ".join(f"{ev} ({'/'.join(kinds)})" for ev, kinds in r["events"].items()) if r["events"] else ""
        lines.append(
            f"| {r['name']} | {plugin_cell} | {'yes' if r['allows_any'] else 'no'} | {events_cell} | "
            f"{'yes' if r['runs_code'] else 'no'} |"
        )
    lines.append("")

    lines.append("### Totals")
    lines.append("")
    n_plugins = sum(1 for r in analysed if r["plugins"])
    n_allows = sum(1 for r in analysed if r["allows_any"])
    n_events = sum(1 for r in analysed if r["events"])
    n_runs = sum(1 for r in analysed if r["runs_code"])
    lines.append(f"- {n_plugins} of {n} projects lock at least one `type: composer-plugin` package.")
    lines.append(f"- {n_allows} of {n} projects' `config.allow-plugins` allows at least one plugin to run.")
    event_counts: dict[str, dict[str, int]] = {ev: {} for ev in SCRIPT_EVENTS}
    for r in analysed:
        for ev, kinds in r["events"].items():
            for kind in kinds:
                event_counts[ev][kind] = event_counts[ev].get(kind, 0) + 1
    for ev in SCRIPT_EVENTS:
        counts = event_counts[ev]
        if not counts:
            continue
        total = sum(counts.values())
        breakdown = ", ".join(f"{kind} {n_}" for kind, n_ in sorted(counts.items(), key=lambda kv: -kv[1]))
        lines.append(f"- `{ev}`: {total} handlers across the corpus ({breakdown}).")
    lines.append(f"- {n_events} of {n} projects define at least one of the nine script events.")
    lines.append(f"- **{n_runs} of {n} projects run some code (a plugin or a script) during `viv install` today.**")
    lines.append("")
    return lines, {"n": n, "n_plugins": n_plugins, "n_allows": n_allows, "n_events": n_events, "n_runs": n_runs}


def main() -> int:
    start = time.monotonic()
    fetch_date = "2026-09-28"

    rows1 = run_part1()
    rows2 = run_part2()
    part2_lines, part2_summary = render_part2(rows2)
    wall = time.monotonic() - start

    lines = []
    lines.append("# Candidate 3.4: installs that run no code, the measurement (#332)")
    lines.append("")
    lines.append(
        f"How to reproduce: `make bench-g3-plugins` (network: Packagist search/p2 API and GitHub clones "
        f"for Part 1, corpus clones for Part 2). Packagist plugin list fetched {fetch_date}."
    )
    lines.append("")
    lines.extend(render_part1(rows1))
    lines.extend(part2_lines)

    lines.append("## Reading")
    lines.append("")
    classified = [(r, r["classified"]) for r in rows1 if r.get("classified")]
    cat_count: dict[str, int] = {}
    decl_count = {"yes": 0, "partly": 0, "no": 0, "n/a": 0}
    for r, c in classified:
        cat_count[c["category"]] = cat_count.get(c["category"], 0) + 1
        decl_count[c["declarable"]] += 1
    cat_sentence = ", ".join(f"{c} {n_}" for c, n_ in sorted(cat_count.items(), key=lambda kv: -kv[1]))
    n_viv = sum(1 for r in rows1 if r.get("viv_adapter"))
    lines.append(
        f"Of the 40 most-downloaded Composer plugins, {cat_sentence}. Declarable: "
        f"{decl_count['yes']} yes, {decl_count['partly']} partly, {decl_count['no']} no, {decl_count['n/a']} n/a "
        f"(no install-time disk effect at all). viv already has a native adapter for {n_viv} of them. "
        f"Across the corpus, {part2_summary['n_plugins']} of {part2_summary['n']} projects lock a composer-plugin "
        f"package, {part2_summary['n_allows']} of {part2_summary['n']} allow at least one to run, "
        f"{part2_summary['n_events']} of {part2_summary['n']} define at least one of the nine script events, and "
        f"{part2_summary['n_runs']} of {part2_summary['n']} run some code during `viv install` today, one way or the other."
    )
    lines.append("")
    lines.append(f"Wall time: {wall:.1f}s.")
    lines.append("")

    print("\n".join(lines))
    print(f"wall time: {wall:.1f}s", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
