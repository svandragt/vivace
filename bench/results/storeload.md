# Store-load and boot (#287)

Chapter 2 of `docs/research.md`: does a project boot after `viv install`, and
what does the linked `vendor/` tree cost against the store it already
shares inodes with. Corpus is `compat/corpus.toml`'s ten pinned projects,
run through `bench/storeload.sh`.

**Only the control arm (compat mode, the linked `vendor/` tree) is measured
below.** The flagged arm — autoload maps pointing at the store with a
minimal `vendor/` (#284, #285) — doesn't exist yet, so there is nothing to
put in a second column; adding one now would imply a result that hasn't
been produced. Re-run `bench/storeload.sh` once #284/#285 land to add it.

## 2026-09-21T15:50:13Z — control arm

viv 0.14.0, flags `--no-plugins --no-scripts` (same as compat/corpus.toml's own control flags), 3 runs each, from a local mirror (bench/mirror.sh, no real network in any timed scenario).

| Project | Packages | Cold | Warm | Boot | linkat (warm) | vendor/ dentries | vendor/ inodes |
|---|---|---|---|---|---|---|---|
| laravel/laravel | 109 | 0.292 | 0.072 | pass: console artisan | 8834 | 10162 | 10162 |
| symfony/demo | 153 | 0.201 | 0.049 | fail: Fatal error: Uncaught LogicException: Symfony Runtime is missing. Try running "composer require symfony/runtime". in /tmp/tmp.p8QMKjdk7d/bench/viv/bin/console:12 | 11404 | 13201 | 13201 |

- symfony/demo: boot failed: Fatal error: Uncaught LogicException: Symfony Runtime is missing. Try running "composer require symfony/runtime". in /tmp/tmp.p8QMKjdk7d/bench/viv/bin/console:12
| drupal/recommended-project | 68 | 0.692 | 0.309 | none: no console or test entry ships without a configured site | 24312 | 31150 | 31150 |
| roots/bedrock | 73 | 0.439 | 0.063 | pass: test vendor/bin/pest | 7059 | 8166 | 8166 |
| composer/composer | 36 | 0.060 | 0.012 | pass: console bin/composer | 919 | 1177 | 1177 |
| phpunit/phpunit | 26 | 0.073 | 0.050 | pass: console phpunit | 829 | 986 | 986 |
| slimphp/Slim-Skeleton | 57 | 0.155 | 0.039 | pass: test vendor/bin/phpunit | 4214 | 4917 | 4917 |
| yiisoft/yii2-app-basic | 93 | 0.248 | 0.041 | pass: console yii | 7852 | 9259 | 9259 |
| statamic/statamic | 161 | 0.499 | 0.123 | pass: console please | 16324 | 18631 | 18631 |
| craftcms/craft | 119 | 0.468 | 0.126 | pass: console craft | 15160 | 16829 | 16829 |
