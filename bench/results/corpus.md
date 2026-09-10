
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

## 2026-09-07T17:45:59Z

viv 0.6.0, composer 2.10.2, riff 0.0.7, flags `--no-plugins --no-scripts`, 3 runs each.

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| laravel/laravel | 109 | composer | 8.370 | 1.472 | 0.845 | 1.762 |
| laravel/laravel | 109 | riff | 3.504 | 0.965 | 0.378 | n/a |
| laravel/laravel | 109 | viv | 3.334 | 0.070 | 0.008 | 1.927 |

- laravel/laravel/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| symfony/demo | 153 | composer | 10.539 | 1.309 | 0.478 | 1.898 |
| symfony/demo | 153 | riff | 2.812 | 0.538 | 0.416 | 0.914 |
| symfony/demo | 153 | viv | 3.077 | 0.048 | 0.010 | 2.430 |
| drupal/recommended-project | 68 | composer | 8.602 | 2.105 | 0.410 | 2.504 |
| drupal/recommended-project | 68 | riff | 3.397 | 1.290 | 1.585 | n/a |
| drupal/recommended-project | 68 | viv | 4.424 | 0.306 | 0.016 | 1.517 |

- drupal/recommended-project/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| roots/bedrock | 73 | composer | 43.494 | 2.005 | 0.563 | 2.334 |
| roots/bedrock | 73 | riff | 3.422 | 0.452 | 0.234 | n/a |
| roots/bedrock | 73 | viv | 4.306 | 0.063 | 0.096 | 2.842 |

- roots/bedrock/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| composer/composer | 36 | composer | 4.776 | 0.811 | 0.397 | 0.654 |
| composer/composer | 36 | riff | 1.117 | 0.218 | 0.228 | 0.256 |
| composer/composer | 36 | viv | 1.649 | 0.012 | 0.004 | 0.424 |
| phpunit/phpunit | 26 | composer | 4.791 | 0.869 | 0.507 | 0.619 |
| phpunit/phpunit | 26 | riff | 7.951 | 7.528 | 0.413 | n/a |
| phpunit/phpunit | 26 | viv | 1.177 | 0.053 | 0.004 | 0.295 |

- phpunit/phpunit/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| slimphp/Slim-Skeleton | 57 | composer | 6.152 | 1.030 | 0.468 | 0.737 |
| slimphp/Slim-Skeleton | 57 | riff | 1.534 | 0.223 | 0.217 | n/a |
| slimphp/Slim-Skeleton | 57 | viv | 2.006 | 0.039 | 0.005 | 0.409 |

- slimphp/Slim-Skeleton/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| yiisoft/yii2-app-basic | 93 | composer | 9.180 | 1.162 | 0.491 | n/a |
| yiisoft/yii2-app-basic | 93 | riff | n/a | n/a | n/a | n/a |
| yiisoft/yii2-app-basic | 93 | viv | n/a | n/a | n/a | n/a |

- yiisoft/yii2-app-basic/composer: Error: Command terminated with non-zero exit code 1 in the first benchmark run. Use the '-i'/'--ignore-failure' option if you want to ignore this. Alternatively, use the '--show-output' option to debug what went wrong.
- yiisoft/yii2-app-basic/riff: Error: Command terminated with non-zero exit code 1 in the first benchmark run. Use the '-i'/'--ignore-failure' option if you want to ignore this. Alternatively, use the '--show-output' option to debug what went wrong.
- yiisoft/yii2-app-basic/viv: Error: Command terminated with non-zero exit code 1 in the first benchmark run. Use the '-i'/'--ignore-failure' option if you want to ignore this. Alternatively, use the '--show-output' option to debug what went wrong.
| statamic/statamic | 160 | composer | 11.602 | 2.187 | 1.135 | 2.459 |
| statamic/statamic | 160 | riff | 5.218 | 0.736 | 0.561 | n/a |
| statamic/statamic | 160 | viv | 3.481 | 0.123 | 0.010 | n/a |

- statamic/statamic/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
- statamic/statamic/viv: warning: viv update-warm failed, skipping (see bench/results/README.md)
| craftcms/craft | 118 | composer | 8.826 | 1.849 | 1.073 | 2.103 |
| craftcms/craft | 118 | riff | 4.295 | 0.678 | 0.450 | 0.717 |
| craftcms/craft | 118 | viv | 3.562 | 0.124 | 0.008 | n/a |

- craftcms/craft/viv: warning: viv update-warm failed, skipping (see bench/results/README.md)

## 2026-09-07T18:57:05Z

viv 0.6.0, composer 2.10.2, riff 0.0.7, flags `--no-plugins --no-scripts`, 3 runs each.

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| laravel/laravel | 109 | composer | 7.952 | 1.479 | 0.872 | 1.763 |
| laravel/laravel | 109 | riff | 4.146 | 0.534 | 0.395 | n/a |
| laravel/laravel | 109 | viv | 3.036 | 0.070 | 0.008 | 1.768 |

