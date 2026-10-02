# Candidate 3.2 follow-up: site-installed libraries an active plugin also bundles (#346)

How to reproduce: `make bench-g3-isolation-sites SITES="path path ..."` (read-only against each site; no composer/viv run against them). Invocation recorded here only as `sites.py <5 paths, held outside the repository>`.

## Site A

| Metric | Count |
|---|---|
| Site-installed libraries (composer.lock) | 56 |
| vendor/composer/installed.json present | yes |
| ...agrees with the lock | yes |
| Plugin/mu-plugin directories scanned | 39 |
| ...with a bundled dependency tree | 22 |
| &nbsp;&nbsp;installed via wpackagist | 7 |
| &nbsp;&nbsp;installed via composer | 9 |
| &nbsp;&nbsp;installed via committed | 1 |
| &nbsp;&nbsp;installed via untracked | 5 |
| Runtime clashes (site lock vs. bundled, unprefixed) | 7 |
| Solver-level clashes (composer.json history) | 0 |

## Site B

| Metric | Count |
|---|---|
| Site-installed libraries (composer.lock) | 196 |
| vendor/composer/installed.json present | yes |
| ...agrees with the lock | yes |
| Plugin/mu-plugin directories scanned | 86 |
| ...with a bundled dependency tree | 11 |
| &nbsp;&nbsp;installed via wpackagist | 5 |
| &nbsp;&nbsp;installed via vendor | 4 |
| &nbsp;&nbsp;installed via committed | 2 |
| Runtime clashes (site lock vs. bundled, unprefixed) | 10 |
| Solver-level clashes (composer.json history) | 0 |

## Site C

| Metric | Count |
|---|---|
| Site-installed libraries (composer.lock) | 65 |
| vendor/composer/installed.json present | yes |
| ...agrees with the lock | yes |
| Plugin/mu-plugin directories scanned | 39 |
| ...with a bundled dependency tree | 12 |
| &nbsp;&nbsp;installed via wpackagist | 7 |
| &nbsp;&nbsp;installed via composer | 1 |
| &nbsp;&nbsp;installed via vendor | 1 |
| &nbsp;&nbsp;installed via untracked | 3 |
| Runtime clashes (site lock vs. bundled, unprefixed) | 1 |
| Solver-level clashes (composer.json history) | 19 |

## Site D

| Metric | Count |
|---|---|
| Site-installed libraries (composer.lock) | 201 |
| vendor/composer/installed.json present | yes |
| ...agrees with the lock | no (8 version mismatches) |
| Plugin/mu-plugin directories scanned | 125 |
| ...with a bundled dependency tree | 10 |
| &nbsp;&nbsp;installed via wpackagist | 1 |
| &nbsp;&nbsp;installed via composer | 3 |
| &nbsp;&nbsp;installed via vendor | 4 |
| &nbsp;&nbsp;installed via committed | 2 |
| Runtime clashes (site lock vs. bundled, unprefixed) | 23 |
| Solver-level clashes (composer.json history) | 15 |

## Site E

| Metric | Count |
|---|---|
| Site-installed libraries (composer.lock) | 41 |
| vendor/composer/installed.json present | yes |
| ...agrees with the lock | yes |
| Plugin/mu-plugin directories scanned | 6 |
| ...with a bundled dependency tree | 3 |
| &nbsp;&nbsp;installed via wpackagist | 3 |
| Runtime clashes (site lock vs. bundled, unprefixed) | 0 |
| Solver-level clashes (composer.json history) | 0 |

## Runtime clashes

