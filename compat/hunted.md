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
| shopware/template | d671d32 | viv error | viv error | lock generated; plugins: refused symfony/flex; The "symfony/flex" plugin was not loaded as plugins are disabled. shopware/conflicts: no dist entry and no git so |
| octobercms/october | 623a2b9 | identical | identical |  |
| wp-cli/wp-cli-bundle | 789ab57 | differs | identical | path-only GeneratedConfig.php diff the sweep failed to normalise |
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
