# Notices

viv is distributed under GPL-3.0-or-later (see `LICENSE`). It reimplements
Composer's behaviour, and each native plugin adapter in `src/plugins/` is a
port — function by function — of the Composer plugin named below. The
copyright notices of those projects apply to the Rust code ported from them.

Three of those plugins are GPL-2.0-or-later. Code ported from them is a
derivative work, which is why viv as a whole is GPL rather than MIT. Their
`or later` term is what permits GPL-3.0 here.

## Ported plugins

| Plugin | Upstream licence | Ported version | Adapter |
|---|---|---|---|
| [drupal/core-composer-scaffold](https://www.drupal.org/project/drupal) | **GPL-2.0-or-later** | 11.4.6 | `src/plugins/drupal_scaffold.rs` |
| [johnpbloch/wordpress-core-installer](https://github.com/johnpbloch/wordpress-core-installer) | **GPL-2.0-or-later** | 2.0.0 | `src/plugins/wordpress_core.rs` |
| [roots/wordpress-core-installer](https://github.com/roots/wordpress-core-installer) | **GPL-2.0-or-later** | v4.0.0 | `src/plugins/wordpress_core.rs` |
| [tbachert/spi](https://github.com/Nevay/spi) | Apache-2.0 | v1.0.5 | `src/plugins/spi.rs` |
| [cweagans/composer-patches](https://github.com/cweagans/composer-patches) | BSD-3-Clause | 2.0.0 | `src/plugins/patches.rs` |
| [yiisoft/yii2-composer](https://github.com/yiisoft/yii2-composer) | BSD-3-Clause | 2.0.11 | `src/plugins/yii2.rs` |
| [composer/installers](https://github.com/composer/installers) | MIT | v2.3.0 | `src/plugins/installers.rs` |
| [codeception/c3](https://github.com/Codeception/c3) | MIT | 2.9.0 | `src/plugins/c3.rs` |
| [craftcms/plugin-installer](https://github.com/craftcms/plugin-installer) | MIT | 1.6.0 | `src/plugins/craft.rs` |
| [ffraenz/private-composer-installer](https://github.com/ffraenz/private-composer-installer) | MIT | 5.0.1 | `src/plugins/private_installer.rs` |
| [pestphp/pest-plugin](https://github.com/pestphp/pest-plugin) | MIT | v5.0.0 | `src/plugins/pest.rs` |
| [php-http/discovery](https://github.com/php-http/discovery) | MIT | 1.20.0 | `src/plugins/discovery.rs` |
| [PHPCSStandards/composer-installer](https://github.com/PHPCSStandards/composer-installer) | MIT | v1.2.1 | `src/plugins/phpcs.rs` |
| [phpstan/extension-installer](https://github.com/phpstan/extension-installer) | MIT | 1.4.3 | `src/plugins/phpstan.rs` |
| [symfony/runtime](https://github.com/symfony/runtime) | MIT | v8.1.0 | `src/plugins/symfony_runtime.rs` |

`src/plugins/phpcs.rs` also ports `Config::setConfigData` from
[squizlabs/php_codesniffer](https://github.com/PHPCSStandards/PHP_CodeSniffer)
3.13.6 (BSD-3-Clause), so that writing `CodeSniffer.conf` needs no PHP
runtime. That is a second upstream for one adapter; see
[#226](https://github.com/svandragt/vivace/issues/226).

Each adapter's `upstream_version()` is the version its fixture was generated
from, and `.github/workflows/adapter-drift.yml` checks both the version and
the licence in this table against Packagist. See
[`docs/plugin-strategy.md`](docs/plugin-strategy.md) for the drift policy.

## Vendored files

These are copied verbatim rather than ported, and keep their own licences.
MIT permits their use in a GPL-licensed work.

| File | Origin | Licence |
|---|---|---|
| `src/autoload/templates/ClassLoader.php`, `InstalledVersions.php`, `LICENSE` | [composer/composer](https://github.com/composer/composer) | MIT |
| `src/spdx-licenses.json` | [composer/spdx-licenses](https://github.com/composer/spdx-licenses) | MIT |
| `tests/fixtures/composer/` | [composer/composer](https://github.com/composer/composer) | MIT |

## Why viv is GPL

viv was MIT until 2026-09-15. The three GPL-2.0-or-later ports above made
that incorrect: a port of GPL source is a derivative work and cannot be
distributed under MIT. Relicensing to GPL-3.0-or-later resolves it and keeps
every adapter. See
[#245](https://github.com/svandragt/vivace/issues/245) for the evidence, and
`CHANGELOG.md` for the release this took effect in.
