# Candidate 3.5: simpler resolution rules, the measurement (#333)

How to reproduce: `make bench-g3-rules` (network: git clones for Part 1; Part 2 is a fixed classification, not refetched). Closed-issue list fetched 2026-09-28.

## Part 1: feature census

Projects: 20 analysed of 53 in the corpus (33 skipped, no committed lock or no checkout).

| project | min-stab | pref-stable | no-version | branch-alias(root) | replace | provide | self.version | no-packagist | conflict | config.platform | allow-plugins | branch-alias(lock) | inline-alias(root) | lock-aliases | dev-*(root) | stability-flags(lock) | repo:vcs | repo:path | repo:composer | repo:package | repo:artifact | lock-type≠lib/meta | lock-dev-version |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| laravel/laravel | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| symfony/demo |  | Y | Y |  | Y |  |  |  |  | Y | Y | 49 |  |  |  |  |  |  |  |  |  | 24 |  |
| drupal/recommended-project | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| roots/bedrock | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| composer/composer |  |  | Y | Y |  |  |  |  |  | Y |  | 13 |  |  |  |  |  |  |  |  |  | 5 |  |
| phpunit/phpunit | Y | Y | Y | Y |  |  |  |  |  | Y |  | 22 |  |  |  |  |  |  |  |  |  |  | 1 |
| slimphp/Slim-Skeleton | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| yiisoft/yii2-app-basic | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| statamic/statamic | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| craftcms/craft | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| typo3/cms-base-distribution | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| cakephp/app | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| contao/managed-edition | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| silverstripe/installer | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| shopware/template | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| octobercms/october | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| wp-cli/wp-cli-bundle | Y | Y | Y | Y |  |  |  |  |  | Y | Y | 78 |  |  | 2 | 2 |  |  |  |  |  | 43 | 3 |
| bolt/project | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| firstphp/ip2region | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| whoa-php/flute | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| accessd/yii2-rollbar | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| tsg/ar | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| tumtum/oxid-inline-translator | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| mozart/event-dispatcher | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| ahmadarif/laravel-pagination | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| tokimikichika/text-analysis | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| 8xprovn/microservice | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| dmstr/api-configuration-bundle | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| numesia/all-my-sms | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| gamebetr/provable |  |  | Y |  |  |  |  |  |  |  |  | 27 |  |  |  |  |  |  |  |  |  |  |  |
| thoughtco/statamic-cp-resources | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| gento-arg/module-oca | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| reedware/laravel-api |  |  | Y | Y |  |  |  |  |  |  |  | 8 |  |  |  |  |  |  |  |  |  |  |  |
| symfony/skeleton | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| api-platform/api-platform | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| laminas/laminas-mvc-skeleton | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| spiral/app | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| codeigniter4/appstarter | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| doctrine/orm | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip | skip |
| phpmyadmin/phpmyadmin |  |  | Y | Y |  |  |  |  | Y | Y | Y | 57 |  |  |  |  |  |  | 1 |  |  | 11 |  |
| matomo-org/matomo |  |  | Y |  |  |  |  |  |  | Y | Y | 40 |  |  | 4 | 4 | 1 |  |  |  |  | 5 | 4 |
| monicahq/monica |  | Y | Y |  |  |  |  |  |  |  | Y | 83 |  |  | 1 | 2 |  |  |  |  |  | 7 | 1 |
| koel/koel |  |  | Y |  |  |  |  |  |  | Y | Y | 83 |  |  | 1 | 1 | 1 |  |  |  |  | 3 | 1 |
| pixelfed/pixelfed |  | Y | Y |  |  |  |  |  |  | Y | Y | 80 |  |  |  |  |  |  |  |  |  | 6 |  |
| BookStackApp/BookStack |  | Y | Y |  |  |  |  |  |  | Y |  | 67 |  |  |  |  |  |  |  |  |  | 1 |  |
| snipe/snipe-it |  |  | Y |  |  |  |  |  | Y |  | Y | 97 |  |  | 1 | 1 | 1 |  |  |  |  | 6 | 1 |
| mautic/mautic | Y | Y | Y |  | Y |  | Y |  | Y | Y | Y | 97 |  |  | 1 | 1 |  | 1 |  |  |  | 40 | 1 |
| kimai/kimai |  |  | Y |  | Y |  |  |  | Y | Y | Y | 62 |  |  |  |  |  |  |  |  | 1 | 35 |  |
| firefly-iii/firefly-iii |  |  | Y |  | Y |  |  |  |  | Y | Y | 88 |  |  | 1 | 1 |  |  |  |  |  | 10 | 1 |
| pterodactyl/panel |  | Y | Y |  |  |  |  |  |  | Y |  | 74 |  |  |  |  |  |  |  |  |  | 5 |  |
| librenms/librenms |  | Y | Y |  |  |  |  |  |  |  | Y | 89 |  |  | 1 | 1 |  |  |  |  |  | 7 | 1 |
| humhub/humhub |  |  | Y |  | Y |  |  |  |  | Y | Y | 93 |  |  | 1 | 1 |  |  | 1 |  |  | 103 | 1 |
| akaunting/akaunting |  |  | Y |  |  |  |  |  |  |  | Y | 102 |  |  |  |  |  |  |  | 1 |  | 6 |  |