- laravel/laravel/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| symfony/demo | 153 | composer | 9.658 | 1.340 | 0.482 | 1.770 |
| symfony/demo | 153 | riff | 2.695 | 0.316 | 0.248 | 1.105 |
| symfony/demo | 153 | viv | 2.642 | 0.051 | 0.009 | 1.825 |
| drupal/recommended-project | 68 | composer | 9.085 | 2.060 | 0.412 | 2.848 |
| drupal/recommended-project | 68 | riff | 4.023 | 1.259 | 1.295 | n/a |
| drupal/recommended-project | 68 | viv | 3.890 | 0.302 | 0.015 | 1.137 |

- drupal/recommended-project/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| roots/bedrock | 73 | composer | 36.205 | 1.568 | 0.554 | 2.450 |
| roots/bedrock | 73 | riff | 3.853 | 0.480 | 0.221 | n/a |
| roots/bedrock | 73 | viv | 3.443 | 0.065 | 0.095 | 1.556 |

- roots/bedrock/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| composer/composer | 36 | composer | 4.437 | 0.812 | 0.404 | 0.650 |
| composer/composer | 36 | riff | 1.184 | 0.237 | 0.251 | 0.246 |
| composer/composer | 36 | viv | 1.316 | 0.012 | 0.005 | 0.385 |
| phpunit/phpunit | 26 | composer | 4.428 | 0.872 | 0.504 | 0.699 |
| phpunit/phpunit | 26 | riff | 8.025 | 9.848 | 0.210 | n/a |
| phpunit/phpunit | 26 | viv | 1.131 | 0.053 | 0.004 | 0.258 |

- phpunit/phpunit/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| slimphp/Slim-Skeleton | 57 | composer | 5.875 | 1.056 | 0.437 | 0.759 |
| slimphp/Slim-Skeleton | 57 | riff | 1.766 | 0.215 | 0.211 | n/a |
| slimphp/Slim-Skeleton | 57 | viv | 1.915 | 0.039 | 0.005 | 0.344 |

- slimphp/Slim-Skeleton/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| yiisoft/yii2-app-basic | 93 | composer | 9.832 | 1.202 | 0.494 | n/a |
| yiisoft/yii2-app-basic | 93 | riff | n/a | n/a | n/a | n/a |
| yiisoft/yii2-app-basic | 93 | viv | n/a | n/a | n/a | n/a |

- yiisoft/yii2-app-basic/composer: -p1: failed to apply patch to src/Iterator.php: error applying hunk #1
- yiisoft/yii2-app-basic/riff: run.sh: riff install failed:
- yiisoft/yii2-app-basic/viv: -p1: failed to apply patch to src/Iterator.php: error applying hunk #1
| statamic/statamic | 160 | composer | 10.885 | 2.142 | 1.146 | 2.462 |
| statamic/statamic | 160 | riff | 3.648 | 0.732 | 0.559 | n/a |
| statamic/statamic | 160 | viv | 3.682 | 0.121 | 0.010 | 4.656 |

- statamic/statamic/riff: warning: riff update-warm failed, skipping (see bench/results/README.md)
| craftcms/craft | 118 | composer | 8.939 | 1.822 | 1.112 | 2.152 |
| craftcms/craft | 118 | riff | 4.186 | 0.645 | 0.620 | 0.606 |
| craftcms/craft | 118 | viv | 3.608 | 0.125 | 0.008 | 1.367 |

## 2026-09-07T20:01:03Z

viv 0.6.0, composer 2.10.2, riff 0.0.7, flags `--no-plugins --no-scripts`, 3 runs each.

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| laravel/laravel | 109 | composer | 7.780 | 1.483 | 0.850 | 1.745 |
| laravel/laravel | 109 | riff | 2.968 | 0.517 | 0.381 | n/a |
| laravel/laravel | 109 | viv | 3.179 | 0.070 | 0.008 | 1.428 |

- laravel/laravel/riff: run.sh: failed: riff
| symfony/demo | 153 | composer | 9.628 | 1.301 | 0.541 | 1.656 |
| symfony/demo | 153 | riff | 6.039 | 0.356 | 0.241 | 0.727 |
| symfony/demo | 153 | viv | 2.460 | 0.050 | 0.010 | 1.487 |
| composer/composer | 36 | composer | 4.603 | 0.806 | 0.402 | 0.631 |
| composer/composer | 36 | riff | 1.769 | 0.302 | 0.227 | 0.245 |
| composer/composer | 36 | viv | 1.784 | 0.012 | 0.005 | 0.391 |
| slimphp/Slim-Skeleton | 57 | composer | 5.802 | 1.112 | 0.444 | 0.756 |
| slimphp/Slim-Skeleton | 57 | riff | 1.719 | 0.225 | 0.239 | n/a |
| slimphp/Slim-Skeleton | 57 | viv | 1.806 | 0.038 | 0.006 | 0.348 |

