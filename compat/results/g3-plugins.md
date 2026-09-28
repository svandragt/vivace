# Candidate 3.4: installs that run no code, the measurement (#332)

How to reproduce: `make bench-g3-plugins` (network: Packagist search/p2 API and GitHub clones for Part 1, corpus clones for Part 2). Packagist plugin list fetched 2026-09-28.

## Part 1: what the 40 most-downloaded Composer plugins do at install

40 packages ranked by Packagist `downloads` (out of >=300 composer-plugin packages paged through), 40 cloned at their latest stable tag, 40 matched against the hand-curated classification below.

| # | package | downloads | version | category | secondary | declarable | viv adapter |
|---|---|---|---|---|---|---|---|
| 1 | php-http/discovery | 364,799,576 | 1.20.0 | code-generation |  | yes | discovery.rs |
| 2 | symfony/flex | 208,692,367 | v2.11.0 | scaffolding | download-or-auth | partly |  |
| 3 | composer/package-versions-deprecated | 202,118,474 | 1.11.99.5 | code-generation |  | yes |  |
| 4 | dealerdirect/phpcodesniffer-composer-installer | 192,631,310 | v1.2.1 | code-generation |  | yes | phpcs.rs |
| 5 | composer/installers | 151,659,027 | v2.3.0 | path-mapping |  | yes | installers.rs |
| 6 | phpstan/extension-installer | 123,071,385 | 1.4.3 | code-generation |  | yes | phpstan.rs |
| 7 | symfony/runtime | 112,413,371 | v8.1.0 | code-generation |  | yes | symfony_runtime.rs |
| 8 | cweagans/composer-patches | 109,808,503 | 2.0.0 | patching |  | yes | patches.rs |
| 9 | pestphp/pest-plugin | 88,418,240 | v5.0.0 | code-generation | command-only | yes | pest.rs |
| 10 | kylekatarnls/update-helper | 48,770,694 | 1.2.1 | other |  | no |  |
| 11 | drupal/core-composer-scaffold | 48,447,887 | 11.4.8 | scaffolding |  | yes | drupal_scaffold.rs |
| 12 | ergebnis/composer-normalize | 46,133,581 | 2.54.0 | command-only |  | n/a |  |
| 13 | oomphinc/composer-installers-extender | 32,271,449 | 2.0.1 | path-mapping |  | yes |  |
| 14 | yiisoft/yii2-composer | 31,786,894 | 2.0.11 | path-mapping | code-generation | yes | yii2.rs |
| 15 | infection/extension-installer | 30,933,310 | 0.1.2 | code-generation |  | yes |  |
| 16 | wikimedia/composer-merge-plugin | 29,943,929 | v2.1.0 | other |  | partly |  |
| 17 | bamarni/composer-bin-plugin | 27,511,781 | 1.9.1 | other | command-only | no |  |
| 18 | tbachert/spi | 27,379,481 | v1.0.5 | code-generation |  | yes | spi.rs |
| 19 | drupal/core-project-message | 26,781,105 | 11.4.8 | check-or-audit |  | n/a |  |
| 20 | magento/magento-composer-installer | 25,161,114 | 0.5.0 | path-mapping | scaffolding | partly |  |
| 21 | hirak/prestissimo | 19,439,046 | 0.3.10 | download-or-auth |  | no |  |
| 22 | roots/wordpress-core-installer | 17,892,727 | v4.0.0 | path-mapping |  | yes | wordpress_core.rs |
| 23 | phpro/grumphp | 17,850,412 | v2.24.0 | other |  | no |  |
| 24 | symfony/thanks | 16,881,131 | v1.4.1 | command-only |  | n/a |  |
| 25 | typo3/cms-composer-installers | 15,740,650 | v5.0.2 | code-generation | other | partly |  |
| 26 | typo3/class-alias-loader | 15,724,267 | v2.0.1 | code-generation |  | yes |  |
| 27 | laminas/laminas-dependency-plugin | 14,995,469 | 2.7.0 | other |  | yes |  |
| 28 | drupal/console-extend-plugin | 14,424,875 | 0.9.5 | path-mapping | code-generation | yes |  |
| 29 | cakephp/plugin-installer | 13,613,028 | 2.0.2 | code-generation |  | yes |  |
| 30 | endroid/installer | 13,516,604 | 1.5.2 | scaffolding |  | yes |  |
| 31 | johnpbloch/wordpress-core-installer | 11,822,270 | 2.0.0 | path-mapping |  | yes | wordpress_core.rs |
| 32 | simplesamlphp/composer-module-installer | 11,663,023 | v2.0.0 | path-mapping |  | yes |  |
| 33 | zaporylie/composer-drupal-optimizations | 10,594,789 | 1.2.0 | download-or-auth |  | no |  |
| 34 | mglaman/composer-drupal-lenient | 9,354,997 | 2.0.0 | other |  | partly |  |
| 35 | drupal-composer/drupal-scaffold | 9,146,397 | 2.6.1 | scaffolding |  | yes |  |
| 36 | robloach/component-installer | 7,901,348 | 0.2.3 | path-mapping |  | yes |  |
| 37 | automattic/jetpack-autoloader | 6,851,324 | v6.0.1 | code-generation |  | partly |  |
| 38 | codeception/c3 | 5,920,190 | 2.9.0 | scaffolding |  | yes | c3.rs |
| 39 | mnsami/composer-custom-directory-installer | 5,734,071 | 2.2.1 | path-mapping |  | yes |  |
| 40 | acquia/blt | 5,595,180 | 13.7.4 | other |  | no |  |

