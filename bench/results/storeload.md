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

## Reading the disk columns

Bytes are not measured, and neither is a count of distinct inodes. Under
the default hardlink mode a `vendor/` file *is* the store's file, one inode
under two names, so a bytes figure reports a saving that removing the tree
would not deliver. Counting distinct inodes is no better: each path under
`vendor/` resolves to one inode however many links it has, so that number
is just the dentry count again.

What the tree actually costs beyond the store is split in two:

- **dentries** — every path under `vendor/`. What the tree costs in
  directory entries whether or not its bytes are shared.
- **owned** — directories (never hardlinked) plus regular files with a link
  count of 1. Real blocks that only `vendor/` holds.
- **shared** — regular files with a link count above 1: one inode held
  jointly with the store, costing a directory entry and no data blocks.

`shared` equals the `linkat` count in every row, which is the check that
both measurements are sound: one successful hardlink, one shared file. And
`owned` comes out as the directory count on every project, so removing the
tree would reclaim directories and directory entries — not package bytes,
which the store keeps either way.

## 2026-09-21T16:12:27Z — control arm

viv 0.14.0, flags `--no-plugins --no-scripts` (same as compat/corpus.toml's own control flags), 3 runs each, from a local mirror (bench/mirror.sh, no real network in any timed scenario).

| Project | Packages | Cold | Warm | Boot | linkat (warm) | vendor/ dentries | vendor/ owned | vendor/ shared |
|---|---|---|---|---|---|---|---|---|
| laravel/laravel | 109 | 0.295 | 0.070 | pass: console artisan | 8834 | 10162 | 1328 | 8834 |
| symfony/demo | 153 | 0.201 | 0.049 | fail: Fatal error: Uncaught LogicException: Symfony Runtime is missing. Try running "composer require symfony/runtime". in /tmp/tmp.Kr2IbSmdSh/bench/viv/bin/console:12 | 11404 | 13201 | 1797 | 11404 |

- symfony/demo: boot failed: Fatal error: Uncaught LogicException: Symfony Runtime is missing. Try running "composer require symfony/runtime". in /tmp/tmp.Kr2IbSmdSh/bench/viv/bin/console:12
| drupal/recommended-project | 68 | 0.710 | 0.302 | none: no console or test entry ships without a configured site | 24312 | 31150 | 6838 | 24312 |
| roots/bedrock | 73 | 0.430 | 0.063 | pass: test vendor/bin/pest | 7059 | 8166 | 1107 | 7059 |
| composer/composer | 36 | 0.059 | 0.012 | pass: console bin/composer | 919 | 1177 | 258 | 919 |
| phpunit/phpunit | 26 | 0.072 | 0.049 | pass: console phpunit | 829 | 986 | 157 | 829 |
| slimphp/Slim-Skeleton | 57 | 0.157 | 0.040 | pass: test vendor/bin/phpunit | 4214 | 4917 | 703 | 4214 |
| yiisoft/yii2-app-basic | 93 | 0.244 | 0.041 | pass: console yii | 7852 | 9259 | 1407 | 7852 |
| statamic/statamic | 161 | 0.490 | 0.124 | pass: console please | 16324 | 18631 | 2307 | 16324 |
| craftcms/craft | 119 | 0.470 | 0.128 | pass: console craft | 15160 | 16829 | 1669 | 15160 |