| Site | Plugin | Library | Site version | Bundled version | Prefixed | Install path |
|---|---|---|---|---|---|---|
| A | post-smtp | symfony/polyfill-mbstring | v1.33.0 | unknown | no | wpackagist |
| A | sitepress-multilingual-cms | symfony/polyfill-ctype | v1.33.0 | unknown | no | composer |
| A | sitepress-multilingual-cms | symfony/polyfill-mbstring | v1.33.0 | unknown | no | composer |
| A | wp-all-export-pro | symfony/polyfill-mbstring | v1.33.0 | v1.31.0 | no | composer |
| A | wp-migrate-db | phpoption/phpoption | 1.9.5 | 1.7.5 | no | wpackagist |
| A | wp-migrate-db | symfony/polyfill-ctype | v1.33.0 | v1.19.0 | no | wpackagist |
| A | wp-migrate-db | vlucas/phpdotenv | v5.6.3 | v4.3.0 | no | wpackagist |
| B | ai | wordpress/mcp-adapter | v0.5.0 | v0.3.0 | no | wpackagist |
| B | wordpress-seo | psr/http-message | 2.0 | unknown | no | wpackagist |
| B | wordpress-seo | psr/container | 1.0.0 | unknown | no | wpackagist |
| B | wordpress-seo | psr/log | 2.0.0 | unknown | no | wpackagist |
| B | wordpress-seo | guzzlehttp/guzzle | 7.10.0 | unknown | no | wpackagist |
| B | wordpress-seo | guzzlehttp/promises | 2.3.0 | unknown | no | wpackagist |
| B | wordpress-seo | guzzlehttp/psr7 | 2.9.0 | unknown | no | wpackagist |
| B | elasticpress | psr/container | 1.0.0 | unknown | no | vendor |
| B | wp-simple-saml | robrichards/xmlseclibs | 3.1.5 | 3.0.2 | no | vendor |
| B | wp-simple-saml | onelogin/php-saml | 3.8.2 | v3.0.0 | no | vendor |
| C | restricted-site-access | composer/installers | v1.12.0 | dev-main | no | wpackagist |
| D | multilingualpress | aws/aws-sdk-php | 3.351.7 | 3.369.0 | no | composer |
| D | multilingualpress | composer/installers | v1.12.0 | v2.3.0 | no | composer |
| D | multilingualpress | guzzlehttp/guzzle | 7.15.1 | 7.10.0 | no | composer |
| D | multilingualpress | guzzlehttp/promises | 2.5.1 | 2.3.0 | no | composer |
| D | multilingualpress | guzzlehttp/psr7 | 2.13.0 | 2.8.0 | no | composer |
| D | multilingualpress | mtdowling/jmespath.php | 2.9.2 | 2.8.0 | no | composer |
| D | multilingualpress | psr/log | 2.0.0 | 3.0.2 | no | composer |
| D | multilingualpress | symfony/deprecation-contracts | v3.7.1 | v3.6.0 | no | composer |
| D | multilingualpress | symfony/filesystem | v7.4.11 | v6.4.30 | no | composer |
| D | multilingualpress | symfony/polyfill-ctype | v1.37.0 | v1.33.0 | no | composer |
| D | multilingualpress | symfony/polyfill-mbstring | v1.38.2 | v1.33.0 | no | composer |
| D | wordpress-seo | psr/http-message | 2.0 | unknown | no | wpackagist |
| D | wordpress-seo | psr/http-client | 1.0.3 | unknown | no | wpackagist |
| D | wordpress-seo | psr/container | 1.0.0 | unknown | no | wpackagist |
| D | wordpress-seo | psr/log | 2.0.0 | unknown | no | wpackagist |
| D | wordpress-seo | psr/http-factory | 1.1.0 | unknown | no | wpackagist |
| D | wordpress-seo | guzzlehttp/guzzle | 7.15.1 | unknown | no | wpackagist |
| D | wordpress-seo | guzzlehttp/promises | 2.5.1 | unknown | no | wpackagist |
| D | wordpress-seo | guzzlehttp/psr7 | 2.13.0 | unknown | no | wpackagist |
| D | wordpress-seo | symfony/service-contracts | v2.2.0 | unknown | no | wpackagist |
| D | elasticpress | psr/container | 1.0.0 | unknown | no | vendor |
| D | wp-simple-saml | robrichards/xmlseclibs | 3.1.5 | 3.0.2 | no | vendor |
| D | wp-simple-saml | onelogin/php-saml | 3.8.2 | v3.0.0 | no | vendor |

