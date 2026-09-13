# Public projects swept for viv bugs

Projects tried once with `compat/run.sh` outside the pinned corpus, so the
next hunt starts from what has been covered. Result is the sweep's own
verdict for dev and no-dev; a refused plugin means the row ran with
`--no-plugins` on both sides. Add a section per hunt.

## 2026-09-08, seed 20260908b

| Project | Commit | dev | no-dev | Notes |
|---|---|---|---|---|
| typo3/cms-base-distribution | c374dbc | skipped | skipped | refused typo3/class-alias-loader,typo3/cms-composer-installers |
| cakephp/app | d9feb07 | identical | identical | refused cakephp/plugin-installer |
| contao/managed-edition | bc544bf | identical | identical | refused contao-components/installer,contao/manager-plugin |
| silverstripe/installer | 2156492 | identical | identical | refused silverstripe/recipe-plugin,silverstripe/vendor-plugin |
| shopware/template | d671d32 | viv error | viv error | lock generated; plugins: refused symfony/flex; The "symfony/flex" plugin was not loaded as plugins are disabled. shopware/conflicts: no dist entry and no git so; fixed by #149 |
| octobercms/october | 623a2b9 | identical | identical |  |
| wp-cli/wp-cli-bundle | 789ab57 | differs | identical | phpstan constraint compaction; fixed by #153 |
| bolt/project | 6f4aa21 | identical | identical | refused composer/package-versions-deprecated,drupol/composer-packages,symfony/flex |
| firstphp/ip2region | sample | identical | identical |  |
| whoa-php/flute | sample | skipped | skipped | composer failed |
| accessd/yii2-rollbar | sample | skipped | skipped | composer failed |
| tsg/ar | sample | skipped | skipped | composer failed |
| tumtum/oxid-inline-translator | sample | identical | identical |  |
| mozart/event-dispatcher | sample | identical | identical |  |
| ahmadarif/laravel-pagination | sample | skipped | skipped | platform |
| tokimikichika/text-analysis | sample | identical | identical |  |
| 8xprovn/microservice | sample | identical | identical |  |
| dmstr/api-configuration-bundle | sample | identical | identical |  |
| numesia/all-my-sms | sample | identical | identical |  |
| gamebetr/provable | sample | identical | identical |  |
| thoughtco/statamic-cp-resources | sample | identical | identical |  |
| gento-arg/module-oca | sample | skipped | skipped | composer failed |
| reedware/laravel-api | sample | identical | identical |  |

## 2026-09-13, update path (#201)

`composer update --no-install --no-scripts --no-plugins --no-interaction`
versus `viv update --no-install --no-scripts --no-plugins` on each project's
own `composer.json` (no committed lock used), both against real Packagist,
inside devbox. Lock result folds `_readme`/`plugin-api-version` through jq,
the same comparison `compat/run.sh`'s `lock_compare_one` (#180) uses.

| Project | Commit | Lock result | Notes |
|---|---|---|---|
| laravel/laravel | aa0cf12 | identical |  |
| symfony/skeleton | c7e48b6 | differs | `config.bump-after-update: true` (set in this skeleton): real Composer rewrites `composer.json`'s own `symfony/flex` constraint from `^2` to the resolved `^2.11` and bumps `content-hash` accordingly; viv leaves `composer.json` untouched. Same packages/versions resolved either way. Minimal repro: `{"require": {"monolog/monolog": "^3.0"}, "config": {"bump-after-update": true}}` — Composer rewrites the require to `^3.12` and reports "./composer.json has been updated"; viv's copy still reads `^3.0`, so the two `content-hash` values differ (`8d225dc9...` vs `5fb11b17...`). `bump-after-update` looks unimplemented in viv (no match for it anywhere in `src/`); filed as #205. |
| drupal/recommended-project | 5823c9f | identical |  |
| api-platform/api-platform | 5152cb1 | identical |  |
| laminas/laminas-mvc-skeleton | — | skipped | `composer update` itself failed: `require-dev` pulls `vimeo/psalm`, which caps PHP at `~8.3.0`; devbox's PHP is 8.4.24. Not a viv/composer divergence. |
| yiisoft/yii2-app-basic | 7f1be64 | identical |  |
| spiral/app | 04ae9df | identical |  |
| codeigniter4/appstarter | 8d252c8 | identical |  |
| slim/slim-skeleton (slimphp/Slim-Skeleton) | 0ef0154 | identical |  |
| doctrine/orm | 7d857bf | identical |  |

Ten projects attempted, none previously in this file: 8 identical, 1 differs,
1 skipped (platform, not tool disagreement). The one divergence is a
`composer.json`-mutation feature gap (`bump-after-update`), not a resolver
disagreement — package/version selection matched in every case, including
the differs row.
