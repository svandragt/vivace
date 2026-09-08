# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [Unreleased]
### Added
- Wire --minimal-changes into viv update ([#61](https://github.com/svandragt/vivace/issues/61))
- Problem messages: port formatDeduplicatedRules and condenseVersionList ([#62](https://github.com/svandragt/vivace/issues/62))
- viv diagnose: environment and configuration report ([#81](https://github.com/svandragt/vivace/issues/81))
- Plugin adapters: yiisoft/yii2-composer and craftcms/plugin-installer ([#92](https://github.com/svandragt/vivace/issues/92))
- Drupal support: drupal/core-composer-scaffold and drupal/core-project-message ([#93](https://github.com/svandragt/vivace/issues/93))
- release-age and licence lines in viv show ([#94](https://github.com/svandragt/vivace/issues/94))
- Adapter for ffraenz/private-composer-installer ([#98](https://github.com/svandragt/vivace/issues/98))
- Adopt a Composer-written vendor/ by default; confirm only through the shim ([#123](https://github.com/svandragt/vivace/issues/123))
- Root package version: guess from git like Composer's VersionGuesser ([#125](https://github.com/svandragt/vivace/issues/125))
- Adapter for codeception/c3 ([#126](https://github.com/svandragt/vivace/issues/126))
- Automatic adopt of a Composer-written `vendor/` is best-effort per package: a package that fails to adopt falls back to a fresh install rather than aborting the run
- The compat sweep now runs plugins on both sides where an adapter is native, so the comparison no longer penalises adapted plugins for staying off

### Fixed
- update_reproduces_the_legacy_lock fails without php on PATH ([#109](https://github.com/svandragt/vivace/issues/109))

### Tooling
- Inline lock_writer's copied content_hash/php_json_encode into lock.rs ([#107](https://github.com/svandragt/vivace/issues/107))
- Share FixtureTransport via tests/common instead of three copies ([#108](https://github.com/svandragt/vivace/issues/108))
- Remove dead package_rule_ids and deduplicate php_string ([#110](https://github.com/svandragt/vivace/issues/110))
- Collapse pool_builder::build onto build_partial ([#111](https://github.com/svandragt/vivace/issues/111))
- Extract locked_by_name helper in tests/require.rs ([#112](https://github.com/svandragt/vivace/issues/112))
- Compat sweep: run native-adapter projects with plugins on and count refusals ([#124](https://github.com/svandragt/vivace/issues/124))

## [0.6.0] - 2026-09-07
### Added
- CHANGELOG.md generated from closed milestone issues at release time ([#80](https://github.com/svandragt/vivace/issues/80))
- Homebrew tap and Debian package built from the release tarballs ([#82](https://github.com/svandragt/vivace/issues/82))
- Stability policy: what viv promises and how it versions ([#83](https://github.com/svandragt/vivace/issues/83))
- Run the compat sweep on a schedule, not only on tags ([#84](https://github.com/svandragt/vivace/issues/84))
- Add riff to the update-warm benchmark ([#87](https://github.com/svandragt/vivace/issues/87))
- 0.6 target: faster than riff on every scenario ([#88](https://github.com/svandragt/vivace/issues/88))
- Cache parsed constraints when building pool packages ([#89](https://github.com/svandragt/vivace/issues/89))
- Metadata closure fetch dominates warm update at ~1.5 s ([#90](https://github.com/svandragt/vivace/issues/90))
- Resolver at Composer's speed on large locks ([#91](https://github.com/svandragt/vivace/issues/91))
- Normalise composer.json on update, require and remove, not on install ([#95](https://github.com/svandragt/vivace/issues/95))
- viv update rejects --no-plugins and --no-scripts that install accepts ([#96](https://github.com/svandragt/vivace/issues/96))
- viv update refuses a composer.json with a vcs repository ([#97](https://github.com/svandragt/vivace/issues/97))
- Aliases: viv add for require, viv rm for remove ([#99](https://github.com/svandragt/vivace/issues/99))
- Adapter for php-http/discovery ([#101](https://github.com/svandragt/vivace/issues/101))
- Rerun the client corpus sweep and bench before tagging 0.6 ([#102](https://github.com/svandragt/vivace/issues/102))
- viv update fails on wpackagist metadata: provider entry is not a list ([#105](https://github.com/svandragt/vivace/issues/105))
- Bench the public compat corpus, not only the Laravel lock ([#106](https://github.com/svandragt/vivace/issues/106))

### Changed
- make install should not put the composer shim on PATH by default ([#103](https://github.com/svandragt/vivace/issues/103))
- update, require and remove should install after writing the lock, as Composer does ([#104](https://github.com/svandragt/vivace/issues/104))

### Fixed
- update: self.version in a dependency's require breaks the closure walk (bedrock, drupal) ([#115](https://github.com/svandragt/vivace/issues/115))
- update: pool optimizer leaves alias_of unremapped, panics on phpunit/phpunit ([#116](https://github.com/svandragt/vivace/issues/116))
- update: root replace/provide ignored, symfony/demo lock gains four polyfills ([#117](https://github.com/svandragt/vivace/issues/117))
- update: php-64bit and lib-* platform packages missing from the solver ([#118](https://github.com/svandragt/vivace/issues/118))
- update: honour available-package-patterns so wpackagist isn't asked about every name ([#119](https://github.com/svandragt/vivace/issues/119))
- update: write the lock's time field as RFC 3339 with +00:00 like ArrayDumper ([#121](https://github.com/svandragt/vivace/issues/121))

### Performance
- update: cap closure fetch concurrency under the h2 stream limit and take the version scan off the fetch loop ([#120](https://github.com/svandragt/vivace/issues/120))

## [0.5.0] - 2026-09-07
### Added
- Release workflow: prebuilt binaries for Linux and macOS on tag push ([#65](https://github.com/svandragt/vivace/issues/65))
- Composer-type repositories beyond Packagist: Satis, Private Packagist, GitLab and GitHub package registries ([#67](https://github.com/svandragt/vivace/issues/67))
- viv audit: security advisories from Packagist ([#68](https://github.com/svandragt/vivace/issues/68))
- viv show and viv outdated ([#69](https://github.com/svandragt/vivace/issues/69))
- viv validate ([#70](https://github.com/svandragt/vivace/issues/70))
- viv x: run a tool from Packagist without installing it into the project (uvx equivalent) ([#85](https://github.com/svandragt/vivace/issues/85))
- viv update-lock and viv tree as first-class commands ([#86](https://github.com/svandragt/vivace/issues/86))

### Performance
- Prune the solver pool before rule generation (no PoolOptimizer) ([#76](https://github.com/svandragt/vivace/issues/76))
- Classmap-scan cache hits still cost ~40-50ms on install -o ([#77](https://github.com/svandragt/vivace/issues/77))
- link_tree's create_dir_all issues far more mkdir than needed ([#78](https://github.com/svandragt/vivace/issues/78))

## [0.4.0] - 2026-09-07
### Added
- viv cache: prune, clean, size, gc of unreferenced archives ([#20](https://github.com/svandragt/vivace/issues/20))
- Limits on extraction: size caps and entry counts ([#21](https://github.com/svandragt/vivace/issues/21))
- Windows support ([#22](https://github.com/svandragt/vivace/issues/22))
- Offline mode and --prefer-dist-cache behaviour ([#23](https://github.com/svandragt/vivace/issues/23))
- Bitbucket OAuth token exchange and remaining auth.json types ([#24](https://github.com/svandragt/vivace/issues/24))
- Cache scanned classmaps per archive for -o installs ([#25](https://github.com/svandragt/vivace/issues/25))
- Release engineering: binaries, cargo-binstall, checksums ([#26](https://github.com/svandragt/vivace/issues/26))
- tar.bz2 dists ([#27](https://github.com/svandragt/vivace/issues/27))
- Small ponytail ceilings: sh proxy marker, adopt confirmation, content-hash slash escaping, pointer temp names ([#29](https://github.com/svandragt/vivace/issues/29))
- Small drop-in differences: target-dir removal, stale sh proxies, empty vendor/bin, absolute vendor-dir depth, summary wording ([#37](https://github.com/svandragt/vivace/issues/37))
- Honour preferred-install: source packages record installation-source source and keep a git checkout ([#43](https://github.com/svandragt/vivace/issues/43))
- Plugin adapters: phpcodesniffer-composer-installer, tbachert/spi, phpstan/extension-installer ([#52](https://github.com/svandragt/vivace/issues/52))
- Plugin adapter: cweagans/composer-patches ([#53](https://github.com/svandragt/vivace/issues/53))
- Git source checkouts: match Composer's .git/config ([#59](https://github.com/svandragt/vivace/issues/59))
- Partial update of a transitively-required package fails: not found in any version ([#79](https://github.com/svandragt/vivace/issues/79))

### Fixed
- Prune empty parent directories after removing a package ([#56](https://github.com/svandragt/vivace/issues/56))
- Classmap differs from Composer on choks/password-policy-bundle ([#71](https://github.com/svandragt/vivace/issues/71))
- macOS CI: snapshot filters must match the canonical /private/var temp path ([#72](https://github.com/svandragt/vivace/issues/72))
- parse_constraint panics on multi-byte input (semver-php slices at a non-char boundary) ([#73](https://github.com/svandragt/vivace/issues/73))

### Tooling
- Fuzz the hand-written parsers with cargo-fuzz ([#44](https://github.com/svandragt/vivace/issues/44))
- Publish test coverage from CI with cargo-llvm-cov ([#45](https://github.com/svandragt/vivace/issues/45))
- Add cargo-machete to make check ([#46](https://github.com/svandragt/vivace/issues/46))
- Silence cargo-deny licence warnings by allowing them explicitly ([#47](https://github.com/svandragt/vivace/issues/47))
- Deny rustdoc warnings in make check and CI ([#48](https://github.com/svandragt/vivace/issues/48))
- Profile viv install and record where the time goes ([#54](https://github.com/svandragt/vivace/issues/54))
- Profile viv update and viv require once the resolver lands ([#55](https://github.com/svandragt/vivace/issues/55))
- compat sweep: keep the last lines of composer's output for skipped rows ([#64](https://github.com/svandragt/vivace/issues/64))

## [0.3.0] - 2026-09-07
### Added
- Port Composer's 19 install-from-lock installer fixtures ([#4](https://github.com/svandragt/vivace/issues/4))
- Dependency resolver: viv update and viv require ([#10](https://github.com/svandragt/vivace/issues/10))
- Scripts: lifecycle and custom ([#11](https://github.com/svandragt/vivace/issues/11))
- Plugin strategy ([#12](https://github.com/svandragt/vivace/issues/12))
- path and vcs repositories ([#13](https://github.com/svandragt/vivace/issues/13))
- viv normalize ([#14](https://github.com/svandragt/vivace/issues/14))
- composer shim ([#15](https://github.com/svandragt/vivace/issues/15))
- CI: fail on warm or no-op benchmark regressions ([#18](https://github.com/svandragt/vivace/issues/18))
- installed.php gaps: root replace/provide, natural sort case, numeric alias check ([#28](https://github.com/svandragt/vivace/issues/28))
- bin-dir default must derive from vendor-dir ([#30](https://github.com/svandragt/vivace/issues/30))
- plan: a kept package must exist on disk ([#31](https://github.com/svandragt/vivace/issues/31))
- store: verify archive completeness with a marker, not directory existence ([#32](https://github.com/svandragt/vivace/issues/32))
- install: check lock freshness against composer.json ([#33](https://github.com/svandragt/vivace/issues/33))
- fetch: enforce https (secure-http) and redact URL credentials in logs and errors ([#34](https://github.com/svandragt/vivace/issues/34))
- lock: reject a package present in both packages and packages-dev ([#35](https://github.com/svandragt/vivace/issues/35))
- Clean up interrupted-install litter in vendor and add viv cache prune to the CLI ([#36](https://github.com/svandragt/vivace/issues/36))
- Resolver stage 1: semver facade and PHP-compatible content-hash ([#38](https://github.com/svandragt/vivace/issues/38))
- Resolver stage 2: Packagist v2 repository client with cache ([#39](https://github.com/svandragt/vivace/issues/39))
- Resolver stage 3: pool and CDCL solver port ([#40](https://github.com/svandragt/vivace/issues/40))
- Resolver stage 4: lock writer and dev split ([#41](https://github.com/svandragt/vivace/issues/41))
- Resolver stage 5: viv require, partial update, problem messages ([#42](https://github.com/svandragt/vivace/issues/42))
- Native adapter for composer/installers and wordpress-core-installer, refuse unknown plugins ([#51](https://github.com/svandragt/vivace/issues/51))

### Fixed
- viv creates an empty vendor/bin when no package declares a binary ([#50](https://github.com/svandragt/vivace/issues/50))
- Classmap scanner must skip dot files, dot directories and VCS directories like Symfony Finder ([#60](https://github.com/svandragt/vivace/issues/60))
- Metapackages with a dist must not be installed ([#63](https://github.com/svandragt/vivace/issues/63))

### Tooling
- Release compatibility sweep against popular and random Packagist projects ([#49](https://github.com/svandragt/vivace/issues/49))
[0.3.0]: https://github.com/svandragt/vivace/releases/tag/v0.3.0
[0.4.0]: https://github.com/svandragt/vivace/releases/tag/v0.4.0
[0.5.0]: https://github.com/svandragt/vivace/releases/tag/v0.5.0
[0.6.0]: https://github.com/svandragt/vivace/releases/tag/v0.6.0
[0.7.0]: https://github.com/svandragt/vivace/releases/tag/v0.7.0