### Totals

- min-stab: **3** of 20 projects.
- pref-stable: **9** of 20 projects.
- no-version: **20** of 20 projects.
- branch-alias(root): **5** of 20 projects.
- replace: **5** of 20 projects.
- provide: **0** of 20 projects.
- self.version: **1** of 20 projects.
- no-packagist: **0** of 20 projects.
- conflict: **4** of 20 projects.
- config.platform: **14** of 20 projects.
- allow-plugins: **14** of 20 projects.
- branch-alias(lock): **20** of 20 projects use it, 1309 total.
- inline-alias(root): **0** of 20 projects use it, 0 total.
- lock-aliases: **0** of 20 projects use it, 0 total.
- dev-*(root): **9** of 20 projects use it, 13 total.
- stability-flags(lock): **9** of 20 projects use it, 14 total.
- repo:vcs: **3** of 20 projects use it, 3 total.
- repo:path: **1** of 20 projects use it, 1 total.
- repo:composer: **2** of 20 projects use it, 2 total.
- repo:package: **1** of 20 projects use it, 1 total.
- repo:artifact: **1** of 20 projects use it, 1 total.
- lock-type≠lib/meta: **17** of 20 projects use it, 317 total.
- lock-dev-version: **10** of 20 projects use it, 15 total.

## Part 2: compatibility bugs by feature

