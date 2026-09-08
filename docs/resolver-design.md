# Resolver design

How `viv update` and `viv add` produce a `composer.lock` that Composer
accepts without rewriting it. Upstream references point at `composer/composer`
2.10.x, `src/Composer/`.

## Algorithm: port Composer's solver

Use a port of Composer's CDCL solver. Do not use the `pubgrub` crate.

PubGrub is the right tool when you own the resolution semantics. uv and
Concerto define their own; we do not. Our output must match whatever
`Solver::solve` produced, and the hard constraints are the easy half. The hard
half is which of several valid solutions gets picked, and that lives in
`DependencyResolver/DefaultPolicy.php`:

- Prefer alias over aliased (`DefaultPolicy.php:156-165`).
- Prefer the replaced package over its replacer (`:169-172`).
- Among mutual replacers, prefer the one whose vendor matches the requiring
  package's vendor (`:178-187`).
- Tie-break on pool insertion order, `$a->id` (`:191-195`).

PubGrub's `DependencyProvider` lets us inject version choice per package, but
not cross-package arbitration between providers of a virtual. Reproducing
`compareByPriority` inside PubGrub means reimplementing the pool ordering
anyway, and then debugging why a conflict-driven backjump landed somewhere
else. Every hour saved on the CDCL loop is spent on lock diffs we cannot
explain.

Scope of the port, about 2,500 lines of PHP:

| File | Lines | Port |
|---|---|---|
| `DependencyResolver/Solver.php` | 789 | Yes, in full |
| `DependencyResolver/RuleSetGenerator.php` | 342 | Yes |
| `Rule.php`, `Rule2Literals.php`, `GenericRule.php`, `MultiConflictRule.php` | 791 | Yes |
| `RuleWatchGraph/Chain/Node.php`, `Decisions.php` | 562 | Yes |
| `DefaultPolicy.php` | 297 | Yes, exactly |
| `Pool.php`, `PoolBuilder.php` | 1,289 | Yes; this is the metadata loader |
| `Transaction.php`, `LockTransaction.php` | 569 | Yes |
| `Problem.php`, `SolverProblemsException.php` | 914 | Stage 5; ship a blunt error first |
| `PoolOptimizer.php` | 479 | Yes (#76) |

`PoolOptimizer` removes pool entries that no rule can distinguish. Pruning
changes nothing about the answer, only the rule count, so it runs between
pool build and rule generation (`src/solver/pool_optimizer.rs`) with a
post-prune remap of `alias_of` back onto the surviving package IDs, gated on
producing an identical lock.

### Composer semantics and how each is handled

Everything below is a port, not a modelling trick, because we are porting the
solver. Listed so nothing is missed.

| Semantic | Where |
|---|---|
| `replace` / `provide` | `RuleSetGenerator` emits an implicit-obsolete rule per provided name; `Pool` indexes packages under all of `getNames()` |
| `conflict` | Negative binary rules, `RuleSetGenerator::addConflictRules` |
| `minimum-stability`, `stability-flags` | Pool-build-time filter, `Package/Version/StabilityFilter.php`; never reaches the solver |
| `prefer-stable` | `DefaultPolicy::versionCompare:59-69` |
| `prefer-lowest` | Flips the comparison operator, `DefaultPolicy:240` |
| Branch aliases (`dev-main as 3.x-dev`) | `AliasPackage` in the pool alongside the aliased package; policy prefers the alias |
| Inline aliases in `require` | Root aliases, `Installer::getRootAliases`; recorded in the lock's `aliases` array |
| Platform packages | `Request::fixPackage` for every entry of `PlatformRepository` (`Installer.php:1022-1035`), so they are irremovable |
| `--ignore-platform-req(s)` | `PlatformRequirementFilter`, applied at `createRepositorySet` (`Installer.php:938-946`) and inside `Solver::solve` |
| Partial update | `Request::setUpdateAllowList` plus the three `Request::UPDATE_*` modes (`Request.php:29-41`); `PoolBuilder` loads only the locked version for packages outside the list |
| `--with-all-dependencies` | `UPDATE_LISTED_WITH_TRANSITIVE_DEPS` |
| `dev-*` branches, `default-branch` | Metadata flag; `ArrayDumper` writes `default-branch: true` (`ArrayDumper.php:98-100`) |

### The dev split is a second solve

`Installer::doUpdate` solves once with `require` and `require-dev` merged
(`Installer.php:527`). It then calls `extractDevPackages`
(`Installer.php:701-744`), which builds a repository containing only the first
solve's result, and solves again with `require` alone. Packages in the first
result but not the second are `packages-dev`.

This ordering matters: dev requirements can pull a higher version of a non-dev
package into `packages`. Solving non-dev first and dev second gives a different
lock. Skip the second solve when `require-dev` is empty (`Installer.php:703`).

## Constraints and versions

Depend on `semver-php` 0.1.0 (MIT, `github.com/m1guelpf/semver-php`) behind a
thin `src/semver.rs` facade. It ports `VersionParser`, `Constraint`,
`MultiConstraint`, `Interval` and `Intervals`, the full surface the solver
needs.

Rejected:

- `composer-semver` 0.2.0 is EUPL-1.2. Copyleft, and not in the allow list in
  `deny.toml`.
- `riff-semver` is not published to crates.io; vendoring it from `shyim/riff`
  means owning it with no upstream.

`semver-php` has one release and few downloads, so treat it as a fork
candidate, not a dependency we trust. The facade makes replacement a one-file
change, and MIT means we can vendor it into `src/semver/` if it goes stale.

Gate it on the corpus before writing a line of solver. Commit these under
`tests/fixtures/composer/semver/`, generated from `composer/semver`'s tests:

- `VersionParserTest::successfulNormalizedVersions` and
  `failingNormalizedVersions` (already in `src/version.rs`).
- `VersionParserTest::simpleSpecs`, `wildcardConstraints`,
  `tildeConstraints`, `caretConstraints`, `hyphenConstraints`,
  `multiConstraints`, `failingConstraints`.
- `VersionParserTest::normalizeBranches`, `parseNumericAliasPrefix`.
- `ComparatorTest`, `SemverTest::satisfies`, `sort`, `rsort`.
- `IntervalsTest::compactConstraint`, `isSubsetOf`, `haveIntersections`.

If more than a handful fail, fork rather than patch upstream; the corpus is the
spec, not the crate. Keep `src/version.rs::normalize`; it already passes the
corpus and the facade can delegate to it.

## Metadata

Packagist v2 protocol, `Repository/ComposerRepository.php`:

- `packages.json` gives `metadata-url` (`:1537`), so `/p2/%package%.json`.
- Fetch `%package%~dev.json` as well, but only when dev stability is acceptable
  for that name; when only dev is acceptable, skip the non-dev file
  (`:1322-1331`).
- Expand `minified: "composer/2.0"` (`:1351`): each entry inherits the previous
  entry's fields, and a field set to `"__unset"` is removed.
- Re-normalise `version_normalized` when it equals `DEFAULT_BRANCH_ALIAS`
  (`:1360-1362`).
- HTTP cache under `$XDG_CACHE_HOME/vivace/repo/<repo-host>/`, key
  `provider-<name with / replaced by $>.json` (`:1160`), revalidated with
  `If-Modified-Since` (`:1889`, `:1968`). Reuse `src/fetch.rs`'s client.
- Concurrency: batch 100 names per wave, matching
  `PoolBuilder::LOAD_BATCH_SIZE`; the closure is loaded breadth-first as the
  pool builder discovers requires, and only versions matching the
  constraints accumulated so far are loaded, the same narrowing
  `PoolBuilder` itself applies, so a version an earlier require's constraint
  already rules out is never fetched. A prior lock's package names can seed
  the first wave's prefetch (`Repository::load_closure_seeded`, #90), a
  hint capped at the same `LOAD_BATCH_SIZE`, never a pool change.

Skip in v1 with a clear error: `available-packages` and
`available-package-patterns` (optimisations), `providers-api`,
`security-advisories`, v1 provider repositories, `path`/`vcs`/`artifact`
repositories. Inline `packages` in `packages.json` is cheap and worth
supporting (`:413`). `auth.json` credentials are already wired in `fetch`.

## Lock output

Top-level key order is fixed by `Locker::setLockData:370-393`: `_readme`,
`content-hash`, `packages`, `packages-dev`, `aliases`, `minimum-stability`,
`stability-flags`, `prefer-stable`, `prefer-lowest`, `platform`,
`platform-dev`, `platform-overrides` (only when non-empty),
`plugin-api-version`.

`_readme` is three fixed strings. `plugin-api-version` is the constant
`"2.9.0"` (`Plugin/PluginInterface.php:35`): hardcode it, do not derive it.

`Locker::fixupJsonDataType:457-470`: empty `stability-flags`, `platform` and
`platform-dev` serialise as `{}`, not `[]`; `stability-flags` is ksorted.
Aliases whose version is `dev-master`, `dev-trunk` or `dev-default` are
rewritten to `DEFAULT_BRANCH_ALIAS` (`:362-368`).

### Package entries

Field order comes from `Package/Dumper/ArrayDumper.php:29-147` (the same order
`installed.json` uses, see `docs/composer-contract.md`). `Locker::lockPackages:479-529`
then post-processes: drops `version_normalized` and `installation-source`,
moves `time` to the end, skips `AliasPackage` entries, sorts by `strcmp(name)`
then `strcmp(version)`. `dist.shasum` and `dist.mirrors` appear only when
non-empty; empty arrays are omitted throughout. `time` itself is renormalised
to RFC 3339 with a `+00:00` offset, whatever shape the source repository sent
(`Z`, a numeric offset, or a space-separated SQL timestamp), matching
`ArrayDumper`'s own `DateTime`-through-`P`-format round trip.

### content-hash

`Locker::getContentHash:89-119` md5s `json_encode($relevant, 0)`. Options `0`
means PHP escapes `/` as `\/` and non-ASCII as `\uXXXX`. `src/lock.rs`'s
`content_hash` does not do this, and every `require` key contains `/`, so the
hash we compute differs from Composer's for essentially every `composer.json`.
Harmless for the freshness check only while both sides are ours; fatal once
`viv update` writes a lock. Fix it in stage 1 with a PHP-compatible encoder,
gated on a fixture table of `composer.json` to known hash. The top level is
ksorted; nested values keep their original key order.

### Verification

For each fixture, in this order:

1. `viv update` then `git diff --exit-code composer.lock` against the committed
   Composer-generated lock. Byte equality, not JSON equality.
2. `composer validate --strict --no-check-publish`.
3. `composer install --dry-run` reports no operations.
4. `composer update --lock` leaves the file byte-identical.

Step 4 is the real test: it re-runs `setLockData` over our lock and rewrites it
if anything differs.

## Staging

Each stage is byte-diffable against Composer on `tests/fixtures/monolog` and
`tests/fixtures/legacy`. Record Packagist `/p2/` responses as fixtures under
`tests/fixtures/packagist/` so the tests are hermetic and pinned; a
`make record-packagist` target refreshes them.

1. **Semver facade and content-hash.** Add `semver-php`, wire `src/semver.rs`,
   commit the composer/semver corpus, fix `content_hash` PHP escaping. Accepts
   when the corpus passes and five real `composer.json` files reproduce their
   committed `content-hash`.
2. **Repository client.** `packages.json`, `/p2/` fetch, minified expansion,
   `~dev` files, HTTP cache with revalidation. No solving. Accepts when an
   internal metadata dump reproduces the package list Composer loads and a
   second run makes zero network requests.
3. **Pool and solver.** `PoolBuilder`, `Pool`, `RuleSetGenerator`, `Solver`,
   `DefaultPolicy`, `Decisions`, watched literals, `LockTransaction`. Full
   update only. Accepts when `viv update` on the monolog fixture selects the
   same package set and versions as the committed lock.
4. **Lock writer.** `ArrayDumper` port, `lockPackages` post-processing,
   top-level key order, `fixupJsonDataType`, the dev split via a second solve.
   Accepts when `viv update` on both fixtures produces a byte-identical
   `composer.lock` and all four verification steps pass.
5. **`require`, partial updates, error messages.** `viv add vendor/pkg`
   with constraint synthesis and a format-preserving `composer.json` write;
   `viv update vendor/pkg` with the three allow-list modes and
   `--with-all-dependencies`; a `Problem.php` port good enough to name the
   conflicting packages.

## Risks

- **Default update is not minimal.** `composer update` takes the highest
  allowed version and ignores the lock. Lock preservation is `--minimal-changes`
  only (`Installer.php:995-1006`), or packages outside a partial update's allow
  list. Implementing the wrong default produces a lock Composer rewrites on
  sight.
- **Repository drift.** Packagist gains releases continuously; recorded
  fixtures are the only stable signal. Any network-touching resolver test is
  advisory.
- **Rate limits.** Recorded fixtures keep CI off the network.
- **Composer version skew.** `plugin-api-version` moves with Composer. Pin the
  devbox Composer, assert it in the fixture test, fail loudly.
- **Toolchain floor.** Dependencies may raise `rust-version`; update the claim
  rather than let clippy's MSRV lint fail.
- **Performance rule.** The resolver runs only on `update` and `require`,
  never on `install`, so the no-slower rule holds as long as no resolver code
  is reached from the install path. Keep the module boundary strict.
