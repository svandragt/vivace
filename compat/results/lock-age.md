# Lock age across the corpus (#307)

Measured 2026-09-26T12:52:45Z. Candidate B (`docs/research.md`): for every project in `compat/corpus.toml` and the public projects in `compat/hunted.md` (via `compat/platform-drift.py`'s `CORPUS`, minus a few large or already-all-skip repos named in `compat/lock-age.py`), the last commit touching `composer.lock` at or before 1, 2 and 4 years before the run date. `viv install --no-scripts --ignore-platform-reqs` and the control `composer install --no-scripts --no-plugins --ignore-platform-reqs` from that lock, no update, each in an isolated, empty cache. The network is what's measured, not a timing variable: a rerun on another date can classify differently.

Platform class is static: `compat/platform-drift.py`'s `_package_drift`/`satisfies` against the devbox `php -v` actually running this measurement, not a version sweep.

## Corpus

| Project | Age | Commit | Commit date | Result |
|---|---|---|---|---|
| laravel/laravel | 1yr | d15ab4b82ed3 | 2015-10-14 | measured |
| laravel/laravel | 2yr | d15ab4b82ed3 | 2015-10-14 | same commit as the 1yr row |
| laravel/laravel | 4yr | d15ab4b82ed3 | 2015-10-14 | same commit as the 1yr row |
| symfony/demo | 1yr | aede8b54a712 | 2025-08-15 | measured |
| symfony/demo | 2yr | e4ef1f8d0360 | 2024-07-19 | measured |
| symfony/demo | 4yr | db0a2759e3d4 | 2022-08-04 | measured |
| roots/bedrock | 1yr | 0c02cb9e8a0c | 2025-09-24 | measured |
| roots/bedrock | 2yr | 6acbb2420eac | 2024-09-11 | measured |
| roots/bedrock | 4yr | a70e2b04acfc | 2022-08-31 | measured |
| composer/composer | 1yr | 8fc94c5e9972 | 2025-09-17 | measured |
| composer/composer | 2yr | 6b81140f81a4 | 2024-09-21 | measured |
| composer/composer | 4yr | 4f0419059262 | 2022-09-14 | measured |
| phpunit/phpunit | 1yr | 9f79830b3076 | 2025-09-24 | measured |
| phpunit/phpunit | 2yr | 9109547f3e73 | 2024-09-17 | measured |
| phpunit/phpunit | 4yr | - | - | no composer.lock commit on or before 2022-09-26 |
| slimphp/Slim-Skeleton | 1yr | f12402f42504 | 2015-12-07 | measured |
| slimphp/Slim-Skeleton | 2yr | f12402f42504 | 2015-12-07 | same commit as the 1yr row |
| slimphp/Slim-Skeleton | 4yr | f12402f42504 | 2015-12-07 | same commit as the 1yr row |
| yiisoft/yii2-app-basic | 1yr | 3be9b8507dc1 | 2016-07-25 | measured |
| yiisoft/yii2-app-basic | 2yr | 3be9b8507dc1 | 2016-07-25 | same commit as the 1yr row |
| yiisoft/yii2-app-basic | 4yr | 3be9b8507dc1 | 2016-07-25 | same commit as the 1yr row |
| typo3/cms-base-distribution | 1yr | - | - | no composer.lock commit on or before 2025-09-26 |
| typo3/cms-base-distribution | 2yr | - | - | no composer.lock commit on or before 2024-09-26 |
| typo3/cms-base-distribution | 4yr | - | - | no composer.lock commit on or before 2022-09-26 |
| cakephp/app | 1yr | 6aec4a64b270 | 2019-12-10 | measured |
| cakephp/app | 2yr | 6aec4a64b270 | 2019-12-10 | same commit as the 1yr row |
| cakephp/app | 4yr | 6aec4a64b270 | 2019-12-10 | same commit as the 1yr row |
| wp-cli/wp-cli-bundle | 1yr | 2633466a8827 | 2025-09-15 | measured |
| wp-cli/wp-cli-bundle | 2yr | 2fd885705c64 | 2024-08-19 | measured |
| wp-cli/wp-cli-bundle | 4yr | 38dab66ad7b3 | 2022-09-19 | measured |
| symfony/skeleton | 1yr | - | - | no composer.lock commit on or before 2025-09-26 |
| symfony/skeleton | 2yr | - | - | no composer.lock commit on or before 2024-09-26 |
| symfony/skeleton | 4yr | - | - | no composer.lock commit on or before 2022-09-26 |
| api-platform/api-platform | 1yr | d82159a320a6 | 2017-09-21 | measured |
| api-platform/api-platform | 2yr | d82159a320a6 | 2017-09-21 | same commit as the 1yr row |
| api-platform/api-platform | 4yr | d82159a320a6 | 2017-09-21 | same commit as the 1yr row |
| laminas/laminas-mvc-skeleton | 1yr | - | - | no composer.lock commit on or before 2025-09-26 |
| laminas/laminas-mvc-skeleton | 2yr | - | - | no composer.lock commit on or before 2024-09-26 |
| laminas/laminas-mvc-skeleton | 4yr | - | - | no composer.lock commit on or before 2022-09-26 |
| spiral/app | 1yr | 96a5e786d1cc | 2021-05-03 | measured |
| spiral/app | 2yr | 96a5e786d1cc | 2021-05-03 | same commit as the 1yr row |
| spiral/app | 4yr | 96a5e786d1cc | 2021-05-03 | same commit as the 1yr row |
| codeigniter4/appstarter | 1yr | 72493f131efb | 2019-01-19 | measured |
| codeigniter4/appstarter | 2yr | 72493f131efb | 2019-01-19 | same commit as the 1yr row |
| codeigniter4/appstarter | 4yr | 72493f131efb | 2019-01-19 | same commit as the 1yr row |
| doctrine/orm | 1yr | b13b2e8bab81 | 2020-10-28 | measured |
| doctrine/orm | 2yr | b13b2e8bab81 | 2020-10-28 | same commit as the 1yr row |
| doctrine/orm | 4yr | b13b2e8bab81 | 2020-10-28 | same commit as the 1yr row |
| phpmyadmin/phpmyadmin | 1yr | 79830ce9cdc9 | 2025-09-12 | measured |
| phpmyadmin/phpmyadmin | 2yr | ec977df5cd07 | 2024-09-20 | measured |
| phpmyadmin/phpmyadmin | 4yr | 9dfedff70b3f | 2022-05-11 | measured |
| monicahq/monica | 1yr | dd527db56663 | 2025-08-09 | measured |
| monicahq/monica | 2yr | f7402830554e | 2024-06-15 | measured |
| monicahq/monica | 4yr | 225d7fef8e65 | 2022-09-23 | measured |
| BookStackApp/BookStack | 1yr | 64b06bcf6184 | 2025-08-30 | measured |
| BookStackApp/BookStack | 2yr | 9f68ca535872 | 2024-08-26 | measured |
| BookStackApp/BookStack | 4yr | f4388d5e4a63 | 2022-09-22 | measured |
| snipe/snipe-it | 1yr | 61df3bc46231 | 2025-09-16 | measured |
| snipe/snipe-it | 2yr | 0b3ac2a9cd21 | 2024-08-14 | measured |
| snipe/snipe-it | 4yr | 443b1df5e182 | 2022-07-22 | measured |
| firefly-iii/firefly-iii | 1yr | d3c557ca2255 | 2025-09-26 | measured |
| firefly-iii/firefly-iii | 2yr | 8938622bd948 | 2024-09-23 | measured |
| firefly-iii/firefly-iii | 4yr | ff55b36f320d | 2022-09-26 | measured |
| pterodactyl/panel | 1yr | 955dd2796d1f | 2024-11-14 | measured |
| pterodactyl/panel | 2yr | 1d38b4f0e201 | 2023-02-23 | measured |
| pterodactyl/panel | 4yr | 80ae600fe1d1 | 2022-06-26 | measured |

40 project-commit pairs measured, 26 skipped.

## Per-pair outcome

| Project | Age | Platform | viv | composer | vendor/ |
|---|---|---|---|---|---|
| laravel/laravel | 1yr | ok | installs | installs | differs |
| symfony/demo | 1yr | ok | installs | installs | identical |
| symfony/demo | 2yr | ok | installs | installs | identical |
| symfony/demo | 4yr | ok | installs | installs | identical |
| roots/bedrock | 1yr | ok | installs | installs | identical |
| roots/bedrock | 2yr | ok | installs | installs | identical |
| roots/bedrock | 4yr | ok | installs | installs | identical |
| composer/composer | 1yr | ok | installs | installs | differs |
| composer/composer | 2yr | ok | installs | installs | identical |
| composer/composer | 4yr | ok | installs | installs | identical |
| phpunit/phpunit | 1yr | ok | installs | installs | identical |
| phpunit/phpunit | 2yr | ok | installs | installs | differs |
| slimphp/Slim-Skeleton | 1yr | ok | installs | installs | differs |
| yiisoft/yii2-app-basic | 1yr | refuses (codeception/base requires php >=5.4.0 <8.0; fzaninotto/faker requires php ^5.3.3|^7.0; phpspec/prophecy requires php ^5.3|^7.0; phpunit/php-code-coverage requires php ^5.6 || ^7.0; phpunit/phpunit requires php ^5.6 || ^7.0) | installs | installs | differs |
| cakephp/app | 1yr | refuses (aura/intl requires php ^5.6|^7.0; symfony/config requires php ^7.1.3; symfony/console requires php ^7.1.3; symfony/filesystem requires php ^7.1.3; symfony/service-contracts requires php ^7.2.5) | installs | installs | identical |
| wp-cli/wp-cli-bundle | 1yr | ok | installs | installs | identical |
| wp-cli/wp-cli-bundle | 2yr | ok | installs | installs | identical |
| wp-cli/wp-cli-bundle | 4yr | ok | installs | installs | identical |
| api-platform/api-platform | 1yr | refuses (composer/ca-bundle requires php ^5.3.2 || ^7.0; doctrine/annotations requires php ^7.1; doctrine/cache requires php ~7.1; doctrine/collections requires php ^7.1; doctrine/common requires php ~7.1) | installs | installs | identical |
| spiral/app | 1yr | refuses (laminas/laminas-diactoros requires php ^7.3 || ~8.0.0; laminas/laminas-hydrator requires php ^7.3 || ~8.0.0; phpspec/prophecy requires php ^7.2 || ~8.0, <8.1) | installs | installs | identical |
| codeigniter4/appstarter | 1yr | refuses (zendframework/zend-escaper requires php ^5.6 || ^7.0; doctrine/instantiator requires php ^7.1; myclabs/deep-copy requires php ^7.1; phar-io/manifest requires php ^5.6 || ^7.0; phar-io/version requires php ^5.6 || ^7.0) | installs | fails | - |
| doctrine/orm | 1yr | ok | installs | installs | identical |
| phpmyadmin/phpmyadmin | 1yr | ok | installs | installs | identical |
| phpmyadmin/phpmyadmin | 2yr | ok | installs | installs | identical |
| phpmyadmin/phpmyadmin | 4yr | ok | installs | installs | differs |
| monicahq/monica | 1yr | ok | installs | installs | identical |
| monicahq/monica | 2yr | refuses (inertiajs/inertia-laravel requires php ^7.3|~8.0.0|~8.1.0|~8.2.0|~8.3.0; lcobucci/clock requires php ~8.2.0 || ~8.3.0; moneyphp/money requires php ~8.1.0 || ~8.2.0 || ~8.3.0; nette/schema requires php 8.1 - 8.3; nette/utils requires php >=8.0 <8.4) | installs | installs | identical |
| monicahq/monica | 4yr | refuses (fgrosse/phpasn1 requires php ~7.1.0 || ~7.2.0 || ~7.3.0 || ~7.4.0 || ~8.0.0 || ~8.1.0; inertiajs/inertia-laravel requires php ^7.2|~8.0.0|~8.1.0; nette/schema requires php >=7.1 <8.2; nette/utils requires php >=7.2 <8.2) | installs | installs | identical |
| BookStackApp/BookStack | 1yr | ok | installs | installs | identical |
| BookStackApp/BookStack | 2yr | ok | installs | installs | identical |
| BookStackApp/BookStack | 4yr | ok | installs | installs | identical |
| snipe/snipe-it | 1yr | ok | installs | installs | identical |
| snipe/snipe-it | 2yr | refuses (lcobucci/jwt requires php ~8.1.0 || ~8.2.0 || ~8.3.0; nette/schema requires php 8.1 - 8.3; nette/utils requires php >=8.0 <8.4; phpspec/prophecy requires php ^7.2 || 8.0.* || 8.1.* || 8.2.* || 8.3.*; brianium/paratest requires php ~8.1.0 || ~8.2.0 || ~8.3.0) | installs | installs | differs |
| snipe/snipe-it | 4yr | refuses (nette/schema requires php >=7.1 <8.2; nette/utils requires php >=7.2 <8.2; phpspec/prophecy requires php ^7.2 || ~8.0, <8.2; overtrue/phplint requires php ^5.5.9 || ^7.0) | installs | installs | identical |
| firefly-iii/firefly-iii | 1yr | ok | installs | installs | identical |
| firefly-iii/firefly-iii | 2yr | refuses (lcobucci/clock requires php ~8.2.0 || ~8.3.0; lcobucci/jwt requires php ~8.1.0 || ~8.2.0 || ~8.3.0; nette/schema requires php 8.1 - 8.3; ergebnis/phpstan-rules requires php ~8.1.0 || ~8.2.0 || ~8.3.0) | installs | installs | differs |
| firefly-iii/firefly-iii | 4yr | ok | installs | installs | identical |
| pterodactyl/panel | 1yr | ok | installs | installs | identical |
| pterodactyl/panel | 2yr | ok | installs | installs | identical |
| pterodactyl/panel | 4yr | ok | installs | installs | identical |

## Package failure classes

| Age | Class | Packages |
|---|---|---|

Tree-hash-saves: 0 of 0 dist-related failed packages (across 0 pairs) have a source reference still fetchable, the case a lock carrying tree hashes would install from. A package failure classed `other` (a validation rule tightened since the lock was written, an aborted/unparsed install) isn't a dist problem and is excluded from this count even where its source happens to still be fetchable.

## viv-vs-Composer differences (not counted as viv bugs unless filed)

- codeigniter4/appstarter 1yr: Composer fails, viv installs: codeigniter4/framework, doctrine/instantiator, kint-php/kint, mikey179/vfsStream, myclabs/deep-copy, phar-io/manifest, phar-io/version, phpdocumentor/reflection-common, phpdocumentor/reflection-docblock, phpdocumentor/type-resolver, phpspec/prophecy, phpunit/php-code-coverage, phpunit/php-file-iterator, phpunit/php-text-template, phpunit/php-timer, phpunit/php-token-stream, phpunit/phpunit, psr/log, sebastian/code-unit-reverse-lookup, sebastian/comparator, sebastian/diff, sebastian/environment, sebastian/exporter, sebastian/global-state, sebastian/object-enumerator, sebastian/object-reflector, sebastian/recursion-context, sebastian/resource-operations, sebastian/version, symfony/polyfill-ctype, theseer/tokenizer, webmozart/assert, zendframework/zend-escaper

## Vendor byte differences (8 of 40 measured pairs)

Not counted as viv bugs unless filed; listed as potential compat issues, checked by hand this run rather than by the classifier above (the task only classifies install failures, not vendor-tree content):

- `vendor/composer/installed.json`/`installed.php`'s `time` field: Composer writes ISO 8601 (`2015-06-28T21:39:13+00:00`), viv writes `2015-06-28 21:39:13` -- the majority of the differs rows below.
- a legacy mixed-case Packagist name (`jeremeamia/SuperClosure`, predating today's lowercase-only naming rule) installs at `vendor/jeremeamia/SuperClosure` under Composer and `vendor/jeremeamia/superclosure` under viv (`laravel/laravel` 1yr).
- `phpunit/phpunit`'s own test suite deliberately declares ambiguous duplicate class names as end-to-end fixtures; the two tools' classmaps pick a different one of the two files for the ambiguous entry.

laravel/laravel 1yr, composer/composer 1yr, phpunit/phpunit 2yr, slimphp/Slim-Skeleton 1yr, yiisoft/yii2-app-basic 1yr, phpmyadmin/phpmyadmin 4yr, snipe/snipe-it 2yr, firefly-iii/firefly-iii 2yr