### Evidence

- **php-http/discovery** (`1.20.0`, `src/Composer/Plugin.php:369-403 (preAutoloadDump writes vendor/composer/GeneratedDiscoveryStrategy.php)`): Writes a generated PHP switch mapping each pinned `*-implementation` interface to its chosen class; a second listener (postUpdate) also edits composer.json to add the missing implementation package, out of scope for viv's own port too. Declarable: yes -- the pinned interface->class map is exactly `extra.discovery`, already data; viv's own discovery.rs proves the generated file is derivable from it.
- **symfony/flex** (`v2.11.0`, `src/Flex.php:400-430 (install() calls fetchRecipes()/Configurator to apply a fetched recipe)`): Fetches a "recipe" (files + config edits) per newly required package from symfony/recipes (or a private endpoint) and applies it via a Configurator that can create files, edit .env/services.yaml/bundles.php, and add composer scripts. Declarable: partly -- the recipe manifest itself (files + config directives) is already close to data, but recipes are fetched from a remote server at install time and a recipe can carry an arbitrary post-install hook.
- **composer/package-versions-deprecated** (`1.11.99.5`, `src/PackageVersions/Installer.php:36-108 (generated class template with a VERSIONS const, written on post-install/update)`): Writes a generated PHP class whose VERSIONS const maps every installed package to its resolved version string; deprecated upstream in favour of Composer 2's own InstalledVersions. Declarable: yes -- the VERSIONS map is derived entirely from the lock -- Composer 2 already generates the same information as data (installed.php).
- **dealerdirect/phpcodesniffer-composer-installer** (`v1.2.1`, `src/Plugin.php:263-297 (saveInstalledPaths shells out to `phpcs --config-set installed_paths ...`)`): Collects every installed PHP_CodeSniffer coding-standard package's path and writes them into phpcs's own CodeSniffer.conf via `phpcs --config-set`. Declarable: yes -- the installed_paths value is just a sorted list of package install paths; viv's own phpcs.rs writes the same config without shelling out.
- **composer/installers** (`v2.3.0`, `src/Composer/Installers/Plugin.php:14-18 (activate() registers a custom Installer via addInstaller)`): Registers an Installer that maps ~200 known package `type` values (wordpress-plugin, drupal-module, ...) to an install path template. Declarable: yes -- the type->path template map is a static table; viv's own installers.rs is that table as Rust data.
- **phpstan/extension-installer** (`1.4.3`, `src/Plugin.php:91-197 (process() writes vendor/phpstan/extension-installer/src/GeneratedConfig.php)`): Scans installed packages of type phpstan-extension (or carrying extra.phpstan) and writes a generated PHP class listing each one's install path and PHPStan version constraint. Declarable: yes -- derived entirely from the lock's package list and each package's own extra.phpstan; viv's own phpstan.rs proves it.
- **symfony/runtime** (`v8.1.0`, `Internal/ComposerPlugin.php:52-108 (updateAutoloadFile writes vendor/autoload_runtime.php from a template)`): Renders an autoload_runtime.php from a template, substituting the project dir and the runtime class/options named in root extra.runtime. Declarable: yes -- template + substitution values are static once extra.runtime is read; viv's own symfony_runtime.rs proves it.
- **cweagans/composer-patches** (`2.0.0`, `src/Plugin/Patches.php:162-173 (getSubscribedEvents: PRE/POST_PACKAGE_INSTALL/UPDATE -> loadLockedPatches/patchPackage)`): Applies a per-package list of patches (from extra.patches, a patches file, or a dependency's own extra.patches) with `patch`/`git apply` right after each package installs. Declarable: yes -- a patch list (URL or path + target package) is exactly data; viv's own patches.rs proves it.
- **pestphp/pest-plugin** (`v5.0.0`, `src/Manager.php:59-78 (post-autoload-dump runs DumpCommand, which writes vendor/pest-plugins.json)`): On post-autoload-dump, runs an internal command that writes a manifest of installed pest-plugin-type packages; also adds a CommandProvider capability for `pest` CLI commands. Declarable: yes -- the manifest is just the set of installed pest-plugin packages; viv's own pest.rs proves it.
- **kylekatarnls/update-helper** (`1.2.1`, `src/UpdateHelper/UpdateHelper.php:132-151 (check() instantiates every class named in any installed package's extra['update-helper'] and calls ->check())`): On post-autoload-dump, scans every installed package's own composer.json for an extra.update-helper class name and instantiates + runs it -- a generic "run this class after install" hook any dependency can register. Declarable: no -- the whole point is running an arbitrary class a dependency names; there is no fixed effect to express as data.
- **drupal/core-composer-scaffold** (`11.4.8`, `Plugin.php:104-107,155-157 (postCmd/postPackage call Handler::scaffold()/onPostPackageEvent, which copy scaffold files into the web root)`): Copies each dependency's declared scaffold files (extra.drupal-scaffold.file-mapping) into the project, honouring per-file append/replace and allowed-package rules. Declarable: yes -- the file-mapping is already a declared manifest key; viv's own drupal_scaffold.rs proves it.
- **ergebnis/composer-normalize** (`2.54.0`, `src/NormalizePlugin.php:29-52 (activate/deactivate/uninstall are all empty; only getCapabilities/getCommands add a `composer normalize` command)`): Adds a `composer normalize` command that reorders/reformats composer.json; no event subscription at all, so nothing runs at install unless the command is invoked by hand. Declarable: n/a -- no install-time effect to declare.
- **oomphinc/composer-installers-extender** (`2.0.1`, `src/Plugin.php:17-21 (activate() registers an Installer via addInstaller)`): Extends composer/installers so any package `type` can map to an install path via root extra.installer-types + extra.installer-paths, without needing a new installer package per type. Declarable: yes -- the type list and path map are both already root manifest data.
- **yiisoft/yii2-composer** (`2.0.11`, `Plugin.php:46-56 (activate() addInstaller + seeds vendor/yiisoft/extensions.php); Installer.php:39-63 (install/update append to it)`): Registers a yii2-extension installer and maintains a generated vendor/yiisoft/extensions.php mapping each extension's alias, class map and bootstrap entries. Declarable: yes -- both the install-path type and the generated extensions map are derived from each package's own extra.yii2 entries; viv's own yii2.rs proves it.
- **infection/extension-installer** (`0.1.2`, `src/Plugin.php:99-140 (process() writes GeneratedExtensionsConfig.php)`): Same shape as phpstan/extension-installer: scans installed infection-extension-type packages and writes a generated PHP class listing them. Declarable: yes -- derived entirely from the lock's package list and each package's extra.infection.
- **wikimedia/composer-merge-plugin** (`v2.1.0`, `src/MergePlugin.php:153-171 (getSubscribedEvents: PluginEvents::INIT -> onInit merges other composer.json fragments into the root package before resolution)`): Merges the require/autoload/repositories/etc of every composer.json matched by extra.merge-plugin.include (a glob) into the in-memory root package before the solve runs -- affects what gets resolved, writes no file itself. Declarable: partly -- the include-glob and merge-mode are already root manifest data, but the effect (folding a second manifest into the solve) needs the resolver itself to accept multiple inputs, not just a data section.
- **bamarni/composer-bin-plugin** (`1.9.1`, `src/BamarniBinPlugin.php:78-141 (onCommandEvent/onPostAutoloadDump forward install/update to a nested `composer bin <ns> install`, when isCommandForwarded())`): Adds a `composer bin` command family for installing dev tools into sibling vendor directories; when extra.bamarni-bin.forward-command is enabled, a plain install/update also runs a full nested Composer install per configured namespace. Declarable: no -- when forwarding is enabled it runs a further, independent Composer install per namespace -- an install, not data.
- **tbachert/spi** (`v1.0.5`, `src/Composer/Plugin.php:58-151 (preAutoloadDump writes vendor/composer/GeneratedServiceProviderData.php)`): Collects every installed package's extra.spi service->provider map (plus provider classes registered via autoload.files) and writes a generated PHP class serving them by class name. Declarable: yes -- the service/provider map is exactly extra.spi, already data; viv's own spi.rs proves it.
- **drupal/core-project-message** (`11.4.8`, `MessagePlugin.php:63-75 (displayPostCreateMessage writes a message to the console; no file touched)`): Prints a post-create/post-install message (from a package's own extra config); changes nothing on disk. Declarable: n/a -- no install-time disk effect to declare, it only writes to the console.
- **magento/magento-composer-installer** (`0.5.0`, `src/.../Plugin.php:84-98,206-269 (activate() addInstaller; deployLibraries() copies libraries into a fixed library path and shells out to phpab to regenerate an autoload.php)`): Registers a Magento-module installer (deploy strategy per module), then copies configured library packages into a fixed target path and regenerates their autoload map via the external `phpab` binary. Declarable: partly -- the deploy paths and extra.chmod list are data, but the library autoload regeneration shells out to an external tool rather than deriving a fixed template.
- **hirak/prestissimo** (`0.3.10`, `src/Plugin.php:98-129 (onPreFileDownload swaps in a CurlRemoteFilesystem for parallel/keep-alive downloads)`): Replaces Composer 1's file downloader with a curl-multi-based one and pre-fetches Composer-type repository metadata in parallel; purely a download-speed strategy, Composer-1-only (2.0 does this natively). Declarable: no -- a runtime network strategy, not project data.
- **roots/wordpress-core-installer** (`v4.0.0`, `src/WordPressCorePlugin.php:36-39 (activate() addInstaller)`): Registers an installer that places johnpbloch/wordpress-core-style WordPress core checkouts at the configured webroot. Declarable: yes -- a single fixed install-path rule; viv's own wordpress_core.rs proves it.
- **phpro/grumphp** (`v2.24.0`, `src/Composer/GrumPHPPlugin.php:108-158,210-234 (detectGrumphpAction schedules init/configure; runGrumPhpCommand shells out via proc_open to the grumphp binary)`): On install/update, runs the installed `grumphp` binary's own `init`/`configure` commands, which write git hooks and a grumphp.yml config. Declarable: no -- invokes an external binary that runs arbitrary configured quality-check tasks; not a fixed effect to declare.
- **symfony/thanks** (`v1.4.1`, `src/Thanks.php:32-118 (activate adds `thanks`/`fund` commands; displayReminder only writes to the console)`): Adds `composer thanks`/`composer fund` commands and, after an update, a console reminder to run them (queries the GitHub API for star status); no file is ever written. Declarable: n/a -- no install-time disk effect to declare.
- **typo3/cms-composer-installers** (`v5.0.2`, `src/Plugin/PluginImplementation.php:82-94 (preAutoloadDump writes an include file via IncludeFile::register); Core/ScriptDispatcher.php:60-80 (executeScripts runs registered InstallerScript classes)`): Writes a generated bootstrap include file with a handful of directory-path tokens substituted, then runs every InstallerScript class other TYPO3 packages have registered via extra. Declarable: partly -- the directory tokens are simple data, but the registered InstallerScript classes are arbitrary code by design.
- **typo3/class-alias-loader** (`v2.0.1`, `src/Plugin.php:38-71 (onPreAutoloadDump calls ClassAliasMapGenerator::generateAliasMapFiles())`): Writes a generated class-alias map and loader files from each package's own extra['typo3/class-alias-loader']['class-alias-maps']. Declarable: yes -- the alias map is already declared as extra data per package.
- **laminas/laminas-dependency-plugin** (`2.7.0`, `src/DependencyRewriterPluginDelegator.php:39-58 (getSubscribedEvents wires onPrePoolCreate/onPrePackageInstallOrUpdate to a rewriter); src/Replacements.php (the zendframework/zend-* -> laminas/* rename table)`): Rewrites zendframework/zend-*/zfcampus-* requirements to their laminas/* successor before the solve, via a fixed rename table. Declarable: yes -- the rename table is static (no per-project configuration at all); a manifest-level migration list would express the same thing.
- **drupal/console-extend-plugin** (`0.9.5`, `src/Extender.php:37-46,63-141 (activate() addInstaller; processPackages aggregates every drupal-console-library package's console.services.yml/console.config.yml into generated cache files)`): Registers a drupal-console-library installer, then aggregates each installed library's fixed-named services/config YAML files into generated cache files consumed by Drupal Console. Declarable: yes -- both the install path and the two well-known filenames aggregated per package are fixed conventions, not per-project logic.
- **cakephp/plugin-installer** (`2.0.2`, `src/Plugin.php:99-118,274-326 (postAutoloadDump writes vendor/cakephp-plugins.php mapping plugin name to path)`): Scans installed cakephp-plugin-type packages (plus an app plugins/ directory) and writes a generated plugin-name-to-path map. Declarable: yes -- derived entirely from package type + a fixed directory convention.
- **endroid/installer** (`1.5.2`, `src/Installer.php:83-127,155-167 (installProjectType copies each package's .install/<project-type>/ tree into the project root)`): Copies a fixed `.install/<detected-project-type>/` directory from each installed package into the project root, skipping files already present. Declarable: yes -- a fixed source-directory convention plus a root exclude list, both data.
- **johnpbloch/wordpress-core-installer** (`2.0.0`, `src/johnpbloch/Composer/WordPressCorePlugin.php:36-39 (activate() addInstaller)`): Same shape as roots/wordpress-core-installer: registers an installer placing WordPress core at the configured webroot. Declarable: yes -- a single fixed install-path rule; viv's own wordpress_core.rs covers both this and roots' package under one adapter.
- **simplesamlphp/composer-module-installer** (`v2.0.0`, `src/ModuleInstallerPlugin.php:27-32 (activate() addInstaller)`): Registers an installer placing simplesamlphp-module-type packages under modules/<name>. Declarable: yes -- a single fixed install-path rule.
- **zaporylie/composer-drupal-optimizations** (`1.2.0`, `src/Plugin.php:45-61 (activate() swaps in a TruncatedComposerRepository-backed RepositoryManager)`): Replaces Composer 1's repository manager with one that truncates the Packagist metadata Composer downloads to versions matching a (derived-or-declared) drupal/core constraint, to speed up solves; disabled outright on Composer 2. Declarable: no -- a runtime metadata-fetch optimisation, not project data.
- **mglaman/composer-drupal-lenient** (`2.0.0`, `src/Plugin.php:20-29,66-72 (modifyPackages, on PRE_POOL_CREATE, widens require.drupal/core on every package matching extra.drupal-lenient.allowed-list)`): Widens the drupal/core (and similar) version constraint inside third-party contrib packages' own require, for a project-declared allow-list, so they resolve against an unreleased core. Declarable: partly -- the allow-list is data, but rewriting another package's own constraint at pre-pool-create is solver behaviour, not a fixed manifest effect.
- **drupal-composer/drupal-scaffold** (`2.6.1`, `src/Plugin.php:73-101 (postPackage/postCmd call Handler::onPostPackageEvent/onPostCmdEvent, which download and copy scaffold files)`): The predecessor to drupal/core-composer-scaffold (deprecated in its own README in favour of it); same file-mapping-driven scaffold copy. Declarable: yes -- same file-mapping-as-data shape as its successor, which viv's drupal_scaffold.rs already proves declarable.
- **robloach/component-installer** (`0.2.3`, `src/ComponentInstaller/ComponentInstallerPlugin.php:30-34 (activate() addInstaller)`): Registers an installer for `component`-type (bower/component.io-style) packages under a components/ directory. Declarable: yes -- a single fixed install-path rule.
- **automattic/jetpack-autoloader** (`v6.0.1`, `src/CustomAutoloaderPlugin.php:93-119 (postAutoloadDump runs a custom AutoloadGenerator writing vendor/autoload_packages.php)`): Replaces Composer's own autoloader with one that resolves version conflicts at runtime, for independently-shipped WordPress plugins/themes that may each bundle a different version of the same library. Declarable: partly -- the package list is data, but the runtime version-conflict resolution it generates is real branching logic, not a static map.
- **codeception/c3** (`2.9.0`, `Installer.php:88-104 (copyC3V2 copies a single fixed c3.php into the project root)`): Copies its own bundled c3.php (a code-coverage collection endpoint) to the project root on install/update, asking before overwriting a locally modified copy. Declarable: yes -- a single fixed file copy with no per-project configuration; viv's own c3.rs proves it.
- **mnsami/composer-custom-directory-installer** (`2.2.1`, `src/Composer/CustomDirectoryInstaller/LibraryPlugin.php:26-30 (activate() addInstaller; one of 3 near-identical installer classes -- Library/Pear/Plugin -- for different legacy package types)`): Extends library-type packages to honour root extra.installer-paths the way composer/installers does for typed packages. Declarable: yes -- a path map from root manifest data, same shape as composer/installers.
- **acquia/blt** (`13.7.4`, `src/Composer/Plugin.php:115-178 (onPostCmdEvent shells out to `blt internal:add-to-project` / `blt blt:update` via ProcessExecutor)`): On install/update, runs its own `blt` CLI (a Robo task runner) to scaffold or update a Drupal project's BLT template files. Declarable: no -- invokes an external Robo-based CLI that runs arbitrary configured tasks, not a fixed file list.

### Aggregate

Of 40 classified packages (0 not classified):

| category | packages | total downloads |
|---|---|---|
| path-mapping | 10 | 310,316,798 |
| scaffolding | 5 | 285,723,445 |
| patching | 1 | 109,808,503 |
| code-generation | 12 | 1,193,694,416 |
| download-or-auth | 2 | 30,033,835 |
| check-or-audit | 1 | 26,781,105 |
| command-only | 2 | 63,014,712 |
| other | 7 | 154,022,462 |

| declarable | packages |
|---|---|
| yes | 25 |
| partly | 6 |
| no | 6 |
| n/a | 3 |

viv already has a native adapter for 13 of the 40 (`src/plugins/*.rs`).

## Part 2: corpus, who runs code at install

20 of 53 corpus projects analysed (33 skipped, no committed lock or no checkout).

| project | composer-plugin packages (locked) | allow-plugins allows any | script events defined | runs code at install |
|---|---|---|---|---|
| laravel/laravel | skip: no committed composer.lock | | | |
| symfony/demo | symfony/flex v2.11.0, symfony/runtime v8.1.0 | yes | post-install-cmd (shell command), post-update-cmd (shell command) | yes |
| drupal/recommended-project | skip: no installable git checkout | | | |
| roots/bedrock | skip: no committed composer.lock | | | |
| composer/composer |  | no |  | no |
| phpunit/phpunit |  | no |  | no |
| slimphp/Slim-Skeleton | skip: no committed composer.lock | | | |
| yiisoft/yii2-app-basic | skip: no committed composer.lock | | | |
| statamic/statamic | skip: no committed composer.lock | | | |
| craftcms/craft | skip: no committed composer.lock | | | |
| typo3/cms-base-distribution | skip: no committed composer.lock | | | |
| cakephp/app | skip: no committed composer.lock | | | |
| contao/managed-edition | skip: no committed composer.lock | | | |
| silverstripe/installer | skip: no committed composer.lock | | | |
| shopware/template | skip: no committed composer.lock | | | |
| octobercms/october | skip: no committed composer.lock | | | |
| wp-cli/wp-cli-bundle | dealerdirect/phpcodesniffer-composer-installer v1.2.1, phpstan/extension-installer 1.4.3 | yes |  | yes |
| bolt/project | skip: no committed composer.lock | | | |
| firstphp/ip2region | skip: no committed composer.lock | | | |
| whoa-php/flute | skip: no committed composer.lock | | | |
| accessd/yii2-rollbar | skip: no committed composer.lock | | | |
| tsg/ar | skip: clone failed: fatal: repository 'https://github.com/tsg/ar.git/' not found | | | |
| tumtum/oxid-inline-translator | skip: no committed composer.lock | | | |
| mozart/event-dispatcher | skip: no committed composer.lock | | | |
| ahmadarif/laravel-pagination | skip: no committed composer.lock | | | |
| tokimikichika/text-analysis | skip: no committed composer.lock | | | |
| 8xprovn/microservice | skip: no committed composer.lock | | | |
| dmstr/api-configuration-bundle | skip: no committed composer.lock | | | |
| numesia/all-my-sms | skip: no committed composer.lock | | | |
| gamebetr/provable |  | no |  | no |
| thoughtco/statamic-cp-resources | skip: no committed composer.lock | | | |
| gento-arg/module-oca | skip: no committed composer.lock | | | |
| reedware/laravel-api |  | no |  | no |
| symfony/skeleton | skip: no committed composer.lock | | | |
| api-platform/api-platform | skip: no committed composer.lock | | | |
| laminas/laminas-mvc-skeleton | skip: no committed composer.lock | | | |
| spiral/app | skip: no committed composer.lock | | | |
| codeigniter4/appstarter | skip: no committed composer.lock | | | |
| doctrine/orm | skip: no committed composer.lock | | | |
| phpmyadmin/phpmyadmin | dealerdirect/phpcodesniffer-composer-installer v1.2.1, phpstan/extension-installer 1.4.3 | yes |  | yes |
| matomo-org/matomo | dealerdirect/phpcodesniffer-composer-installer v1.2.1 | yes |  | yes |
| monicahq/monica | php-http/discovery 1.20.0, phpstan/extension-installer 1.4.3 | yes | post-update-cmd (static PHP callback/@php), post-autoload-dump (static PHP callback/@php) | yes |
| koel/koel | php-http/discovery 1.20.0 | yes | post-install-cmd (@php/@php/@php/@php), pre-update-cmd (@php), post-update-cmd (@php/@php), post-autoload-dump (static PHP callback/@php), post-root-package-install (@php), post-create-project-cmd (@php) | yes |
| pixelfed/pixelfed | pestphp/pest-plugin v4.0.0, php-http/discovery 1.20.0 | yes | post-install-cmd (@php), post-update-cmd (@php/@php), post-autoload-dump (static PHP callback/@php), post-root-package-install (@php), post-create-project-cmd (@php) | yes |
| BookStackApp/BookStack |  | no | pre-install-cmd (@php), post-install-cmd (@php/@php), post-autoload-dump (static PHP callback/@php), post-root-package-install (@php), post-create-project-cmd (@php) | yes |
| snipe/snipe-it | dealerdirect/phpcodesniffer-composer-installer v1.2.1 | yes | post-update-cmd (@php), post-autoload-dump (static PHP callback/@php/@php), post-create-project-cmd (shell command) | yes |
| mautic/mautic | composer/installers v2.3.0, composer/package-versions-deprecated 1.11.99.1, cweagans/composer-patches 1.7.3, phpstan/extension-installer 1.4.3 | yes | post-install-cmd (shell command/shell command/shell command/shell command/shell command/shell command), post-update-cmd (shell command/shell command/shell command/shell command/shell command/shell command) | yes |
| kimai/kimai | symfony/flex v2.11.0, symfony/runtime v6.4.41 | yes | post-install-cmd (shell command), post-update-cmd (shell command) | yes |
| firefly-iii/firefly-iii | php-http/discovery 1.20.0, phpstan/extension-installer 1.4.3 | yes | post-install-cmd (@php/@php), post-update-cmd (@php/@php/@php/@php/@php/@php/@php/@php), post-autoload-dump (static PHP callback), post-root-package-install (@php), post-create-project-cmd (@php) | yes |
| pterodactyl/panel |  | no | post-install-cmd (@php), post-autoload-dump (static PHP callback/@php), post-root-package-install (@php), post-create-project-cmd (@php/@php/@php) | yes |
| librenms/librenms | php-http/discovery 1.20.0 | yes | pre-install-cmd (static PHP callback), post-install-cmd (static PHP callback/static PHP callback/@php/@php/@php/@php), pre-update-cmd (static PHP callback), post-update-cmd (static PHP callback), post-autoload-dump (static PHP callback/@php), post-root-package-install (static PHP callback), post-create-project-cmd (@php) | yes |
| humhub/humhub | yiisoft/yii2-composer 2.0.11 | yes | post-create-project-cmd (static PHP callback) | yes |
| akaunting/akaunting | mnsami/composer-custom-directory-installer 2.0.0, php-http/discovery 1.20.0 | yes | post-install-cmd (shell command/shell command/shell command), post-update-cmd (shell command/shell command/shell command), post-autoload-dump (static PHP callback/@php), post-create-project-cmd (static PHP callback/shell command/shell command) | yes |

### Totals

- 14 of 20 projects lock at least one `type: composer-plugin` package.
- 14 of 20 projects' `config.allow-plugins` allows at least one plugin to run.
- `pre-install-cmd`: 2 handlers across the corpus (@php 1, static PHP callback 1).
- `post-install-cmd`: 27 handlers across the corpus (@php 14, shell command 11, static PHP callback 2).
- `pre-update-cmd`: 2 handlers across the corpus (@php 1, static PHP callback 1).
- `post-update-cmd`: 27 handlers across the corpus (@php 14, shell command 11, static PHP callback 2).
- `post-autoload-dump`: 18 handlers across the corpus (static PHP callback 9, @php 9).
- `post-root-package-install`: 6 handlers across the corpus (@php 5, static PHP callback 1).
- `post-create-project-cmd`: 13 handlers across the corpus (@php 8, shell command 3, static PHP callback 2).
- 13 of 20 projects define at least one of the nine script events.
- **16 of 20 projects run some code (a plugin or a script) during `viv install` today.**

## Reading

Of the 40 most-downloaded Composer plugins, code-generation 12, path-mapping 10, other 7, scaffolding 5, command-only 2, download-or-auth 2, patching 1, check-or-audit 1. Declarable: 25 yes, 6 partly, 6 no, 3 n/a (no install-time disk effect at all). viv already has a native adapter for 13 of them. Across the corpus, 14 of 20 projects lock a composer-plugin package, 14 of 20 allow at least one to run, 13 of 20 define at least one of the nine script events, and 16 of 20 run some code during `viv install` today, one way or the other.

Wall time: 182.9s.

