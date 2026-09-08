# Plugin strategy

Composer plugins are PHP classes that hook into Composer's own process.
viv has no PHP runtime, so it can never run them as written. This page
records how viv treats them and why.

## Inventory

Plugins found in the lock files available on 2026-09-06 (four public
projects, the Laravel benchmark lock, two test fixtures and one private
WordPress project):

| Plugin | What it changes | Portable? |
|---|---|---|
| composer/installers | Install path per package type from `extra.installer-paths` | Yes, pure path mapping |
| johnpbloch/wordpress-core-installer | Install path of `wordpress-core` packages from `extra.wordpress-install-dir` | Yes, pure path mapping |
| dealerdirect/phpcodesniffer-composer-installer | Runs `phpcs --config-set installed_paths` after install | Native (`src/plugins/phpcs.rs`) |
| phpstan/extension-installer | Writes `GeneratedConfig.php` listing every `extra.phpstan` package | Native (`src/plugins/phpstan.rs`) |
| php-http/discovery | Adds packages to the resolver and generates a discovery file | Native (`src/plugins/discovery.rs`), `preAutoloadDump`/`extra.discovery` only; the resolver half (`postUpdate`) isn't ported |
| tbachert/spi | Generates a service-provider map file after autoload dump | Native (`src/plugins/spi.rs`), `extra.spi` only |
| cweagans/composer-patches | Applies patches from `extra.patches`/a patches file | Native (`src/plugins/patches.rs`), git-apply path only (#53) |
| yiisoft/yii2-composer | Writes `vendor/yiisoft/extensions.php` listing every `yii2-extension` package | Native (`src/plugins/yii2.rs`) (#92) |
| craftcms/plugin-installer | Writes `vendor/craftcms/plugins.php` listing every `craft-plugin` package | Native (`src/plugins/craft.rs`) (#92) |
| ffraenz/private-composer-installer | Substitutes `{%NAME}`/`{%version}` placeholders in a dist URL from the environment/`.env` right before download | Native (`src/plugins/private_installer.rs`) (#98) — the substitution itself is fully ported and tested against `fetch::Fetcher` directly, but reaching it from `viv install` needs a one-line `Fetcher::private_installer(...)` builder call in `install.rs`, not yet wired (a file another agent owns as of this writing) |
| codeception/c3 | Copies its bundled `c3.php` into the project root on install/update, unless an existing, edited one is there | Native (`src/plugins/c3.rs`) (#126) |
| drupal/core-composer-scaffold | Copies scaffold files (`index.php`, `.htaccess`, `settings.php`, …) from every allowed package, manages `.gitignore`, writes `vendor/drupal/DrupalInstalled.php` and points the root classmap at it | Native (`src/plugins/drupal_scaffold.rs`) (#93) |
| drupal/core-project-message | Prints a message to stdout after `create-project`/`install`, no filesystem effect | Known inert — Composer prints a message viv does not |
| drupal/core-recipe-unpack | Unpacks a required `drupal-recipe` package's own dependencies into the root `composer.json` | Known inert for `install`/`update` — only subscribes to `POST_UPDATE_CMD`/`POST_CREATE_PROJECT_CMD` (via `composer require`/`create-project`), never plain `install` |
| symfony/runtime | Writes `vendor/autoload_runtime.php` from a fixed template plus `extra.runtime` options, after autoload dump | Native (`src/plugins/symfony_runtime.rs`) (#93) |

Plugins named in issue #12 but not seen in any lock yet: symfony/flex
(rewrites composer.json and recipes, not portable), bamarni/composer-bin-plugin
(nested installs, portable by running viv in each `vendor-bin/*`).

## Rule

For every package of type `composer-plugin` in the lock that is enabled by
`config.allow-plugins`:

1. **Native adapter exists**: viv applies the equivalent behaviour itself.
   Output must be byte-identical to Composer running the real plugin.
2. **Known inert**: the plugin only affects Composer's own commands that viv
   does not implement (for example ergebnis/composer-normalize). viv ignores it
   silently. The list lives in code.
3. **Anything else**: viv refuses with an error naming the plugin and pointing
   at this page. `--no-plugins` turns the refusal into a warning and installs
   as Composer would with `--no-plugins`. Silent wrong installs are worse than
   a loud stop.

Plugins that `allow-plugins` sets to `false`, or that are absent from the map,
are ignored, as Composer ignores them.

## Order of work

1. composer/installers and johnpbloch/wordpress-core-installer, plus the
   refusal in rule 3. Without these a WordPress project installs into the
   wrong directories. Done.
2. dealerdirect/phpcodesniffer-composer-installer, tbachert/spi,
   phpstan/extension-installer: post-install file or command generators. Done.
3. cweagans/composer-patches. Done (#53). bamarni/composer-bin-plugin is
   still refused.
4. ffraenz/private-composer-installer (#98): three local client projects
   need it for paid-plugin dist URLs. The placeholder substitution is done
   and tested at the `fetch::Fetcher` level; `install.rs` still needs to
   build a `Fetcher` with `.private_installer(...)` when
   `Plugins::has_private_installer` is set, or a project using this plugin
   installs with the plugin recognised (no refusal) but every placeholder
   left literal in the dist URL it actually requests, which 404s.
5. php-http/discovery (#101): the generated file only. Done. Its resolver
   hook (`postUpdate`) is still out of scope, even with the resolver
   shipped, since it needs `viv update` to treat the discovered packages as
   additional requirements, not just a file to write.
6. yiisoft/yii2-composer and craftcms/plugin-installer (#92): the generated
   file only. Done.
7. codeception/c3 (#126): copies `c3.php` into the project root. Done.
   Removing `c3.php` when `codeception/c3` itself is uninstalled isn't
   ported — there's no `plan.remove`-side hook this reaches today.
8. drupal/core-composer-scaffold and symfony/runtime (#93), plus the
   known-inert classification for drupal/core-project-message and
   drupal/core-recipe-unpack: moves `drupal/recommended-project` from
   `plugins: refused` to `plugins: native` in the compat sweep. Not ported:
   `ScaffoldOptions::symlink()` (copy only, no symlink mode) and
   `Handler::scaffold`'s unchanged-file skip (see the ponytail notes in
   `src/plugins/drupal_scaffold.rs`).

symfony/flex stays refused. Its value is in `composer require`, which is
where Symfony users should keep using Composer.

## Testing

Each adapter gets a fixture whose expected output is generated by real
Composer with the plugin enabled, in the same way as `tests/fixtures/monolog`.
The compat sweep reports refused plugins as `skipped: plugin <name>`.
