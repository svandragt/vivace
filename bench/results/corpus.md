
## 2026-09-07T12:05:54Z

Partial run: stopped after the first project, with three build agents loading the machine, so the numbers are indicative only. Rerun with `make bench-corpus` on a quiet machine (#106).

viv 0.6.0, composer 2.10.2, riff 0.0.7, flags `--no-plugins --no-scripts`, 3 runs each.

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| laravel/laravel | 109 | composer | 8.324 | 1.537 | 0.925 | 1.819 |
| laravel/laravel | 109 | riff | 4.093 | 0.707 | 0.654 | n/a |
| laravel/laravel | 109 | viv | 3.312 | 0.080 | 0.008 | 8.714 |

## 2026-09-07T15:30:24Z

viv 0.6.0, composer 2.10.2, riff 0.0.7, flags `--no-plugins --no-scripts`, 3 runs each.

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| laravel/laravel | 109 | composer | 9.255 | 1.503 | 0.927 | 1.830 |
| laravel/laravel | 109 | riff | 5.596 | 0.557 | 0.380 | n/a |
| laravel/laravel | 109 | viv | 3.325 | 0.096 | 0.010 | 2.063 |
| symfony/demo | 153 | composer | 9.651 | 1.346 | 0.501 | 1.687 |
| symfony/demo | 153 | riff | 3.224 | 0.296 | 0.245 | 0.616 |
| symfony/demo | 153 | viv | 2.691 | 0.048 | 0.010 | 2.376 |
| drupal/recommended-project | 68 | composer | 8.532 | 2.076 | 0.414 | 2.582 |
| drupal/recommended-project | 68 | riff | 3.948 | 1.358 | 1.276 | n/a |
| drupal/recommended-project | 68 | viv | 4.349 | 0.326 | 0.015 | n/a |
| roots/bedrock | 73 | composer | 44.647 | 1.521 | 0.553 | 2.398 |
| roots/bedrock | 73 | riff | 3.546 | 0.449 | 0.214 | n/a |
| roots/bedrock | 73 | viv | 3.532 | 0.062 | 0.093 | n/a |
| composer/composer | 36 | composer | 4.809 | 0.827 | 0.403 | 0.663 |
| composer/composer | 36 | riff | 2.021 | 0.219 | 0.297 | 0.262 |
| composer/composer | 36 | viv | 1.478 | 0.012 | 0.004 | 0.433 |
| phpunit/phpunit | 26 | composer | 4.468 | 0.891 | 0.520 | 0.615 |
| phpunit/phpunit | 26 | riff | 10.519 | 9.508 | 0.275 | n/a |
| phpunit/phpunit | 26 | viv | 1.875 | 0.057 | 0.005 | n/a |
| slimphp/Slim-Skeleton | 57 | composer | 5.605 | 1.046 | 0.460 | 0.781 |
| slimphp/Slim-Skeleton | 57 | riff | 1.668 | 0.221 | 0.215 | n/a |
| slimphp/Slim-Skeleton | 57 | viv | 2.070 | 0.041 | 0.006 | 0.403 |
| yiisoft/yii2-app-basic | 93 | composer | 8.752 | 1.408 | 0.529 | n/a |

Run aborted after yiisoft/yii2-app-basic's composer row: `bench/run.sh`
reported a problem for that project and `corpus.sh` exited before writing
the riff and viv rows or any footnotes, so the `n/a` reasons above (viv
update-warm on drupal, bedrock, phpunit; riff update-warm) are not
recorded for this run. viv is the post-0.6.0 main build (the binary still
reports 0.6.0).