- slimphp/Slim-Skeleton/riff: run.sh: failed: riff
| statamic/statamic | 160 | composer | 10.928 | 2.101 | 1.184 | 2.396 |
| statamic/statamic | 160 | riff | 4.701 | 0.716 | 0.562 | n/a |
| statamic/statamic | 160 | viv | 4.198 | 0.122 | 0.010 | 1.884 |

- statamic/statamic/riff: run.sh: failed: riff

Subset run: statamic/statamic, symfony/demo, laravel/laravel, composer/composer
and slimphp/Slim-Skeleton only, the projects with an open question after the
18:57 run, at the closure-walk commits that followed it. The other five
projects' numbers stand from 18:57. `run.sh: failed: riff` is riff's
update-warm failing to resolve; its install columns are real.

## 2026-09-09T13:20:33Z

viv 0.7.0, composer 2.10.2, riff 0.0.7, flags `--no-plugins --no-scripts`, 3 runs each, from a local mirror.

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| laravel/laravel | 109 | composer | 1.657 | 1.215 | 0.624 | 0.363 |
| laravel/laravel | 109 | riff | 0.675 | 0.607 | 0.508 | n/a |
| laravel/laravel | 109 | viv | 0.505 | 0.083 | 0.007 | 0.611 |

- laravel/laravel/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| symfony/demo | 153 | composer | 2.429 | 1.836 | 0.188 | 0.564 |
| symfony/demo | 153 | riff | 0.313 | 0.246 | 0.160 | n/a |
| symfony/demo | 153 | viv | 0.242 | 0.054 | 0.008 | 1.205 |

- symfony/demo/riff: run.sh: riff update-warm skipped, known mirror-mode failure (304 race, see bench/results/README.md)
| drupal/recommended-project | 68 | composer | 3.308 | 2.677 | 0.118 | 0.298 |
| drupal/recommended-project | 68 | riff | 0.741 | 0.666 | 0.020 | n/a |
| drupal/recommended-project | 68 | viv | 1.284 | 0.330 | 0.006 | 0.538 |

- drupal/recommended-project/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| roots/bedrock | 73 | composer | 1.587 | 1.256 | 0.284 | 0.357 |
| roots/bedrock | 73 | riff | 0.648 | 0.424 | 0.169 | n/a |
| roots/bedrock | 73 | viv | 0.509 | 0.068 | 0.006 | 0.369 |

- roots/bedrock/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| composer/composer | 36 | composer | 0.749 | 0.536 | 0.104 | 0.143 |
| composer/composer | 36 | riff | 0.086 | 0.048 | 0.011 | n/a |
| composer/composer | 36 | viv | 0.062 | 0.013 | 0.004 | 0.172 |

- composer/composer/riff: run.sh: riff update-warm skipped, known mirror-mode failure (304 race, see bench/results/README.md)
| phpunit/phpunit | 26 | composer | 0.744 | 0.576 | 0.222 | 0.109 |
| phpunit/phpunit | 26 | riff | 7.722 | 8.468 | 0.093 | n/a |
| phpunit/phpunit | 26 | viv | 0.120 | 0.063 | 0.004 | 0.069 |

- phpunit/phpunit/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| slimphp/Slim-Skeleton | 57 | composer | 1.006 | 0.758 | 0.145 | 0.142 |
| slimphp/Slim-Skeleton | 57 | riff | 0.145 | 0.097 | 0.035 | n/a |
| slimphp/Slim-Skeleton | 57 | viv | 0.188 | 0.042 | 0.005 | 0.171 |

- slimphp/Slim-Skeleton/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| yiisoft/yii2-app-basic | 93 | composer | 1.351 | 0.983 | 0.199 | 0.483 |
| yiisoft/yii2-app-basic | 93 | riff | n/a | n/a | n/a | n/a |
| yiisoft/yii2-app-basic | 93 | viv | 0.257 | 0.045 | 0.006 | n/a |

- yiisoft/yii2-app-basic/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
- yiisoft/yii2-app-basic/viv: run.sh: failed: viv
| statamic/statamic | 160 | composer | 2.487 | 1.916 | 0.898 | 0.490 |
| statamic/statamic | 160 | riff | 1.320 | 1.312 | 1.042 | n/a |
| statamic/statamic | 160 | viv | 0.873 | 0.136 | 0.009 | 0.932 |

- statamic/statamic/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| craftcms/craft | 118 | composer | 2.013 | 1.611 | 0.774 | 0.468 |
| craftcms/craft | 118 | riff | 1.204 | 0.840 | 0.611 | n/a |
| craftcms/craft | 118 | viv | 0.844 | 0.134 | 0.008 | 0.813 |