## Solver-level clashes

| Site | Constraint | Year |
|---|---|---|
| C | pin `"roots/wordpress": "5.6"` | 2021 |
| C | pin `"humanmade/network-media-library": "1.5.0"` | 2021 |
| C | pin `"humanmade/network-media-library": "1.5.0"` | 2021 |
| C | pin `"humanmade/coding-standards": "1.1.3"` | 2021 |
| C | pin `"roots/wordpress": "6.2"` | 2023 |
| C | pin `"altis/core": "15.0"` | 2023 |
| C | pin `"altis/dev-tools": "15.0"` | 2023 |
| C | pin `"altis/local-server": "15.0.1"` | 2023 |
| C | pin `"roots/wordpress": "6.2"` | 2023 |
| C | pin `"altis/core": "15.0"` | 2023 |
| C | pin `"altis/dev-tools": "15.0"` | 2023 |
| C | pin `"altis/local-server": "15.0.1"` | 2023 |
| C | pin `"wp-coding-standards/wpcs": "2.3.0"` | 2025 |
| C | pin `"cweagans/composer-patches": "1.7.3"` | 2025 |
| C | pin `"johnbillion/extended-cpts": "5.1.0"` | 2025 |
| C | pin `"vlucas/phpdotenv": "5.6.1"` | 2025 |
| C | pin `"roots/wordpress": "5.3.2"` | 2025 |
| C | pin `"humanmade/coding-standards": "1.2.1"` | 2025 |
| C | pin `"wp-coding-standards/wpcs": "2.3.0"` | 2025 |
| D | pin `"humanmade/coding-standards": "0.7.0"` | 2019 |
| D | pin `"gravityforms/gravityforms": "2.4.23.2"` | 2021 |
| D | pin `"altis/enhanced-search": "12.0.0"` | 2023 |
| D | pin `"altis/seo": "12.0.0"` | 2023 |
| D | pin `"wp-phpunit/wp-phpunit": "6.2.0"` | 2023 |
| D | pin `"wp-phpunit/wp-phpunit": "6.2.0"` | 2024 |
| D | pin `"altis/enhanced-search": "13.0.1"` | 2024 |
| D | pin `"altis/enhanced-search": "13.0.1"` | 2024 |
| D | pin `"league/oauth2-server": "9"` | 2024 |
| D | pin `"league/oauth2-server": "9"` | 2024 |
| D | pin `"woocommerce/action-scheduler": "3.8.1"` | 2024 |
| D | pin `"league/oauth2-server": "9"` | 2024 |
| D | pin `"league/oauth2-server": "9"` | 2024 |
| D | pin `"altis/local-server": "22.0.5"` | 2026 |
| D | pin `"altis/local-server": "22.0.5"` | 2026 |

## Reading

Across the 5 sites, 295 plugin/mu-plugin directories were scanned and 58 of them carry a bundled dependency tree (a vendor/, vendor_prefixed/, vendor-prefixed/, lib/packages/ or dependencies/ directory somewhere inside). 5 of 5 sites also carry a vendor/composer/installed.json, of which 4 agree with composer.lock on every shared library's version. 41 of the bundled copies are a library the site's own lock also installs, at a different version, read as unprefixed -- the runtime collision candidates the WordPress.org sample in bench/results/g3-isolation.md could not show a rate for. Of those 41, 20 have a bundled version this script could not read (a directory-name fallback, not an installed.json) and 20 are psr/*, symfony/polyfill-* or composer/installers -- the same built-to-coexist or install-time-only families bench/results/g3-isolation.md classified out of its own conflict-candidate count. Walking each site's composer.json history the same way (WordPress asset pins excluded) turned up 34 commits that pinned a library to an exact or narrow version, added a conflict entry or a replace where the prior revision had neither.
