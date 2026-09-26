# Lock age across the corpus (#307)

Measured 2026-09-26T13:09:21Z. Candidate B (`docs/research.md`): for every project in `compat/corpus.toml` and the public projects in `compat/hunted.md` (via `compat/platform-drift.py`'s `CORPUS`, minus a few large or already-all-skip repos named in `compat/lock-age.py`), the last commit touching `composer.lock` at or before 1, 2 and 4 years before 2026-09-26, the date the corpus was queried. That cutoff is how a pair was *sampled*, not its age: a project that stopped committing its lock returns the same, older commit for every cutoff still on or after it, so the tables below bucket every pair by the commit's real age against that date instead. `viv install --no-scripts --ignore-platform-reqs` and the control `composer install --no-scripts --no-plugins --ignore-platform-reqs` from that lock, no update, each in an isolated, empty cache. The network is what's measured, not a timing variable: a rerun on another date can classify differently.

Platform class is static: `compat/platform-drift.py`'s `_package_drift`/`satisfies` against the devbox `php -v` actually running this measurement, not a version sweep.

## Corpus

| Project | Age | Commit | Commit date | Result |
|---|---|---|---|---|
| laravel/laravel | 7yr+ | d15ab4b82ed3 | 2015-10-14 | measured |
| laravel/laravel | 7yr+ | d15ab4b82ed3 | 2015-10-14 | same commit as the 1yr row |
| laravel/laravel | 7yr+ | d15ab4b82ed3 | 2015-10-14 | same commit as the 1yr row |
| symfony/demo | 2-3yr | aede8b54a712 | 2025-08-15 | measured |
| symfony/demo | 2-3yr | e4ef1f8d0360 | 2024-07-19 | measured |
| symfony/demo | 4-6yr | db0a2759e3d4 | 2022-08-04 | measured |
| roots/bedrock | 2-3yr | 0c02cb9e8a0c | 2025-09-24 | measured |
| roots/bedrock | 2-3yr | 6acbb2420eac | 2024-09-11 | measured |
| roots/bedrock | 4-6yr | a70e2b04acfc | 2022-08-31 | measured |
| composer/composer | 2-3yr | 8fc94c5e9972 | 2025-09-17 | measured |
| composer/composer | 2-3yr | 6b81140f81a4 | 2024-09-21 | measured |
| composer/composer | 4-6yr | 4f0419059262 | 2022-09-14 | measured |
| phpunit/phpunit | 2-3yr | 9f79830b3076 | 2025-09-24 | measured |
| phpunit/phpunit | 2-3yr | 9109547f3e73 | 2024-09-17 | measured |
| phpunit/phpunit | - | - | - | no composer.lock commit on or before 2022-09-26 |
| slimphp/Slim-Skeleton | 7yr+ | f12402f42504 | 2015-12-07 | measured |
| slimphp/Slim-Skeleton | 7yr+ | f12402f42504 | 2015-12-07 | same commit as the 1yr row |
| slimphp/Slim-Skeleton | 7yr+ | f12402f42504 | 2015-12-07 | same commit as the 1yr row |
| yiisoft/yii2-app-basic | 7yr+ | 3be9b8507dc1 | 2016-07-25 | measured |
| yiisoft/yii2-app-basic | 7yr+ | 3be9b8507dc1 | 2016-07-25 | same commit as the 1yr row |
| yiisoft/yii2-app-basic | 7yr+ | 3be9b8507dc1 | 2016-07-25 | same commit as the 1yr row |
| typo3/cms-base-distribution | - | - | - | no composer.lock commit on or before 2025-09-26 |
| typo3/cms-base-distribution | - | - | - | no composer.lock commit on or before 2024-09-26 |
| typo3/cms-base-distribution | - | - | - | no composer.lock commit on or before 2022-09-26 |
| cakephp/app | 7yr+ | 6aec4a64b270 | 2019-12-10 | measured |
| cakephp/app | 7yr+ | 6aec4a64b270 | 2019-12-10 | same commit as the 1yr row |
| cakephp/app | 7yr+ | 6aec4a64b270 | 2019-12-10 | same commit as the 1yr row |
| wp-cli/wp-cli-bundle | 2-3yr | 2633466a8827 | 2025-09-15 | measured |
| wp-cli/wp-cli-bundle | 2-3yr | 2fd885705c64 | 2024-08-19 | measured |
| wp-cli/wp-cli-bundle | 4-6yr | 38dab66ad7b3 | 2022-09-19 | measured |
| symfony/skeleton | - | - | - | no composer.lock commit on or before 2025-09-26 |
| symfony/skeleton | - | - | - | no composer.lock commit on or before 2024-09-26 |
| symfony/skeleton | - | - | - | no composer.lock commit on or before 2022-09-26 |
| api-platform/api-platform | 7yr+ | d82159a320a6 | 2017-09-21 | measured |
| api-platform/api-platform | 7yr+ | d82159a320a6 | 2017-09-21 | same commit as the 1yr row |
| api-platform/api-platform | 7yr+ | d82159a320a6 | 2017-09-21 | same commit as the 1yr row |
| laminas/laminas-mvc-skeleton | - | - | - | no composer.lock commit on or before 2025-09-26 |
| laminas/laminas-mvc-skeleton | - | - | - | no composer.lock commit on or before 2024-09-26 |
| laminas/laminas-mvc-skeleton | - | - | - | no composer.lock commit on or before 2022-09-26 |
| spiral/app | 4-6yr | 96a5e786d1cc | 2021-05-03 | measured |
| spiral/app | 4-6yr | 96a5e786d1cc | 2021-05-03 | same commit as the 1yr row |
| spiral/app | 4-6yr | 96a5e786d1cc | 2021-05-03 | same commit as the 1yr row |
| codeigniter4/appstarter | 7yr+ | 72493f131efb | 2019-01-19 | measured |
| codeigniter4/appstarter | 7yr+ | 72493f131efb | 2019-01-19 | same commit as the 1yr row |
| codeigniter4/appstarter | 7yr+ | 72493f131efb | 2019-01-19 | same commit as the 1yr row |
| doctrine/orm | 4-6yr | b13b2e8bab81 | 2020-10-28 | measured |
| doctrine/orm | 4-6yr | b13b2e8bab81 | 2020-10-28 | same commit as the 1yr row |
| doctrine/orm | 4-6yr | b13b2e8bab81 | 2020-10-28 | same commit as the 1yr row |
| phpmyadmin/phpmyadmin | 2-3yr | 79830ce9cdc9 | 2025-09-12 | measured |
| phpmyadmin/phpmyadmin | 2-3yr | ec977df5cd07 | 2024-09-20 | measured |
| phpmyadmin/phpmyadmin | 4-6yr | 9dfedff70b3f | 2022-05-11 | measured |
| monicahq/monica | 2-3yr | dd527db56663 | 2025-08-09 | measured |
| monicahq/monica | 2-3yr | f7402830554e | 2024-06-15 | measured |
| monicahq/monica | 4-6yr | 225d7fef8e65 | 2022-09-23 | measured |
| BookStackApp/BookStack | 2-3yr | 64b06bcf6184 | 2025-08-30 | measured |
| BookStackApp/BookStack | 2-3yr | 9f68ca535872 | 2024-08-26 | measured |
| BookStackApp/BookStack | 4-6yr | f4388d5e4a63 | 2022-09-22 | measured |
| snipe/snipe-it | 2-3yr | 61df3bc46231 | 2025-09-16 | measured |
| snipe/snipe-it | 2-3yr | 0b3ac2a9cd21 | 2024-08-14 | measured |
| snipe/snipe-it | 4-6yr | 443b1df5e182 | 2022-07-22 | measured |
| firefly-iii/firefly-iii | <=1yr | d3c557ca2255 | 2025-09-26 | measured |
| firefly-iii/firefly-iii | 2-3yr | 8938622bd948 | 2024-09-23 | measured |
| firefly-iii/firefly-iii | 4-6yr | ff55b36f320d | 2022-09-26 | measured |
| pterodactyl/panel | 2-3yr | 955dd2796d1f | 2024-11-14 | measured |
| pterodactyl/panel | 4-6yr | 1d38b4f0e201 | 2023-02-23 | measured |
| pterodactyl/panel | 4-6yr | 80ae600fe1d1 | 2022-06-26 | measured |

40 project-commit pairs measured, 26 skipped.
40 of 40 measured pairs are distinct project+commit pairs (0 duplicate).
Oldest lock measured: laravel/laravel at 2015-10-14 (d15ab4b82ed3).

## Per-pair outcome

| Project | Age | Commit | Platform | viv | composer | vendor/ |
|---|---|---|---|---|---|---|
| laravel/laravel | 7yr+ | d15ab4b82ed3 | ok | installs | installs | differs |
| symfony/demo | 2-3yr | aede8b54a712 | ok | installs | installs | identical |
| symfony/demo | 2-3yr | e4ef1f8d0360 | ok | installs | installs | identical |
| symfony/demo | 4-6yr | db0a2759e3d4 | ok | installs | installs | identical |
| roots/bedrock | 2-3yr | 0c02cb9e8a0c | ok | installs | installs | identical |
| roots/bedrock | 2-3yr | 6acbb2420eac | ok | installs | installs | identical |
| roots/bedrock | 4-6yr | a70e2b04acfc | ok | installs | installs | identical |
| composer/composer | 2-3yr | 8fc94c5e9972 | ok | installs | installs | differs |
| composer/composer | 2-3yr | 6b81140f81a4 | ok | installs | installs | identical |
| composer/composer | 4-6yr | 4f0419059262 | ok | installs | installs | identical |
| phpunit/phpunit | 2-3yr | 9f79830b3076 | ok | installs | installs | identical |
| phpunit/phpunit | 2-3yr | 9109547f3e73 | ok | installs | installs | differs |
| slimphp/Slim-Skeleton | 7yr+ | f12402f42504 | ok | installs | installs | differs |
| yiisoft/yii2-app-basic | 7yr+ | 3be9b8507dc1 | refuses (codeception/base requires php >=5.4.0 <8.0; fzaninotto/faker requires php ^5.3.3|^7.0; phpspec/prophecy requires php ^5.3|^7.0; phpunit/php-code-coverage requires php ^5.6 || ^7.0; phpunit/phpunit requires php ^5.6 || ^7.0) | installs | installs | differs |
| cakephp/app | 7yr+ | 6aec4a64b270 | refuses (aura/intl requires php ^5.6|^7.0; symfony/config requires php ^7.1.3; symfony/console requires php ^7.1.3; symfony/filesystem requires php ^7.1.3; symfony/service-contracts requires php ^7.2.5) | installs | installs | identical |
| wp-cli/wp-cli-bundle | 2-3yr | 2633466a8827 | ok | installs | installs | identical |
| wp-cli/wp-cli-bundle | 2-3yr | 2fd885705c64 | ok | installs | installs | identical |
| wp-cli/wp-cli-bundle | 4-6yr | 38dab66ad7b3 | ok | installs | installs | identical |
| api-platform/api-platform | 7yr+ | d82159a320a6 | refuses (composer/ca-bundle requires php ^5.3.2 || ^7.0; doctrine/annotations requires php ^7.1; doctrine/cache requires php ~7.1; doctrine/collections requires php ^7.1; doctrine/common requires php ~7.1) | installs | installs | identical |
| spiral/app | 4-6yr | 96a5e786d1cc | refuses (laminas/laminas-diactoros requires php ^7.3 || ~8.0.0; laminas/laminas-hydrator requires php ^7.3 || ~8.0.0; phpspec/prophecy requires php ^7.2 || ~8.0, <8.1) | installs | installs | identical |
| codeigniter4/appstarter | 7yr+ | 72493f131efb | refuses (zendframework/zend-escaper requires php ^5.6 || ^7.0; doctrine/instantiator requires php ^7.1; myclabs/deep-copy requires php ^7.1; phar-io/manifest requires php ^5.6 || ^7.0; phar-io/version requires php ^5.6 || ^7.0) | installs | fails | - |
| doctrine/orm | 4-6yr | b13b2e8bab81 | ok | installs | installs | identical |
| phpmyadmin/phpmyadmin | 2-3yr | 79830ce9cdc9 | ok | installs | installs | identical |
| phpmyadmin/phpmyadmin | 2-3yr | ec977df5cd07 | ok | installs | installs | identical |
| phpmyadmin/phpmyadmin | 4-6yr | 9dfedff70b3f | ok | installs | installs | differs |
| monicahq/monica | 2-3yr | dd527db56663 | ok | installs | installs | identical |
| monicahq/monica | 2-3yr | f7402830554e | refuses (inertiajs/inertia-laravel requires php ^7.3|~8.0.0|~8.1.0|~8.2.0|~8.3.0; lcobucci/clock requires php ~8.2.0 || ~8.3.0; moneyphp/money requires php ~8.1.0 || ~8.2.0 || ~8.3.0; nette/schema requires php 8.1 - 8.3; nette/utils requires php >=8.0 <8.4) | installs | installs | identical |
| monicahq/monica | 4-6yr | 225d7fef8e65 | refuses (fgrosse/phpasn1 requires php ~7.1.0 || ~7.2.0 || ~7.3.0 || ~7.4.0 || ~8.0.0 || ~8.1.0; inertiajs/inertia-laravel requires php ^7.2|~8.0.0|~8.1.0; nette/schema requires php >=7.1 <8.2; nette/utils requires php >=7.2 <8.2) | installs | installs | identical |
| BookStackApp/BookStack | 2-3yr | 64b06bcf6184 | ok | installs | installs | identical |
| BookStackApp/BookStack | 2-3yr | 9f68ca535872 | ok | installs | installs | identical |
| BookStackApp/BookStack | 4-6yr | f4388d5e4a63 | ok | installs | installs | identical |
| snipe/snipe-it | 2-3yr | 61df3bc46231 | ok | installs | installs | identical |
| snipe/snipe-it | 2-3yr | 0b3ac2a9cd21 | refuses (lcobucci/jwt requires php ~8.1.0 || ~8.2.0 || ~8.3.0; nette/schema requires php 8.1 - 8.3; nette/utils requires php >=8.0 <8.4; phpspec/prophecy requires php ^7.2 || 8.0.* || 8.1.* || 8.2.* || 8.3.*; brianium/paratest requires php ~8.1.0 || ~8.2.0 || ~8.3.0) | installs | installs | differs |
| snipe/snipe-it | 4-6yr | 443b1df5e182 | refuses (nette/schema requires php >=7.1 <8.2; nette/utils requires php >=7.2 <8.2; phpspec/prophecy requires php ^7.2 || ~8.0, <8.2; overtrue/phplint requires php ^5.5.9 || ^7.0) | installs | installs | identical |
| firefly-iii/firefly-iii | <=1yr | d3c557ca2255 | ok | installs | installs | identical |
| firefly-iii/firefly-iii | 2-3yr | 8938622bd948 | refuses (lcobucci/clock requires php ~8.2.0 || ~8.3.0; lcobucci/jwt requires php ~8.1.0 || ~8.2.0 || ~8.3.0; nette/schema requires php 8.1 - 8.3; ergebnis/phpstan-rules requires php ~8.1.0 || ~8.2.0 || ~8.3.0) | installs | installs | differs |
| firefly-iii/firefly-iii | 4-6yr | ff55b36f320d | ok | installs | installs | identical |
| pterodactyl/panel | 2-3yr | 955dd2796d1f | ok | installs | installs | identical |
| pterodactyl/panel | 4-6yr | 1d38b4f0e201 | ok | installs | installs | identical |
| pterodactyl/panel | 4-6yr | 80ae600fe1d1 | ok | installs | installs | identical |

## Package failure classes

| Age | Class | Packages |
|---|---|---|

Tree-hash-saves: 0 of 0 dist-related failed packages (across 0 pairs) have a source reference still fetchable, the case a lock carrying tree hashes would install from. A package failure classed `other` (a validation rule tightened since the lock was written, an aborted/unparsed install) isn't a dist problem and is excluded from this count even where its source happens to still be fetchable.

## viv-vs-Composer differences (not counted as viv bugs unless filed)

- codeigniter4/appstarter 72493f131efb (2019-01-19): Composer fails, viv installs: codeigniter4/framework, doctrine/instantiator, kint-php/kint, mikey179/vfsStream, myclabs/deep-copy, phar-io/manifest, phar-io/version, phpdocumentor/reflection-common, phpdocumentor/reflection-docblock, phpdocumentor/type-resolver, phpspec/prophecy, phpunit/php-code-coverage, phpunit/php-file-iterator, phpunit/php-text-template, phpunit/php-timer, phpunit/php-token-stream, phpunit/phpunit, psr/log, sebastian/code-unit-reverse-lookup, sebastian/comparator, sebastian/diff, sebastian/environment, sebastian/exporter, sebastian/global-state, sebastian/object-enumerator, sebastian/object-reflector, sebastian/recursion-context, sebastian/resource-operations, sebastian/version, symfony/polyfill-ctype, theseer/tokenizer, webmozart/assert, zendframework/zend-escaper

## Vendor byte differences (8 of 40 measured pairs)

Not counted as viv bugs unless filed. Grouped by which files differed (`vendor_diff_detail` in `lock-age.raw.jsonl`), then checked by hand against a fresh install of one pair per group, 2026-09-26 (the task classifies install failures, not vendor-tree content, so this grouping is a manual read, not the classifier above):

- **ambiguous classmap entry (duplicate class name)** (1): phpunit/phpunit 9109547f3e73 (2024-09-17)
- **installed.json time format** (2): slimphp/Slim-Skeleton f12402f42504 (2015-12-07), yiisoft/yii2-app-basic 3be9b8507dc1 (2016-07-25)
- **installed.php only, unexplained** (4): composer/composer 8fc94c5e9972 (2025-09-17), phpmyadmin/phpmyadmin 9dfedff70b3f (2022-05-11), snipe/snipe-it 0b3ac2a9cd21 (2024-08-14), firefly-iii/firefly-iii 8938622bd948 (2024-09-23)
- **legacy mixed-case package name (vendor dir case)** (1): laravel/laravel d15ab4b82ed3 (2015-10-14)

Exact reproduction, one pair per confirmed cause (checked by hand, not reproducible purely from the jsonl -- vendor content isn't stored):

- `installed.json` time format -- `yiisoft/yii2-app-basic` `3be9b8507dc1` (2016-07-25), `vendor/composer/installed.json`, package `behat/gherkin` v4.4.1. The lock's own `time` for that package is `"2015-12-30 14:47:00"`. Composer's `installed.json` writes `"time": "2015-12-30T14:47:00+00:00"`; viv's writes `"time": "2015-12-30 14:47:00"` -- unchanged from the lock, where Composer reformats to ISO 8601.
- legacy mixed-case package name -- `laravel/laravel` `d15ab4b82ed3` (2015-10-14), `vendor/jeremeamia/`. The lock names the package `"jeremeamia/SuperClosure"`. Composer installs it at `vendor/jeremeamia/SuperClosure`; viv installs it at `vendor/jeremeamia/superclosure`.
- phpunit ambiguous classmap entry -- `phpunit/phpunit` `9109547f3e73` (2024-09-17), `vendor/composer/autoload_classmap.php`, class `PHPUnit\TestFixture\AlternativeSuffixTest` (one of several; phpunit's own end-to-end tests declare duplicate fixture class names on purpose). Composer's classmap points it at `tests/end-to-end/_files/coverage-annotation-based-filter/tests/AnnotationFilterTest.php`; viv's points it at `tests/_files/AlternativeSuffixTest.test.php`.
- the `installed.php`-only group did not reproduce: re-installing `composer/composer` `8fc94c5e9972` and `phpmyadmin/phpmyadmin` `9dfedff70b3f` fresh on 2026-09-26 produced byte-identical `installed.php` on both sides. `installed.php` carries no `time` field, so the time-format explanation doesn't apply to it; the original difference isn't classified.

