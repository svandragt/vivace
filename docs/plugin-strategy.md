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
| composer/installers | Install path per package type from `extra.installer-paths` | Native (`src/plugins/installers.rs`), pure path mapping |
| johnpbloch/wordpress-core-installer | Install path of `wordpress-core` packages from `extra.wordpress-install-dir` | Native (`src/plugins/wordpress_core.rs`), pure path mapping |
| dealerdirect/phpcodesniffer-composer-installer | Points phpcs's `installed_paths` at every installed standard | Native (`src/plugins/phpcs.rs`); writes `CodeSniffer.conf` directly rather than shelling out to `phpcs --config-set`, so an install needs no PHP runtime (#218) |
| phpstan/extension-installer | Writes `GeneratedConfig.php` listing every `extra.phpstan` package | Native (`src/plugins/phpstan.rs`) |
| php-http/discovery | Adds packages to the resolver and generates a discovery file | Native (`src/plugins/discovery.rs`), `preAutoloadDump`/`extra.discovery` only; the resolver half (`postUpdate`) isn't ported |
| tbachert/spi | Generates a service-provider map file after autoload dump | Native (`src/plugins/spi.rs`), `extra.spi` only |
| cweagans/composer-patches | Applies patches from `extra.patches`/a patches file | Native (`src/plugins/patches.rs`), git-apply path only (#53) |
| yiisoft/yii2-composer | Writes `vendor/yiisoft/extensions.php` listing every `yii2-extension` package | Native (`src/plugins/yii2.rs`) (#92) |
| craftcms/plugin-installer | Writes `vendor/craftcms/plugins.php` listing every `craft-plugin` package | Native (`src/plugins/craft.rs`) (#92) |
| pestphp/pest-plugin | Writes `vendor/pest-plugins.json` listing every package's `extra.pest.plugins`, root last | Native (`src/plugins/pest.rs`) (#131) |
| ffraenz/private-composer-installer | Substitutes `{%NAME}`/`{%version}` placeholders in a dist URL from the environment/`.env` right before download | Native (`src/plugins/private_installer.rs`) (#98) |
| codeception/c3 | Copies its bundled `c3.php` into the project root on install/update, unless an existing, edited one is there | Native (`src/plugins/c3.rs`) (#126) |
| drupal/core-composer-scaffold | Copies scaffold files (`index.php`, `.htaccess`, `settings.php`, …) from every allowed package, manages `.gitignore`, writes `vendor/drupal/DrupalInstalled.php` and points the root classmap at it | Native (`src/plugins/drupal_scaffold.rs`) (#93) |
| drupal/core-project-message | Prints a message to stdout after `create-project`/`install`, no filesystem effect | Known inert — Composer prints a message viv does not |
| drupal/core-recipe-unpack | Unpacks a required `drupal-recipe` package's own dependencies into the root `composer.json` | Known inert for `install`/`update` — only subscribes to `POST_UPDATE_CMD`/`POST_CREATE_PROJECT_CMD` (via `composer require`/`create-project`), never plain `install` |
| symfony/runtime | Writes `vendor/autoload_runtime.php` from a fixed template plus `extra.runtime` options, after autoload dump | Native (`src/plugins/symfony_runtime.rs`) (#93) |
| altis/cms-installer | Copies `index.php`/`wp-config.php`/`.build-script` from `vendor/altis/cms/`, scaffolds `content/{plugins,themes}`/`.gitignore`, then writes `vendor/modules.php` listing every `extra.altis` package's `load.php` plus the root's own `extra.altis.modules.*.entrypoint` | Native (`src/plugins/altis_cms_installer.rs`) (#355) |
| altis/core | `Override_Installer`: a `wordpress-plugin`/`wordpress-muplugin` package named in any installed package's `extra.altis.install-overrides` installs at the default `vendor/<name>` instead of wherever `composer/installers` would place it | Native (`src/plugins/altis_core.rs`) (#355), pure path mapping, lock-wide |
| altis/dev-tools-command | Seeds `.travis.yml`/`.config/travis.yml` from `vendor/altis/dev-tools/travis/` if absent, then keeps `.travis.yml`'s pinned ref in sync with the installed `altis/dev-tools` version | Native (`src/plugins/altis_dev_tools_command.rs`) (#355) |
| altis/local-server | `activate()` conditionally `require`s a PHP file in-process for its own later use; subscribes to no events, only adds a `composer server` `CommandProvider` | Known inert — no disk effect at install (#355) |
| ion-bazan/composer-diff | Adds a `composer diff` `CommandProvider`, no event subscription | Known inert — command-only (#355) |

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
   a loud stop. The refusal also says whether `--no-plugins` is safe: plugins
   in `BY_DESIGN_REFUSALS` (`src/plugins/mod.rs`) get "by design, you lose
   nothing" with the one-clause reason from this page; everything else gets
   the generic "not adapted yet, this skips real work" (#224).

Plugins that `allow-plugins` sets to `false`, or that are absent from the map,
are ignored, as Composer ignores them.

## Data file or adapter

A plugin whose install-time effect is a path map, a file list or a patch
list is a data file in `src/plugins/data/`, not Rust code:

- `composer-installers.toml` — `composer/installers`
- `johnpbloch-wordpress-core-installer.toml` — `johnpbloch/wordpress-core-installer`
- `roots-wordpress-core-installer.toml` — `roots/wordpress-core-installer`
- `drupal-core-composer-scaffold.toml` — `drupal/core-composer-scaffold`
- `cweagans-composer-patches.toml` — `cweagans/composer-patches`

A plugin that generates code or fetches something stays an adapter in
`src/plugins/*.rs`:

- `discovery.rs` — code-generation (`php-http/discovery`, the generated
  discovery file)
- `symfony_runtime.rs` — code-generation (`symfony/runtime`'s
  `autoload_runtime.php`)
- `yii2.rs` — code-generation (`yiisoft/yii2-composer`'s `extensions.php`)
- `craft.rs` — code-generation (`craftcms/plugin-installer`'s `plugins.php`)
- `phpcs.rs` — code-generation (`dealerdirect/phpcodesniffer-composer-installer`'s
  `CodeSniffer.conf`)
- `phpstan.rs` — code-generation (`phpstan/extension-installer`'s
  `GeneratedConfig.php`)
- `pest.rs` — code-generation (`pestphp/pest-plugin`'s `pest-plugins.json`)
- `spi.rs` — code-generation (`tbachert/spi`'s `GeneratedServiceProviderData.php`)
- `c3.rs` — scaffolding that copies a bundled file (`codeception/c3`)
- `private_installer.rs` — download-or-auth (`ffraenz/private-composer-installer`
  substitutes a dist URL from the environment)
- `altis_cms_installer.rs` — scaffolding (fixed-name file copies, no
  `extra`-driven config of its own to bind a data file to) plus
  code-generation (`altis/cms-installer`'s `vendor/modules.php`)
- `altis_dev_tools_command.rs` — scaffolding plus a conditional rewrite
  (`altis/dev-tools-command`'s `.travis.yml`/`.config/travis.yml`)
- `altis_core.rs` — a path map in shape (`altis/core`'s `Override_Installer`),
  but not a `data/*.toml` rule: the override set is the union of every
  *other* installed package's `extra.altis.install-overrides`, not the root's
  or the target package's own `extra`, so it needs the whole lock
  (`super::resolve` computes it once) rather than the one-package,
  one-root-extra signature every `data.rs` rule answers from

The census of the 40 most-downloaded Composer plugins finds 25 of 40
declarable as data, 6 partly declarable and 6 not declarable at all; across
the compat corpus, 16 of 20 projects still run some code (a plugin or a
script) during `viv install` today. Full numbers and per-plugin evidence are
in [`compat/results/g3-plugins.md`](../compat/results/g3-plugins.md).

`config.allow-plugins` stays the gate: a plugin viv has neither a data file
nor an adapter for is reported and never run. `viv install` refuses with:

> No adapter exists for it yet, so the work the plugin would have done is
> skipped. Check what it writes before you rely on the result. (see
> docs/plugin-strategy.md, "Data file or adapter")

## Order of work

1. composer/installers and johnpbloch/wordpress-core-installer, plus the
   refusal in rule 3. Without these a WordPress project installs into the
   wrong directories. Done.
2. dealerdirect/phpcodesniffer-composer-installer, tbachert/spi,
   phpstan/extension-installer: post-install file or command generators. Done.
3. cweagans/composer-patches. Done (#53). bamarni/composer-bin-plugin is
   still refused.
4. ffraenz/private-composer-installer (#98): three local client projects
   need it for paid-plugin dist URLs. Done.
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

9. pestphp/pest-plugin (#131): writes `vendor/pest-plugins.json`, moving
   `roots/bedrock` from `plugins: refused` to `plugins: native`. Its
   `CommandProvider` capability isn't ported — it only matters to
   `composer pest:dump-plugins`, which viv never runs. Entries come in
   install order, not lock order, and the file is written only when the
   plugin is itself installed: a `--no-dev` install of a project that needs
   pest only for tests must write no file at all, as Composer writes none.

10. altis/cms-installer, altis/core, altis/dev-tools-command (#355), plus
    the known-inert classification for altis/local-server and
    ion-bazan/composer-diff: moves an Altis site from `plugins: refused`
    (five plugins, one per site) to `plugins: native`/known-inert, and lets
    `viv install` run without `--no-plugins` — which previously meant every
    `wordpress-plugin` landing in `vendor/` instead of `content/plugins/`,
    since `--no-plugins` also disables `composer/installers`. The issue's
    own starting guess had `altis/core` as inert ("module loading is
    runtime") and `altis/dev-tools-command` as command-only; neither held
    up against the plugin class at its pinned tag — see
    `src/plugins/altis_core.rs`'s and `altis_dev_tools_command.rs`'s own
    doc comments for what each actually does instead.

symfony/flex stays refused. Its value is in `composer require`, which is
where Symfony users should keep using Composer.

## Adding a native adapter

Check whether a data file covers it first (see "Data file or adapter"
above) — a path map, a file list or a patch list needs no Rust at all. For
example, a fixed install-path override is a `named-override` rule
(`src/plugins/data/roots-wordpress-core-installer.toml`):

```toml
kind = "named-override"
type = "wordpress-core"
key = "wordpress-install-dir"
```

Only a plugin that generates code or fetches something needs an adapter.
Every adapter is a unit struct in its own `src/plugins/<name>.rs` implementing
the internal `Adapter` trait (`src/plugins/mod.rs`): `plugin_names()` and
`upstream_version()` are required, and a default no-op covers every phase
method (`install_dir`, `fetch_env`, `state_fingerprint`, `post_link`,
`extra_classmap`, `pre_autoload_dump`, `post_autoload_dump`, `post_install`) —
implement only the ones the real plugin hooks. `Ctx` carries the `root`,
`project_dir` and `vendor_dir` a phase method needs (`post_link` also gets the
`Store`, for `cweagans/composer-patches`' relink-through-copy); no adapter
re-derives a path or reaches for a global.

Wiring one in is one line: add the module (`mod <name>;`), a variant on
`AdapterId`, its match arm in `make_adapter`, and its position in
`NATIVE_ADAPTERS` — the registration order every `Plugins` phase method loops
adapters in, and the order `tests::adapter_order_does_not_affect_the_wordpress_fixture`
(`src/plugins/mod.rs`) asserts output never depends on. `install.rs` never
names an adapter module directly; it only calls `Plugins`' own methods.

## Testing

Each adapter gets a fixture whose expected output is generated by real
Composer with the plugin enabled, in the same way as `tests/fixtures/monolog`.
The compat sweep reports refused plugins as `skipped: plugin <name>`.

## Upstream drift

An adapter is allowed to lag the Composer plugin it ports
(`upstream_version()`, one per adapter): its fixture, generated from the
pinned version, still proves the exact output it emits, whatever the plugin
has shipped since. `.github/workflows/adapter-drift.yml` checks every
adapter's pin against Packagist weekly and files or updates one issue naming
whichever have fallen behind, so a lag is a tracked, visible decision rather
than a silent one. Each row also names the fixture to re-check. That comes
from the adapter itself (`Adapter::fixture`), so it can't drift from the code.

The fallback stays refusal, never a guess: if re-porting an adapter to a
newer version would ever mean guessing at behaviour a fixture doesn't cover,
viv refuses that plugin instead of installing a package it can't prove
correct.
