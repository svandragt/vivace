
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