- craftcms/craft/riff: run.sh: riff update-warm skipped, known mirror-mode failure (304 race, see bench/results/README.md)

laravel/laravel was run twice, at 13:20:33Z and 14:13:42Z; the second run
reproduced the first within 3%, and the rows above are the later run.
Machine: AMD Ryzen 9 7900X3D (24 threads), ext4, 2026-09-09. viv 0.7.0,
composer 2.10.2, riff 0.0.7. roots/bedrock and yii2-app-basic were rerun on
2026-09-09 after the mirror learned to record every repository and
reference-less dists (#171, #173); yii2's viv update-warm still fails (#172).

## 2026-09-10T09:07:14Z

viv 0.9.0, composer 2.10.2, riff 0.0.7, flags `--no-plugins --no-scripts`, 3 runs each, from a local mirror.

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| laravel/laravel | 109 | composer | 1.752 | 1.239 | 0.607 | 0.385 |
| laravel/laravel | 109 | riff | 0.697 | 0.650 | 0.531 | n/a |
| laravel/laravel | 109 | viv | 0.514 | 0.084 | 0.008 | 0.319 |

- laravel/laravel/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| symfony/demo | 153 | composer | 1.637 | 1.204 | 0.270 | 1.025 |
| symfony/demo | 153 | riff | 0.489 | 0.380 | 0.295 | n/a |
| symfony/demo | 153 | viv | 0.308 | 0.076 | 0.008 | 0.855 |

- symfony/demo/riff: run.sh: riff update-warm skipped, known mirror-mode failure (304 race, see bench/results/README.md)
| drupal/recommended-project | 68 | composer | 2.313 | 1.991 | 0.138 | 0.310 |
| drupal/recommended-project | 68 | riff | 0.799 | 0.699 | 0.020 | n/a |
| drupal/recommended-project | 68 | viv | 1.443 | 0.336 | 0.006 | 0.220 |

- drupal/recommended-project/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| roots/bedrock | 73 | composer | 1.571 | 1.265 | 0.283 | 0.374 |
| roots/bedrock | 73 | riff | 0.559 | 0.445 | 0.171 | n/a |
| roots/bedrock | 73 | viv | 0.512 | 0.069 | 0.006 | 0.214 |

- roots/bedrock/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| composer/composer | 36 | composer | 0.651 | 0.520 | 0.105 | 0.163 |
| composer/composer | 36 | riff | 0.080 | 0.049 | 0.011 | n/a |
| composer/composer | 36 | viv | 0.061 | 0.013 | 0.004 | 0.075 |

- composer/composer/riff: run.sh: riff update-warm skipped, known mirror-mode failure (304 race, see bench/results/README.md)
| phpunit/phpunit | 26 | composer | 0.807 | 0.589 | 0.223 | 0.110 |
| phpunit/phpunit | 26 | riff | 8.192 | 8.562 | 0.105 | n/a |
| phpunit/phpunit | 26 | viv | 0.118 | 0.075 | 0.004 | 0.028 |

- phpunit/phpunit/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| slimphp/Slim-Skeleton | 57 | composer | 1.026 | 0.764 | 0.148 | 0.149 |
| slimphp/Slim-Skeleton | 57 | riff | 0.141 | 0.098 | 0.048 | n/a |
| slimphp/Slim-Skeleton | 57 | viv | 0.189 | 0.050 | 0.005 | 0.086 |

- slimphp/Slim-Skeleton/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| yiisoft/yii2-app-basic | 93 | composer | 1.289 | 0.943 | 0.208 | 0.666 |
| yiisoft/yii2-app-basic | 93 | riff | n/a | n/a | n/a | n/a |
| yiisoft/yii2-app-basic | 93 | viv | 0.255 | 0.051 | 0.007 | 0.602 |

- yiisoft/yii2-app-basic/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| statamic/statamic | 161 | composer | 3.092 | 2.001 | 0.914 | 0.537 |
| statamic/statamic | 161 | riff | 1.381 | 1.261 | 1.117 | n/a |
| statamic/statamic | 161 | viv | 0.965 | 0.142 | 0.010 | 0.441 |

- statamic/statamic/riff: skipped, known failure at riff 0.0.7 (bench/skips.txt)
| craftcms/craft | 119 | composer | 2.111 | 1.656 | 0.835 | 0.684 |
| craftcms/craft | 119 | riff | 1.234 | 0.931 | 0.738 | n/a |
| craftcms/craft | 119 | viv | 1.096 | 0.186 | 0.008 | 0.552 |

- craftcms/craft/riff: run.sh: riff update-warm skipped, known mirror-mode failure (304 race, see bench/results/README.md)

laravel/laravel was rerun alone because its first pass was contended; run
with viv 0.9.0 from a local mirror; the update column now contains no
advisory request (#182).
