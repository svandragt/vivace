# Lock age --verify: does viv's vendor/ match Composer's? (#307 follow-up)

Measured 2026-09-27T09:50:01Z in 116.8s wall-clock. Composer's install for a (project, commit) pair is cached once (`LOCK_AGE_REF`) and reused across runs; both tools share one warm cache; pairs run in parallel (`LOCK_AGE_JOBS`). Skips the source-ref fetch checks and dist-gone classification `lock-age.py`'s normal mode does -- this only answers whether the two vendor/ trees match.

| Project | Age | Commit | Lock date | viv installs | vendor/ | Time |
|---|---|---|---|---|---|---|
| laravel/laravel | 1yr | d15ab4b82ed3 | 2015-10-14 | yes | identical | 1.3s |
| laravel/laravel | 2yr | d15ab4b82ed3 | 2015-10-14 | - | same commit as the 1yr row | - |
| laravel/laravel | 4yr | d15ab4b82ed3 | 2015-10-14 | - | same commit as the 1yr row | - |
| symfony/demo | 1yr | aede8b54a712 | 2025-08-15 | yes | identical | 2.3s |
| symfony/demo | 2yr | e4ef1f8d0360 | 2024-07-19 | yes | identical | 2.5s |
| symfony/demo | 4yr | db0a2759e3d4 | 2022-08-04 | yes | identical | 3.1s |
| roots/bedrock | 1yr | 0c02cb9e8a0c | 2025-09-24 | yes | identical | 1.5s |
| roots/bedrock | 2yr | 6acbb2420eac | 2024-09-11 | yes | identical | 1.2s |
| roots/bedrock | 4yr | a70e2b04acfc | 2022-08-31 | yes | identical | 1.5s |
| composer/composer | 1yr | 8fc94c5e9972 | 2025-09-17 | yes | identical | 2.5s |
| composer/composer | 2yr | 6b81140f81a4 | 2024-09-21 | yes | identical | 2.2s |
| composer/composer | 4yr | 4f0419059262 | 2022-09-14 | yes | identical | 2.0s |
| phpunit/phpunit | 1yr | 9f79830b3076 | 2025-09-24 | yes | identical | 3.9s |
| phpunit/phpunit | 2yr | 9109547f3e73 | 2024-09-17 | yes | differs (Files /home/sander/.cache/vivace-bench/lock-age-ref/phpunit_phpunit/9109547f3e73-v4-noplugins/vendor/composer/installed.php and /tmp/lock-age-verify-a_uckx7l/pairs/phpunit_phpunit_2yr/work/vendor/composer/installed.php differ) | 2.8s |
| phpunit/phpunit | 4yr | - | - | - | no composer.lock commit on or before 2022-09-26 | - |
| slimphp/Slim-Skeleton | 1yr | f12402f42504 | 2015-12-07 | yes | identical | 0.6s |
| slimphp/Slim-Skeleton | 2yr | f12402f42504 | 2015-12-07 | - | same commit as the 1yr row | - |
| slimphp/Slim-Skeleton | 4yr | f12402f42504 | 2015-12-07 | - | same commit as the 1yr row | - |
| yiisoft/yii2-app-basic | 1yr | 3be9b8507dc1 | 2016-07-25 | yes | identical | 1.5s |
| yiisoft/yii2-app-basic | 2yr | 3be9b8507dc1 | 2016-07-25 | - | same commit as the 1yr row | - |
| yiisoft/yii2-app-basic | 4yr | 3be9b8507dc1 | 2016-07-25 | - | same commit as the 1yr row | - |
| typo3/cms-base-distribution | 1yr | - | - | - | no composer.lock commit on or before 2025-09-26 | - |
| typo3/cms-base-distribution | 2yr | - | - | - | no composer.lock commit on or before 2024-09-26 | - |
| typo3/cms-base-distribution | 4yr | - | - | - | no composer.lock commit on or before 2022-09-26 | - |
| cakephp/app | 1yr | 6aec4a64b270 | 2019-12-10 | yes | identical | 1.6s |
| cakephp/app | 2yr | 6aec4a64b270 | 2019-12-10 | - | same commit as the 1yr row | - |
| cakephp/app | 4yr | 6aec4a64b270 | 2019-12-10 | - | same commit as the 1yr row | - |
| wp-cli/wp-cli-bundle | 1yr | 2633466a8827 | 2025-09-15 | yes | identical (path-only differences normalised: 1) | 1.9s |
| wp-cli/wp-cli-bundle | 2yr | 2fd885705c64 | 2024-08-19 | yes | identical | 1.7s |
| wp-cli/wp-cli-bundle | 4yr | 38dab66ad7b3 | 2022-09-19 | yes | identical | 1.8s |
| symfony/skeleton | 1yr | - | - | - | no composer.lock commit on or before 2025-09-26 | - |
| symfony/skeleton | 2yr | - | - | - | no composer.lock commit on or before 2024-09-26 | - |
| symfony/skeleton | 4yr | - | - | - | no composer.lock commit on or before 2022-09-26 | - |
| api-platform/api-platform | 1yr | d82159a320a6 | 2017-09-21 | yes | identical | 2.9s |
| api-platform/api-platform | 2yr | d82159a320a6 | 2017-09-21 | - | same commit as the 1yr row | - |
| api-platform/api-platform | 4yr | d82159a320a6 | 2017-09-21 | - | same commit as the 1yr row | - |
| laminas/laminas-mvc-skeleton | 1yr | - | - | - | no composer.lock commit on or before 2025-09-26 | - |
| laminas/laminas-mvc-skeleton | 2yr | - | - | - | no composer.lock commit on or before 2024-09-26 | - |
| laminas/laminas-mvc-skeleton | 4yr | - | - | - | no composer.lock commit on or before 2022-09-26 | - |
| spiral/app | 1yr | 96a5e786d1cc | 2021-05-03 | yes | identical | 1.3s |
| spiral/app | 2yr | 96a5e786d1cc | 2021-05-03 | - | same commit as the 1yr row | - |
| spiral/app | 4yr | 96a5e786d1cc | 2021-05-03 | - | same commit as the 1yr row | - |
| codeigniter4/appstarter | 1yr | 72493f131efb | 2019-01-19 | - | composer failed (In RootPackageLoader.php line 179:) | 0.0s |
| codeigniter4/appstarter | 2yr | 72493f131efb | 2019-01-19 | - | same commit as the 1yr row | - |
| codeigniter4/appstarter | 4yr | 72493f131efb | 2019-01-19 | - | same commit as the 1yr row | - |
| doctrine/orm | 1yr | b13b2e8bab81 | 2020-10-28 | yes | identical | 2.0s |
| doctrine/orm | 2yr | b13b2e8bab81 | 2020-10-28 | - | same commit as the 1yr row | - |
| doctrine/orm | 4yr | b13b2e8bab81 | 2020-10-28 | - | same commit as the 1yr row | - |
| phpmyadmin/phpmyadmin | 1yr | 79830ce9cdc9 | 2025-09-12 | yes | identical (path-only differences normalised: 1) | 20.5s |
| phpmyadmin/phpmyadmin | 2yr | ec977df5cd07 | 2024-09-20 | yes | identical | 20.6s |
| phpmyadmin/phpmyadmin | 4yr | 9dfedff70b3f | 2022-05-11 | yes | differs (Files /home/sander/.cache/vivace-bench/lock-age-ref/phpmyadmin_phpmyadmin/9dfedff70b3f-v4-noplugins/vendor/composer/installed.php and /tmp/lock-age-verify-a_uckx7l/pairs/phpmyadmin_phpmyadmin_4yr/work/vendor/composer/installed.php differ) | 20.6s |
| monicahq/monica | 1yr | dd527db56663 | 2025-08-09 | yes | identical (path-only differences normalised: 1) | 3.6s |
| monicahq/monica | 2yr | f7402830554e | 2024-06-15 | yes | identical | 4.2s |
| monicahq/monica | 4yr | 225d7fef8e65 | 2022-09-23 | yes | identical | 3.0s |
| BookStackApp/BookStack | 1yr | 64b06bcf6184 | 2025-08-30 | yes | identical | 4.2s |
| BookStackApp/BookStack | 2yr | 9f68ca535872 | 2024-08-26 | yes | identical | 4.1s |
| BookStackApp/BookStack | 4yr | f4388d5e4a63 | 2022-09-22 | yes | identical | 3.5s |
| snipe/snipe-it | 1yr | 61df3bc46231 | 2025-09-16 | yes | identical | 7.2s |
| snipe/snipe-it | 2yr | 0b3ac2a9cd21 | 2024-08-14 | yes | identical | 8.6s |
| snipe/snipe-it | 4yr | 443b1df5e182 | 2022-07-22 | yes | identical | 8.4s |
| firefly-iii/firefly-iii | 1yr | d3c557ca2255 | 2025-09-26 | yes | identical (path-only differences normalised: 1) | 5.5s |
| firefly-iii/firefly-iii | 2yr | 8938622bd948 | 2024-09-23 | yes | identical (path-only differences normalised: 1) | 3.9s |
| firefly-iii/firefly-iii | 4yr | ff55b36f320d | 2022-09-26 | yes | identical | 3.7s |
| pterodactyl/panel | 1yr | 955dd2796d1f | 2024-11-14 | yes | identical | 3.6s |
| pterodactyl/panel | 2yr | 1d38b4f0e201 | 2023-02-23 | yes | identical | 3.3s |
| pterodactyl/panel | 4yr | 80ae600fe1d1 | 2022-06-26 | yes | identical | 2.8s |

40 pairs measured, 26 skipped. Outcomes: composer failed 1, differs 2, identical 37.

