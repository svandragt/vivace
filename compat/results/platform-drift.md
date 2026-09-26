# Platform drift across the corpus (#309)

Candidate D's measurement (`docs/research.md`): for every committed lock in the compat corpus, does a locked package's own `require.php` refuse a PHP version the project's root `composer.json` claims to support -- drift, the only thing this chapter counts -- across the PHP minors from the lock's own floor up to the newest minor its CI matrix names? Static read of `composer.json`, `composer.lock` and `.github/workflows/*.yml` at a pinned commit, no installs, no registry solve. Method and the constraint checker: `compat/platform-drift.py`.

The lock's `php` floor is the highest lower bound of `require.php` across every locked package (packages and packages-dev separately; the prod floor is the deploy-relevant one). `ext-*` requirements are recorded (locked packages vs root) but never scored: a static read has no way to know which extensions a given PHP actually has loaded, only which version it is.

A refusal is one of two different things, and only one of them is drift. (i) The root `composer.json`'s own `require.php` already refuses a PHP: the project never claimed to support it, correct behaviour, not counted. (ii) Root admits a PHP but a locked package's own `require.php` refuses it anyway: drift. Drift is checked against the full install (packages and packages-dev), since a default `composer install`/`viv install` installs dev requirements too and that is what a CI matrix normally runs; a require-dev-only drift is marked `[dev]` so it can be read differently by a project that installs `--no-dev` in production. Each drift instance also names whether the failing bound is upper (the package caps out before this PHP, e.g. `~8.3.0` failing 8.5 -- the package hasn't caught up yet) or lower (the package needs newer than what's being tested, which shouldn't happen above the floor by construction, so a lower-bound drift there is a red flag on the method, not just a result).

A CI matrix value with only two segments (`"8.2"`) names a minor, not a patch: `setup-php` installs whichever 8.2.x patch is newest when CI actually runs, which this script cannot know after the fact. It is evaluated at the highest patch of that minor (`8.2.999`, a stand-in), since that is closer to reality than the literal `8.2.0` and is what a lower-bound constraint like `~8.2.27` needs to be correctly satisfied. A three-segment CI value (`"8.2.3"`) is an exact pin, tested literally. The floor's own previous minor is a separate sanity column, not a drift count: it is expected to refuse by construction (the floor is defined as the highest lower bound found), so a project where it doesn't would mean the floor computation itself is wrong.

A CI matrix that tests multiple historical branches of the project itself against different PHP versions (only `phpunit/phpunit`'s `nightly.yaml` in this corpus) still gets compared against the pinned commit's own lock for every PHP version it names, even on legs that actually run an older branch's own, different lock; read that row with that in mind.

Run: 2026-09-26T09:14:56Z. Corpus: `compat/corpus.toml` and `compat/hunted.md` (public projects only; anonymised local entries excluded).

## Corpus

| Project | Source | Commit |
|---|---|---|
| laravel/laravel | corpus.toml | `aa0cf127fc36` |
| symfony/demo | corpus.toml | `920d86dc809f` |
| drupal/recommended-project | corpus.toml | `-` |
| roots/bedrock | corpus.toml | `fb226251bf6d` |
| composer/composer | corpus.toml | `85ae0251528d` |
| phpunit/phpunit | corpus.toml | `2c534d37d8a5` |
| slimphp/Slim-Skeleton | corpus.toml | `0ef01549870b` |
| yiisoft/yii2-app-basic | corpus.toml | `575120b0949c` |
| statamic/statamic | corpus.toml | `d819487a4ce0` |
| craftcms/craft | corpus.toml | `a1747d5be2a3` |
| typo3/cms-base-distribution | hunted 2026-09-08 | `c374dbcd5683` |
| cakephp/app | hunted 2026-09-08 | `d9feb0725791` |
| contao/managed-edition | hunted 2026-09-08 | `bc544bf976fe` |
| silverstripe/installer | hunted 2026-09-08 | `2156492fb03b` |
| shopware/template | hunted 2026-09-08 | `d671d3246fa4` |
| octobercms/october | hunted 2026-09-08 | `623a2b96b714` |
| wp-cli/wp-cli-bundle | hunted 2026-09-08 | `789ab57db208` |
| bolt/project | hunted 2026-09-08 | `6f4aa21839f1` |
| firstphp/ip2region | hunted 2026-09-08 (sample) | `59c4b1b820db` |
| whoa-php/flute | hunted 2026-09-08 (sample) | `220d512f8271` |
| accessd/yii2-rollbar | hunted 2026-09-08 (sample) | `bf3bf37cf574` |
| tsg/ar | hunted 2026-09-08 (sample) | `HEAD` |
| tumtum/oxid-inline-translator | hunted 2026-09-08 (sample) | `4b4c050464db` |
| mozart/event-dispatcher | hunted 2026-09-08 (sample) | `f1bbd2d018b3` |
| ahmadarif/laravel-pagination | hunted 2026-09-08 (sample) | `de7fb7cfd33b` |
| tokimikichika/text-analysis | hunted 2026-09-08 (sample) | `98b78471fa83` |
| 8xprovn/microservice | hunted 2026-09-08 (sample) | `9f83a4eca413` |
| dmstr/api-configuration-bundle | hunted 2026-09-08 (sample) | `b3bc2d35c5d3` |
| numesia/all-my-sms | hunted 2026-09-08 (sample) | `cfb4da6a734a` |
| gamebetr/provable | hunted 2026-09-08 (sample) | `185bb6a6f474` |
| thoughtco/statamic-cp-resources | hunted 2026-09-08 (sample) | `b536a27010ae` |
| gento-arg/module-oca | hunted 2026-09-08 (sample) | `99e62e2e70a8` |
| reedware/laravel-api | hunted 2026-09-08 (sample) | `79246515ccdc` |
| symfony/skeleton | hunted 2026-09-13 | `c7e48b636a41` |
| api-platform/api-platform | hunted 2026-09-13 | `5152cb18965e` |
| laminas/laminas-mvc-skeleton | hunted 2026-09-13 | `bd22f39ecfd8` |
| spiral/app | hunted 2026-09-13 | `04ae9df43895` |
| codeigniter4/appstarter | hunted 2026-09-13 | `8d252c8f5949` |
| doctrine/orm | hunted 2026-09-13 | `7d857bf960f0` |
| phpmyadmin/phpmyadmin | hunted 2026-09-15 | `9e4dc5b5f41f` |
| matomo-org/matomo | hunted 2026-09-15 | `bdb35cfd7021` |
| monicahq/monica | hunted 2026-09-15 | `e08e91734170` |
| koel/koel | hunted 2026-09-15 | `8befe78c7900` |
| pixelfed/pixelfed | hunted 2026-09-15 | `472b4c4f40c5` |
| BookStackApp/BookStack | hunted 2026-09-15 | `b5641aa00d90` |
| snipe/snipe-it | hunted 2026-09-15 | `16362cc6a5cf` |
| mautic/mautic | hunted 2026-09-15 | `1f0a58c4d787` |
| kimai/kimai | hunted 2026-09-15 | `c7b8f18fbeb1` |
| firefly-iii/firefly-iii | hunted 2026-09-15 | `6143c0f14f49` |
| pterodactyl/panel | hunted 2026-09-15 | `113ea43d0d48` |
| librenms/librenms | hunted 2026-09-15 | `c0950bab2a46` |
| humhub/humhub | hunted 2026-09-15 | `ffb67017b4c4` |
| akaunting/akaunting | hunted 2026-09-15 | `50a029304028` |

## Skipped

| Project | Reason |
|---|---|
| laravel/laravel | no committed composer.lock |
| drupal/recommended-project | no installable git checkout (composer create-project, not a clone) |
| roots/bedrock | no committed composer.lock |
| slimphp/Slim-Skeleton | no committed composer.lock |
| yiisoft/yii2-app-basic | no committed composer.lock |
| statamic/statamic | no committed composer.lock |
| craftcms/craft | no committed composer.lock |
| typo3/cms-base-distribution | no committed composer.lock |
| cakephp/app | no committed composer.lock |
| contao/managed-edition | no committed composer.lock |
| silverstripe/installer | no committed composer.lock |
| shopware/template | no committed composer.lock |
| octobercms/october | no committed composer.lock |
| bolt/project | no committed composer.lock |
| firstphp/ip2region | no committed composer.lock |
| whoa-php/flute | no committed composer.lock |
| accessd/yii2-rollbar | no committed composer.lock |
| tsg/ar | clone failed: fatal: repository 'https://github.com/tsg/ar.git/' not found |
| tumtum/oxid-inline-translator | no committed composer.lock |
| mozart/event-dispatcher | no committed composer.lock |
| ahmadarif/laravel-pagination | no committed composer.lock |
| tokimikichika/text-analysis | no committed composer.lock |
| 8xprovn/microservice | no committed composer.lock |
| dmstr/api-configuration-bundle | no committed composer.lock |
| numesia/all-my-sms | no committed composer.lock |
| thoughtco/statamic-cp-resources | no committed composer.lock |
| gento-arg/module-oca | no committed composer.lock |
| symfony/skeleton | no committed composer.lock |
| api-platform/api-platform | no committed composer.lock |
| laminas/laminas-mvc-skeleton | no committed composer.lock |
| spiral/app | no committed composer.lock |
| codeigniter4/appstarter | no committed composer.lock |
| doctrine/orm | no committed composer.lock |

33 of 53 projects skipped: 32 with no committed lock to read, 1 whose clone failed.

## Per-project

Drift cells mark a CI-tested minor with `*`; an unmarked one is a fill-in between the floor and the newest CI minor that CI itself doesn't name. `[dev]` marks a packages-dev-only cause. Sanity is the floor's own previous minor: `refuses` is the expected case; `OK (!)` would flag a floor bug.

| Project | Prod floor | `config.platform.php` | CI PHP | Sanity (floor-1) | Root refuses (not drift) | Drift |
|---|---|---|---|---|---|---|
| symfony/demo | 8.4.1 | 8.4.1 | 8.4, 8.5 | 8.3: refuses | - | none |
| composer/composer | 7.2.5 | 7.2.5 | 5.6, 7.2, 7.3, 7.4, 8.0, 8.1, 8.2, 8.3, 8.4, 8.5, 8.6 | 7.1: refuses | 5.6 | none |
| phpunit/phpunit | 8.4 | 8.4.1 | 7.2, 7.3, 7.4, 8.0, 8.1, 8.2, 8.3, 8.4, 8.5, 8.6 | 8.3: refuses | 7.2, 7.3, 7.4, 8.0, 8.1, 8.2, 8.3 | none |
| wp-cli/wp-cli-bundle | 7.2.24 | 7.2.24 | 5.6, 7.1, 7.2, 7.4, 8.5 | 7.1: refuses | 5.6, 7.1 | none |
| gamebetr/provable | - (dev 7.3) | - | - | - | - | none |
| reedware/laravel-api | 7.1.8 | - | - | 7.0: refuses | - | none |
| phpmyadmin/phpmyadmin | 8.2 | 8.2.99 | 7.2, 8.2, 8.3, 8.4, 8.5, 8.6 | 8.1: refuses | 7.2 | 8.5*: lcobucci/clock (~8.2.0 || ~8.3.0 || ~8.4.0, upper, dev) | 8.6*: laminas/laminas-httphandlerrunner (~8.1.0 || ~8.2.0 || ~8.3.0 || ~8.4.0 || ~8.5.0, upper); laminas/laminas-diactoros (~8.2.0 || ~8.3.0 || ~8.4.0 || ~8.5.0, upper, dev); lcobucci/clock (~8.2.0 || ~8.3.0 || ~8.4.0, upper, dev); vimeo/psalm (~8.1.31 || ~8.2.27 || ~8.3.16 || ~8.4.3 || ~8.5.0, upper, dev) |
| matomo-org/matomo | 8.1 | 8.1.0 | 8.1 (+unresolved) | 8.0: refuses | - | none |
| monicahq/monica | 8.2 | - | 8.3 | 8.1: refuses | 8.2 | none |
| koel/koel | 8.3 | 8.3.0 | 8.3, 8.4 | 8.2: refuses | - | none |
| pixelfed/pixelfed | 8.3 | 8.3.0 | 8.4, 8.5 | 8.2: refuses | - | none |
| BookStackApp/BookStack | 8.2 | 8.2.0 | - | 8.1: refuses | - | none |
| snipe/snipe-it | 8.2 | - | 8.2, 8.3, 8.4, 8.5 | 8.1: refuses | - | none |
| mautic/mautic | 8.2 | 8.2.0 | 8.2, 8.3, 8.5 (+unresolved) | 8.1: refuses | - | 8.4: ezyang/htmlpurifier (~5.6.0 || ~7.0.0 || ~7.1.0 || ~7.2.0 || ~7.3.0 || ~7.4.0 || ~8.0.0 || ~8.1.0 || ~8.2.0 || ~8.3.0, upper) | 8.5*: ezyang/htmlpurifier (~5.6.0 || ~7.0.0 || ~7.1.0 || ~7.2.0 || ~7.3.0 || ~7.4.0 || ~8.0.0 || ~8.1.0 || ~8.2.0 || ~8.3.0, upper) |
| kimai/kimai | 8.2 | 8.2 | 8.2, 8.3, 8.4, 8.5 | 8.1: refuses | - | 8.5*: openspout/openspout (~8.2.0 || ~8.3.0 || ~8.4.0, upper); scheb/2fa-backup-code (~8.0.0 || ~8.1.0 || ~8.2.0 || ~8.3.0 || ~8.4.0, upper); scheb/2fa-bundle (~8.0.0 || ~8.1.0 || ~8.2.0 || ~8.3.0 || ~8.4.0, upper); scheb/2fa-totp (~8.0.0 || ~8.1.0 || ~8.2.0 || ~8.3.0 || ~8.4.0, upper) |
| firefly-iii/firefly-iii | 8.5 | 8.5 | unresolved | 8.4: refuses | - | none |
| pterodactyl/panel | 8.2 | 8.2.0 | 8.2, 8.3 | 8.1: refuses | - | none |
| librenms/librenms | 8.2 | - | 8.2, 8.4 | 8.1: refuses | - | none |
| humhub/humhub | 8.2 | 8.2 | 8.2, 8.3, 8.4 | 8.1: refuses | - | none |
| akaunting/akaunting | 8.1 | - | 8.1, 8.2, 8.3 | 8.0: refuses | - | none |

## Totals

- Projects examined: 20
- Projects skipped: 33 (32 no lock, 1 clone failed)
- Projects with drift on a CI-tested PHP: 3
  - of those, `config.platform.php` already set: 3
- Projects with drift anywhere root admits, floor up to the newest CI minor: 3
  - of those, `config.platform.php` already set: 3
- Drift instances: 7 in a prod package, 4 in a packages-dev-only package (dev deps normally install in CI too, so both are live failures there; only the prod count is a `--no-dev` production risk)
- Drift instances by bound: 11 upper (package capped below a newer PHP), 0 lower (shouldn't happen above the floor -- see sanity below), 0 mixed
- Sanity check: floor-1 unexpectedly did not refuse in 0 of 19 projects with a checkable floor (0 expected; a non-zero count would flag a bug in the floor computation)
- Projects with an unresolved CI PHP expression: 3
- Projects with an unparsed `php` constraint in the floor computation: 0