Selection rule: Closed issues (plus #312 and #316, both open, named by the candidate) describing a case where viv's resolved package set, lock bytes, installed.json/php, autoload output, or install/update/require success-vs-failure differs from real Composer's on the same input. Read individually (title, body, and the closing commit message where the cause was ambiguous from the issue text alone); not selected by title keyword match alone.

| Issue | Title | Feature | Reason |
|---|---|---|---|
| #28 | installed.php gaps: root replace/provide, natural sort case, numeric alias check | replace/provide | root-level replace/provide missing from installed.php's `versions` map (also touches a branch-alias numeric-prefix check, secondary) |
| #35 | lock: reject a package present in both packages and packages-dev | none of these | lock structural validation (duplicate package name across packages/packages-dev), not a resolution-rule feature |
| #63 | Metapackages with a dist must not be installed | package type | viv keyed install-or-not on dist presence instead of `type: metapackage` |
| #79 | Partial update of a transitively-required package fails: not found in any version | none of these | fix: "partial update loads the allow-listed package through locked parents" -- graph-traversal completeness, not a manifest feature |
| #105 | viv update fails on wpackagist metadata: provider entry is not a list | repositories | composer-type repository (wpackagist) serving Composer v1's object-keyed provider format |
| #115 | update: self.version in a dependency's require breaks the closure walk (bedrock, drupal) | replace/provide | self.version literal reached the constraint parser before substitution in the closure walk |
| #116 | update: pool optimizer leaves alias_of unremapped, panics on phpunit/phpunit | branch-alias | triggered specifically by branch-aliased packages (phpunit) surviving pool pruning |
| #117 | update: root replace/provide ignored, symfony/demo lock gains four polyfills | replace/provide | solver didn't model the root package's own replace/provide as satisfying a dependent's requirement |
| #118 | update: php-64bit and lib-* platform packages missing from the solver | none of these | built-in platform-package completeness (php-64bit, lib-*), not the census's `config.platform` override field |
| #119 | update: honour available-package-patterns so wpackagist isn't asked about every name | repositories | composer-type repository capability (`available-package-patterns`) not honoured |
| #125 | Root package version: guess from git like Composer's VersionGuesser | root-version guessing | root `version` absent; guess-from-git never ran for installed.php/installed.json |
| #128 | Root version guess differs from Composer on composer/composer and phpunit/phpunit checkouts | branch-alias | fix: "apply extra.branch-alias to the root package's own aliases" |
| #149 | Metapackage with no dist and no source fails install: shopware/conflicts | package type | same `type: metapackage` handling gap as #63, different project |
| #158 | add and rm: the solve ignores --offline and the project's repositories | repositories | partial-update solve hard-coded Packagist, ignoring composer.json's own `repositories` |
| #160 | add: bare-name constraint synthesis still resolves against Packagist only and ignores --offline | repositories | same repositories gap as #158, different call site (bare-name constraint synthesis) |
| #161 | update, add, rm: composer-type repositories with a file:// URL are rejected | repositories | composer-type repository whose url is a local file:// mirror was rejected outright |
| #172 | update: yii2-app-basic fails to resolve, viv sees only dev-master for yiisoft/yii2 | branch-alias | fix: "keep branch aliases in the dev-split second solve" -- extra.branch-alias dropped when the pool was rebuilt |
| #242 | solver: --ignore-platform-req(s) does not reach the solve, only the autoload write | none of these | CLI flag plumbing (fixed together with #300); not the census's `config.platform` manifest field |
| #257 | install: dist URL placeholders (%prettyVersion%) are fetched literally | none of these | dist URL templating, not a resolution-rule feature |
| #258 | install: lock check ignores replace/provide, refuses humhub/humhub | replace/provide | lock-freshness check didn't credit a locked package's replace/provide as satisfying a root requirement |
| #261 | update: an inferred stability flag equal to minimum-stability is dropped | dev-*/stability-flags | lock's `stability-flags` differs when an inferred flag equals the root's own `minimum-stability` |
| #267 | update: partial update fails when a held dependency is locked at a dev branch | branch-alias | fix: "a held dev branch keeps its branch alias in a partial update" |
| #282 | update rewrites a VCS package's source.url from SSH to HTTPS | repositories | vcs-type repository source.url normalisation differed from Composer's |
| #283 | Contract break: the API-fallback path writes an https source.url where Composer writes ssh | repositories | same vcs-type source.url gap as #282, different code path (GitHub API unreachable) |
| #294 | Inline package repositories are refused; 16 of 355 client merges cannot re-solve | repositories | `package`-type repository unsupported |
| #300 | install: missing PHP extension required by the lock installs anyway, Composer refuses | none of these | ext-* platform requirement enforcement at install, not the census's `config.platform` field |
| #304 | update: self.version in the root composer.json's own require still fails to parse | replace/provide | self.version substitution missing on the root's own require (#115 only covered a dependency's) |
| #305 | update/install: a top-level path-type repository is refused outright | repositories | `path`-type repository unsupported |
| #317 | install: installed.json keeps a Composer 1 lock's time format where Composer writes RFC 3339 | none of these | output formatting (time field), not a resolution rule |
| #318 | install: a mixed-case package name installs into a lowercase vendor directory | none of these | installer path casing, not a resolution rule |
| #205 | update: config.bump-after-update is ignored | none of these | manifest-mutation feature (`config.bump-after-update`), not in the census's rule list |
| #188 | Adopting a committed vendor/ rewrites autoload files with a different PSR-4 order | none of these | autoload generation order, not a resolution rule |
| #175 | update: block versions with security advisories and abandoned packages by default | none of these | security-advisories/abandoned filtering, not in the census's rule list |
| #322 | installed.php: a branch-aliased package's self.version replace lists only the branch, not the alias | branch-alias | named by the candidate; branch-alias interacting with a self.version replace |
| #312 | update: the solve never uses the git-guessed root version, only installed.php does | root-version guessing | OPEN -- named by the candidate; root `version` absent, the solve (unlike installed.php) never guesses from git |
| #316 | solver: craftcms/craft resolves yii2-shell 2.0.6 where Composer picks dev-master | none of these | OPEN -- named by the candidate; search-order/backjump interaction with the security-advisories feed, not one of the census's manifest features |

### Totals by feature

- none of these: **12**
- repositories: **9**
- replace/provide: **5**
- branch-alias: **5**
- package type: **2**
- root-version guessing: **2**
- dev-*/stability-flags: **1**

36 issues classified (6 distinct resolution-rule features, 12 'none of these').

### Excluded but considered

- #293 The pre-merge performance gate cannot fail on its current project -- bench-harness gate, not a viv/Composer divergence
- #148 Compat sweep aborts on a failed clone instead of skipping the row -- compat/run.sh harness bug, not viv's own output
- #151 Plugins refused on popular skeletons: TYPO3, CakePHP, Contao, Silverstripe, Bolt -- a record of known adapter gaps, not itself a fix (adapters tracked separately)
- #214 install: --ignore-platform-reqs is unreachable, so a migrated build stage gets a platform_check.php Composer omits -- first stage of the same gap superseded by #242
- #231 update/require/remove: --ignore-platform-reqs still unreachable -- second stage of the same gap superseded by #242
- #73 parse_constraint panics on multi-byte input (semver-php slices at a non-char boundary) -- parser encoding robustness (non-ASCII constraint), not a resolution-rule divergence
- Plugin-adapter completeness bugs (#51, #52, #53, #75, #92, #93, #98, #101, #126, #129, #130, #131, #157, #162, #218): each is a real compatibility bug (a specific third-party plugin behaves differently under viv), but about that plugin, not a resolution rule this census counts; every one would classify `none of these`, so they're counted here rather than given a row each.

## Reading

Of 20 corpus projects with a committed lock, 20 have no root `version` field (Composer guesses it from git on every one of them), 5 set a root `extra.branch-alias` and 20 carry a locked package with one, 5 set root `replace` and 0 set root `provide` (1 of those use `self.version` as a value), 0 use an inline alias (` as `) in a root constraint and 0 carry a non-empty lock `aliases` array, 9 have a root `dev-*` constraint and 9 a non-empty lock `stability-flags`, 3 set a non-stable `minimum-stability` and 9 set `prefer-stable`, 4 set root `conflict`, 14 set `config.platform` and 14 set `config.allow-plugins`, and 17 carry a locked package whose type is neither library nor metapackage. On the bug side, 36 closed and candidate-named issues were classified by feature (repositories 9, replace/provide 5, branch-alias 5, package type 2, root-version guessing 2, dev-*/stability-flags 1, 12 'none of these'), plus 15 plugin-adapter bugs folded into 'none of these' rather than given a row each.

Wall time: 164.4s.

## Part 3: coverage

Scanned: `tests/fixtures/**/composer.json` (plus the `composer.json.before`/`.after` pairs, skipping manifests nested under a `packages/` or `vendor/` directory) and any sibling `composer.lock`, evaluated with the same `features_for` Part 1 uses; inline composer.json literals in `tests/*.rs` (whole file) and `src/**/*.rs` (text from the first `#[cfg(test)]` marker on, one hit per file), grepped per feature for its JSON key or a quoted-string literal; and `compat/corpus.toml` projects, reusing Part 1's per-project rows. `no-version` and `lock-type≠lib/meta` aren't literal substrings a grep can key on, so those two are fixture-file-only, with no inline-literal hits possible.

| feature | used by N of 20 | fixture files | corpus projects | example fixtures |
|---|---|---|---|---|
| min-stab | 3 of 20 | 4 | phpunit/phpunit, wp-cli/wp-cli-bundle, mautic/mautic | tests/install_index_merge.rs:155, tests/update.rs:740, tests/workspace.rs:174 |
| pref-stable | 9 of 20 | 3 | symfony/demo, phpunit/phpunit, wp-cli/wp-cli-bundle, monicahq/monica, pixelfed/pixelfed, BookStackApp/BookStack, mautic/mautic, pterodactyl/panel, librenms/librenms | tests/install_index_merge.rs:157, tests/workspace.rs:175, src/lock.rs:1744 |
| no-version | 20 of 20 | 54 | symfony/demo, composer/composer, phpunit/phpunit, wp-cli/wp-cli-bundle, gamebetr/provable, reedware/laravel-api, phpmyadmin/phpmyadmin, matomo-org/matomo, monicahq/monica, koel/koel, pixelfed/pixelfed, BookStackApp/BookStack, snipe/snipe-it, mautic/mautic, kimai/kimai, firefly-iii/firefly-iii, pterodactyl/panel, librenms/librenms, humhub/humhub, akaunting/akaunting | tests/fixtures/audit/abandoned/composer.json, tests/fixtures/audit/clean/composer.json, tests/fixtures/audit/vulnerable/composer.json |
| branch-alias(root) | 5 of 20 | 5 | composer/composer, phpunit/phpunit, wp-cli/wp-cli-bundle, reedware/laravel-api, phpmyadmin/phpmyadmin | tests/root_version.rs:119, tests/update.rs:949, tests/vcs.rs:78 |
| replace | 5 of 20 | 2 | symfony/demo, mautic/mautic, kimai/kimai, firefly-iii/firefly-iii, humhub/humhub | tests/fixtures/root-replace/composer.json, src/lock.rs:1817 |
| provide | 0 of 20 | 3 | -- | tests/fixtures/validate/fixable/composer.json, src/autoload/installed.rs:766, src/lock.rs:1844 |
| self.version | 1 of 20 | 3 | mautic/mautic | tests/repository.rs:295, tests/self_version_root.rs:1, src/autoload/installed.rs:773 |
| no-packagist | 0 of 20 | 21 | -- | tests/fixtures/ignore-platform-solve/composer.json, tests/fixtures/package-repository/composer.json, tests/fixtures/satis/psr-project/composer.json |
| conflict | 4 of 20 | 2 | phpmyadmin/phpmyadmin, snipe/snipe-it, mautic/mautic, kimai/kimai | tests/problem_messages.rs:115, src/lock.rs:1742 |
| config.platform | 14 of 20 | 4 | symfony/demo, composer/composer, phpunit/phpunit, wp-cli/wp-cli-bundle, phpmyadmin/phpmyadmin, matomo-org/matomo, koel/koel, pixelfed/pixelfed, BookStackApp/BookStack, mautic/mautic, kimai/kimai, firefly-iii/firefly-iii, pterodactyl/panel, humhub/humhub | tests/fixtures/platform-check-php/composer.json, tests/install_index_merge.rs:159, tests/update.rs:1469 |
| allow-plugins | 14 of 20 | 15 | symfony/demo, wp-cli/wp-cli-bundle, phpmyadmin/phpmyadmin, matomo-org/matomo, monicahq/monica, koel/koel, pixelfed/pixelfed, snipe/snipe-it, mautic/mautic, kimai/kimai, firefly-iii/firefly-iii, librenms/librenms, humhub/humhub, akaunting/akaunting | tests/fixtures/plugins/c3/composer.json, tests/fixtures/plugins/composer-patches/composer.json, tests/fixtures/plugins/craft/composer.json |
| branch-alias(lock) | 20 of 20 | 29 | symfony/demo, composer/composer, phpunit/phpunit, wp-cli/wp-cli-bundle, gamebetr/provable, reedware/laravel-api, phpmyadmin/phpmyadmin, matomo-org/matomo, monicahq/monica, koel/koel, pixelfed/pixelfed, BookStackApp/BookStack, snipe/snipe-it, mautic/mautic, kimai/kimai, firefly-iii/firefly-iii, pterodactyl/panel, librenms/librenms, humhub/humhub, akaunting/akaunting | tests/fixtures/audit/vulnerable/composer.json, tests/fixtures/legacy/composer.json, tests/fixtures/minimal-changes/composer.json |
| inline-alias(root) | 0 of 20 | 8 | -- | tests/fixtures/root-alias/composer.json, tests/install_e2e.rs:338, tests/lock_merge.rs:375 |
| lock-aliases | 0 of 20 | 2 | -- | tests/fixtures/root-alias/composer.json, tests/install_index_merge.rs:154 |
| dev-*(root) | 9 of 20 | 20 | wp-cli/wp-cli-bundle, matomo-org/matomo, monicahq/monica, koel/koel, snipe/snipe-it, mautic/mautic, firefly-iii/firefly-iii, librenms/librenms, humhub/humhub | tests/fixtures/root-alias/composer.json, tests/install_e2e.rs:580, tests/path_repository.rs:71 |
| stability-flags(lock) | 9 of 20 | 2 | wp-cli/wp-cli-bundle, matomo-org/matomo, monicahq/monica, koel/koel, snipe/snipe-it, mautic/mautic, firefly-iii/firefly-iii, librenms/librenms, humhub/humhub | tests/fixtures/root-alias/composer.json, tests/install_index_merge.rs:156 |
| repo:vcs | 3 of 20 | 3 | matomo-org/matomo, koel/koel, snipe/snipe-it | tests/install_e2e.rs:960, tests/vcs.rs:97, src/repository.rs:3662 |
| repo:path | 1 of 20 | 18 | mautic/mautic | tests/fixtures/lock-operations/composer.json, tests/fixtures/metapackage/composer.json, tests/fixtures/path/composer.json |
| repo:composer | 2 of 20 | 12 | phpmyadmin/phpmyadmin, humhub/humhub | tests/fixtures/ignore-platform-solve/composer.json, tests/fixtures/satis/psr-project/composer.json, tests/fixtures/show/outdated-monolog/composer.json |
| repo:package | 1 of 20 | 8 | akaunting/akaunting | tests/fixtures/metapackage/composer.json, tests/fixtures/package-repository/composer.json, tests/fixtures/self-version-root/default-version/composer.json |
| repo:artifact | 1 of 20 | 1 | kimai/kimai | src/repository.rs:3627 |
| lock-type≠lib/meta | 17 of 20 | 12 | symfony/demo, composer/composer, wp-cli/wp-cli-bundle, phpmyadmin/phpmyadmin, matomo-org/matomo, monicahq/monica, koel/koel, pixelfed/pixelfed, BookStackApp/BookStack, snipe/snipe-it, mautic/mautic, kimai/kimai, firefly-iii/firefly-iii, pterodactyl/panel, librenms/librenms, humhub/humhub, akaunting/akaunting | tests/fixtures/plugins/c3/composer.json, tests/fixtures/plugins/composer-patches/composer.json, tests/fixtures/plugins/craft/composer.json |
| lock-dev-version | 10 of 20 | 6 | phpunit/phpunit, wp-cli/wp-cli-bundle, matomo-org/matomo, monicahq/monica, koel/koel, snipe/snipe-it, mautic/mautic, firefly-iii/firefly-iii, librenms/librenms, humhub/humhub | tests/fixtures/root-alias/composer.json, tests/install_e2e.rs:841, tests/update.rs:948 |

**used, no fixture** (0): none.

**fixture, no use** (4, informational): provide, no-packagist, inline-alias(root), lock-aliases.

### Reading

Of the 23 Part 1 features, 0 are used by at least one corpus project but have no fixture-file or inline-literal hit, and 4 have fixture coverage with no corpus project (of 20 analysed) currently using them. The fixture-files column combines 145 tests/fixtures file hits and 92 inline tests/*.rs and src/**/*.rs #[cfg(test)] literal hits (one hit per file per feature).

