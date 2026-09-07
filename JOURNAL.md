# Journal

An engineering log for vivace, written as we go. Numbers, decisions, dead
ends. Newest entry last.

## 2026-09-06: study before code

**Goal.** Decide whether a Rust `composer install` built on a content-addressed
store with hardlinks can beat what already exists, before writing any of it.

**Prior art.**

Presto (Go, 230 stars) turned out to be a demo rather than a competitor. The
source has no download cache at all despite the README, so its "warm" install
is a cold one. It rewrites `composer.lock` on `install`, has no shasum check, is
open to zip-slip, and its autoloader is a hand-rolled closure with no classmap,
PSR-0 treated as PSR-4, and `ClassLoader` and `InstalledVersions` stubbed to
return nothing. Laravel cannot boot on it because package discovery reads
`vendor/composer/installed.json`, which Presto never writes.

Riff (Rust, started two weeks ago by a Shopware core developer) is the real
bar. It trusts the lock, ports Composer's transaction diff, vendors Composer's
`ClassLoader.php` and `InstalledVersions.php` verbatim, ports `PackageSorter`,
and reproduces `installed.json` key order. On a 101-package Laravel lock its
`vendor/composer/*` differed from Composer's in four small places: it ignores
`config.autoloader-suffix`, it puts `provide` after `require-dev`, it writes
`NULL` instead of `null` in `installed.php`, and it ships a newer
`InstalledVersions.php`. Everything else was byte-identical.

Reading Riff's installer showed where its time goes. There is no extracted
store: every warm install inflates every zip into `vendor/`, one create, copy
and chmod per file, at most ten archives at once on a two-worker Tokio runtime.
The cache key is name plus version rather than the dist reference, so two
`dev-main` locks at different commits collide. A no-op install rebuilds the
whole autoloader and re-tokenises every classmap file. Installs are not atomic;
a crash leaves a partial package directory that the next run treats as
installed. HTTP is 1.1 only.

**Baseline.** Ryzen 9 7900X3D, ext4, PHP 8.4.24, Composer 2.10.2, 101
packages, three runs each. Scenarios: cold is no cache and no vendor, warm is
cache present and no vendor, no-op is vendor present.

| Tool | Cold | Warm | No-op |
|---|---|---|---|
| composer 2.10.2 | 7.30 s | 1.09 s | 0.50 s |
| riff 0.0.7 | 1.74 s | 0.23 s | 0.23 s |
| presto 0.1.12 | 6.46 s | 6.33 s | 3.41 s |

Riff's no-op equals its warm time. With `--no-audit` it drops to 157 ms, and
strace shows 25 `php` processes spawned during a no-op for platform detection.
Its warm time is almost all system calls from extraction. Raw hyperfine JSON is
in `bench/results/`.

**The thesis.** Two things Riff does not do, and one it does not test:

1. Extract each package once into a global store and hardlink into `vendor/`.
   One `link()` per file instead of inflate and write. Target under 100 ms
   warm for this lock.
2. Make no-op cheap: compare lock references against `installed.json`, spawn
   no PHP, touch no network, skip autoloader generation when nothing changed.
   Target around 10 ms.
3. Prove compatibility with a byte-diff against real Composer output in the
   test suite, and fix Riff's four diffs from the start.

Cold installs are download-bound; parity with Riff is fine there.

**Decisions.**

- Store files are read-only (mode 0444). Hardlinks share inodes, so an edit
  to a vendor file would silently change the store and every other project
  linked to it. Read-only makes the edit fail loudly instead. Projects that
  patch vendor use `--link-mode copy`.
- `vendor/bin` proxies wait for v0.1.1. The fixture has none; we warn when a
  package declares `bin`.
- Crate `vivace`, binary `viv`.

**Groundwork laid.** `devbox.json` pins PHP, Composer and hyperfine. The
`tests/fixtures/monolog` project depends on monolog and psr/log with a dev
dependency, a PSR-4 root namespace, a `classmap` directory containing an
interface, a class and a backed enum, and a `files` entry. Composer's output
for it, dev and `--no-dev`, is saved under `expected/` as the diff target.
`docs/composer-contract.md` records the exact rules Composer follows, taken
from its source. `ARCHITECTURE.md` describes the pipeline. Composer's three
verbatim files sit in `src/autoload/templates/`.

**Still open.** Which of Composer's own test fixtures port cleanly as
data-driven tests, and which of uv's cache, linking and snapshot-test
patterns to copy. Both are being studied before the plan is final.

## 2026-09-06, later: what to borrow, what to test against

**From uv.** uv's install path is the model, and reading it settled several
details. Cache buckets carry a version suffix (`archive-v0`) so a format
change is a rename and pruning is "delete anything not current". A package is
unzipped into a temporary directory inside the bucket and renamed into place;
if another process got there first, ours is discarded and theirs used. One
shared file lock on the cache root covers the whole process, and cleaning
takes it exclusively. Link modes fall back per file, but only the first file
may fall back, with a single warning; after that a failure is a real error.
Zip entry names are checked by walking path components rather than by regex.
Only the executable bit is kept from zip modes. Its test harness runs the
binary against an isolated cache directory and snapshots stdout and stderr
after replacing paths and timings with placeholders. All of that is copied.

uv does not protect against the hardlink footgun beyond copying the one file
it mutates itself. We add read-only store files on top.

**Test corpora.** Composer's own autoloader tests are byte-exact goldens
driven by small package setups, about 35 cases and 48 expected files, all MIT.
composer/class-map-generator ships a fixture tree of awkward PHP files with
expected class lists. composer/semver has a version normalisation table.
PackageSorter has eight rows that pin the `files` autoload order. Nineteen of
Composer's installer fixtures are true install-from-lock cases with a trivial
sectioned text format; Riff generates one test per file from them in a build
script, which we copy. Presto has no tests.

Working method from here: copy the failing test in first, watch it fail, make
it pass, commit.

## 2026-09-06, afternoon: from lock to vendor tree

**How the work runs.** Code is written by role agents against the plan, red
test first, then reviewed by a separate agent before commit. Two reviews so
far paid for themselves. The version normaliser sliced a string at byte four
and would have panicked on a branch name such as `café-dev`. The natural sort
port stripped leading zeros where PHP does not. The second review's own
expectations turned out wrong too: the coder ran real PHP and found that
`strnatcasecmp("0", "00")` is equal, because PHP strips a string's leading
zeros once before comparing. The test table now records what PHP does, not
what either of us assumed.

**Modules landed.** Version normalisation and the package sorter pass the
upstream tables. The lock parser keeps each package's raw entry with key
order intact for `installed.json`, lowercases names, accepts `bin` as a string
or an array, and trims `vendor-dir`. The class scanner passes
class-map-generator's fixture tree, including a file that is not valid UTF-8,
and after review matches exclusion patterns against both the literal and the
canonical path, survives directory symlink cycles, and skips broken links
with a warning instead of aborting.

The store, fetch, link and plan modules take a lock to a `vendor/` tree.
Archives are keyed by the sha256 of their bytes, so identical zips under
different references share one extracted tree; a per-package pointer under
`dists-v0/` names the archive. Extraction goes into a temporary directory and
is renamed into place, with the loser of a race adopting the winner's copy.
Files end up read-only, executables at 0555. Linking builds a temporary
sibling of the package directory and renames it over the old one, so a
package install is atomic where Riff's is not. One design change from the
plan: the download stream yields completed archives to the caller rather than
taking a callback, because storing an archive is blocking work that the async
loop wants to hand to a worker thread.

**Quota.** A spend limit on the Fable pool killed three coders mid-task. The
one with real progress resumed from its transcript; the others restarted.

**Next.** The autoloader generator against Composer's 27 golden cases,
`installed.json`, `installed.php` and `platform_check.php`, then the CLI and
the end-to-end byte-diff on the monolog fixture.

## 2026-09-06, evening: `viv install` works, and the numbers

**End to end.** The CLI wires lock, plan, store, fetch, link and autoloader
together. On the monolog fixture `vendor/` is byte-identical to Composer
2.10.2 in dev and no-dev mode, PHP autoloads every class the fixture
declares, a second run is a no-op, and vendor files are read-only hardlinks
into the store. On the 101-package Laravel lock all thirteen generated files
match Composer's output exactly and the package trees are identical apart
from `vendor/bin`, which v0.1 does not write.

**Benchmark.** Same lock, vendor and cache on one filesystem, three runs
each.

| Tool | Cold | Warm | No-op |
|---|---|---|---|
| composer 2.10.2 | 8.28 s | 1.69 s | 0.50 s |
| riff 0.0.7 | 1.75 s | 0.26 s | 0.23 s |
| viv 0.1.0 | 2.22 s | 0.146 s | 0.010 s |

Warm is where the store pays: 107 ms of system time linking against 258 ms
for Riff inflating. No-op is where not spawning PHP pays: ten milliseconds.
Cold is half a second behind Riff and that is the download layer, tracked
as the next piece of work.

**Reviews that mattered.** The store, link and plan review found twelve
things, five of them real: lock names and references went into store paths
unsanitised, a duplicate zip entry failed against an already read-only file,
replacing a package deleted the old tree before the new one was in place,
copy mode still produced read-only files, and the plan compared only the
dist reference so a reference-less package bumped in version was never
reinstalled. All fixed with a failing test first.

**A mistake worth recording.** The first run against the Laravel lock
happened with the project on a different filesystem from the cache. Every
package fell back to copying, and the warning meant to fire once fired a
hundred and one times. The benchmark was rerun on one filesystem; the
warning is being deduplicated.

**Tooling.** A Makefile fronts the devbox commands, CI runs the gate on
every push, and the README carries the table above.

## 2026-09-06, night: a real project

**First contact.** Sander pointed `viv` at a work project: 105 packages,
private GitHub repositories, a private Composer repository, everything
locked to dev branches. The first run failed in a quarter of a second with
a 404: `viv` sent no credentials. Composer reads `auth.json` from the
project and from its home directory, plus `COMPOSER_AUTH`, and sends a
token per host. That was out of scope for v0.1 and lasted until the first
real project. It is in now, with one wrinkle worth knowing: reqwest strips
the Authorization header on a cross-host redirect, and GitHub's API
redirects zipball requests from api.github.com to codeload.github.com, so
`viv` follows redirects itself and re-applies the credential for each host.

**Then the diff.** Installing the same lock with both tools side by side
found four differences the fixtures had never exercised. Packages whose
vendor name is `composer` live in `vendor/composer/`, the directory that
holds `installed.json`, and Composer's shortest-path rule writes their
`install-path` as `./installers` where `viv` wrote `../composer/installers`.
Dev-branch packages carry an alias in `installed.php`: the `branch-alias`
from `extra` if there is one, normalised and with `.9999999` runs collapsed
back to `.x`, or `9999999-dev` for a default branch with no alias, and
nothing for numeric branches such as `1.x-dev`. Two provided virtual
packages named `php-http/...` vanished because the platform-package filter
matched `php` as a prefix; Composer's rule is an exact regex. And an earlier
review "fix" that deleted a package's top-level `.DS_Store` was wrong:
Composer ignores that file only when deciding whether an archive has a
single top directory, it does not delete it.

After those fixes the two trees are byte-identical apart from `vendor/bin`.
Warm install on 47,270 files: 0.61 s against Composer's 3.19 s. A second
run is a no-op.

**Lesson.** The golden corpora caught the generator's logic. The real
project caught the assumptions around it: credentials, the `composer/`
vendor name, dev-branch aliases, a too-broad regex. One afternoon with a
real lock file was worth more than another fixture.

**Later that night: `vendor/bin`.** The same project has seven binaries
(phpcs, phpcbf, carbon, tus, cs2pr, and two shell-wrapped sniffer tools),
which made Composer's output for it the golden set. The PHP proxy with its
stream-wrapper block and the shell proxy are ported verbatim; mode follows
the umask as Composer's does. One deliberate difference: Composer also
chmods the target inside the package, and `viv` does not, because package
files are read-only hardlinks into the shared store. The exec bit comes from
the zip, and it was there for all seven. With this the entire `vendor/`
tree matches Composer's; the only extra file is `.vivace-state`.

## 2026-09-06, late: milestone 0.2 in flight

Work ran in file-disjoint lanes and landed together. Cold install was
measured before anything changed: network-bound, hashing and extraction
under 300 ms in total. Download concurrency went from 16 to 64 and
extraction now overlaps downloads in a bounded task set; cold dropped from
3.98 s to 2.29 s in one session against Riff's 1.69 s. HTTP/1.1 against
HTTP/2 made no measurable difference, so the remaining gap is still open
(#1).

The optimised autoloader modes (`-o`, `-a`, `--apcu-autoloader`) and
include-path support pass the eight golden cases that had been skipped, and
per-package `include-path` is read from the lock. A second real-package
fixture (PEAR-style PSR-0, `target-dir`, `files` across polyfills,
phpunit's classmap tree, `vendor/bin` for phpunit and php-parse) is
byte-identical to Composer in dev and no-dev mode. It exposed one more
proxy branch: Composer rewrites `__DIR__` and `__FILE__` through its stream
wrapper when the target script uses them, and adds an isolation list for
phpunit. Eleven of Composer's nineteen install-from-lock fixtures run
against the planner; the other eight need a constraint solver and are
listed with reasons. Benchmarks now use isolated caches after one run
wiped a developer's store.

Lesson repeated: goldens catch logic, real projects catch assumptions, and
regenerating fixtures inside a git checkout makes Composer guess the root
version from the repository.

## 2026-09-06, evening: milestone 0.2 closed

The last cold-install finding was a bug of our own making. The guard that
capped concurrent extractions sat on the `select!` branch that polls the
download stream, so while eight archives were being unpacked none of the
sixty-four in-flight downloads made progress. Moving the cap into a
semaphore inside the extraction task fixed it: 2.52 s to 2.13 s in an A/B
run, and viv and Riff now trade places between runs. Repeated cold runs
against GitHub throttle visibly within minutes, so the honest statement is
parity within noise, recorded as such in the results. Riff does not rewrite
GitHub API URLs to codeload as I had assumed; it mirrors Composer exactly,
so there was nothing to copy there.

macOS passes the whole suite once the bin proxies used canonical paths
(`/var` is a symlink on macOS), and the job now blocks like Linux. Tar and
tar.gz dists, retries with backoff, GitLab credentials, `dump-autoload`,
and `--adopt` for a Composer-made vendor all landed with a failing test
first. The eight installer fixtures that need a solver moved to 0.3.

## 2026-09-06, later: beyond install

Milestone 0.3 asks what a Composer replacement does once installing from a
lock is solved. Today's answer in order of landing: a resolver design,
`viv normalize`, the audit fixes, lifecycle scripts, a Packagist client, a
`composer` shim, a release compatibility sweep, and a plugin strategy.

The resolver decision was the one worth a second opinion. PubGrub is the
obvious crate, and uv proves it, but uv owns its resolution semantics and
we do not. Our lock must contain the versions Composer would have picked,
and where several valid solutions exist Composer's `DefaultPolicy` breaks
the tie by alias, replacer, vendor and pool order. Reproducing that inside
PubGrub means porting the pool ordering anyway and then explaining why a
backjump landed elsewhere. So the plan is a straight port of Composer's
CDCL solver, about 2,500 lines of PHP, staged behind recorded Packagist
fixtures. Two corrections came out of the review: `composer update` does
not preserve locked versions by default, and our content-hash was wrong
for writing because PHP's `json_encode` escapes `/`. Both are fixed or
filed. `docs/resolver-design.md` has the whole argument.

Two findings came from running viv against projects that were not written
as fixtures. viv created an empty `vendor/bin` on every install because
every fixture we had happened to contain a package with a binary. And a
project whose global Composer config prefers source for one vendor got
`installation-source: source` and a git checkout from Composer, which viv
cannot yet do. The compatibility sweep that found the first one now runs on
every tag against ten pinned popular projects and a seeded random sample
from Packagist.

The plugin inventory changed a plan. The only plugins in any lock we can
reach are the WordPress ones: composer/installers routes seventeen
packages of one project into `wp-content`, and viv had put them all under
`vendor/`. The earlier byte-identical result on that project was against a
plugin-free Composer run. Native adapters for pure path mapping come
first, file generators second, and everything else is refused loudly with
`--no-plugins` as the escape hatch.

Scripts nearly shipped with a quiet deviation: skipping them on the no-op
fast path. Composer fires `post-install-cmd` on every install and people
rely on it, so the fast path is now taken only when the root defines no
scripts. Script-free projects keep the 7 ms no-op.

## 2026-09-06, night: milestone 0.3 closed

The resolver shipped in five stages in one evening, each gated on a byte
diff. Stage 1 fixed the content-hash encoding and put `semver-php` behind
a facade with 474 of 476 composer/semver corpus rows passing; the two
skipped rows are unrelated dev branches that Composer treats as
incomparable and Rust's `Ordering` cannot express. Stage 2 recorded
Packagist's v2 metadata as fixtures so nothing in the suite touches the
network. Stage 3 ported the CDCL solver file by file, 2,700 lines, keeping
Composer's names. Stage 4 wrote the lock and reproduced both fixtures byte
for byte; Composer's own `validate --strict`, `install --dry-run` and
`update --lock` leave the files alone. Stage 5 added partial updates,
`require` and `remove`, with a hand-written balanced-JSON scanner standing
in for `JsonManipulator` because Rust's regex crate has no recursive
patterns. Two honest gaps went to 0.4: `--minimal-changes` parses but does
nothing, and multi-cause problem messages skip Composer's deduplication.

The release sweep earned its place before it was a day old. It found the
unconditional `vendor/bin`, a classmap scanner that did not skip dot
directories the way Symfony Finder does, and a metapackage with a dist zip
that viv linked into `vendor/` because it keyed on the dist instead of the
type. Each was a one-line fix with a test, and none would have shown up in
the fixtures we wrote ourselves. It also exposed a mistake of mine twice:
running Composer with `--ignore-platform-reqs` changes the files it writes,
so the byte diff was comparing different things.

Numbers on the 101-package lock, quiet machine: warm 140 ms, no-op 7 ms,
both below 0.2.0. Nothing added today touched the install fast path except
the plugin check and the normaliser, and both are cheaper than the noise.

## 2026-09-07: milestone 0.4 closed

Hardening, and the day the tools started finding bugs for us. The fuzzer
found a panic in the semver crate on its first run: a slice at a byte
offset inside a multi-byte character, reachable from `viv require` with a
non-ASCII constraint. The compat sweep found a class name containing a
raw 0xA9 byte that Composer preserves and we had been converting to
U+FFFD; class names are byte strings now, all the way to the PHP writers.
The profiler found that `viv update` builds a pool of 45,604 package
versions and half a million rules on the Laravel lock and spends most of
its nine seconds generating them. Composer takes 1.2 s because its
PoolOptimizer removes versions no rule can distinguish. We skipped it in
the port as having no semantic effect, which remains true, and it is now
the first item of 0.5 with a number attached.

Two speed changes landed by looking rather than by accident. Linking
packages from the store in a bounded thread pool took warm install on
101 packages from 140 ms to 42 ms; the zip extraction had been parallel
for weeks while linking ran one package at a time. And a classmap sidecar
keyed on the archive hash took `install -o` from 370 ms to 124 ms; the
remaining cost is the root package's own directories, which can never be
cached.

Plugin adapters grew from two to six: the phpcs installer, phpstan's
extension installer, spi and composer-patches all reproduce the real
plugin's output byte for byte from fixtures generated by Composer with the
plugin enabled. composer-patches was the delicate one: patched files must
not reach the read-only store, so a patched package is relinked as a copy,
and a fingerprint of the patch set forces a re-patch when only the patches
change.

Scope got a proper statement. viv covers the commands people run every
day and hands the long tail to Composer through the shim; the README says
so now, and five items that only matched the command reference moved to
the backlog. One concurrency lesson: nine agents on one working tree
produced three build-breaking collisions in an hour. Four at a time, with
named file ownership, produced none.

## 2026-09-07: milestone 0.5 closed

Reach. Nine issues, all about being usable outside this machine: private
repositories, the everyday read-only commands, and the update speed gap.

Private repositories turned into an archaeology exercise. Satis's current
output no longer emits the v1 `providers-url` protocol at all; it writes
v2 metadata plus a legacy `includes` fallback. So the v1 path is verified
by hand-built fixtures with correct hashes, and the byte-diff against a
real Satis build exercises the paths that still exist. Repository order
follows RepositorySet: the first repository to offer a version wins, a
non-canonical one only fills gaps.

`show`, `tree`, `outdated`, `why`, `audit` and `validate` all match
Composer's plain output byte for byte on the fixtures, with two honest
cuts: the detail view skips the licence and release-date lines that need
an SPDX list or today's clock, and `validate` cannot show root-level
errors the way ConfigValidator formats them because Composer itself throws
before reaching that code. `viv x` is the one thing here Composer does
not have: a tool resolved into its own environment under the cache and
executed, a cache hit costing a stat and an exec.

The update gap got three fixes and two honest remainders. Porting
PoolOptimizer cut the pool by 89% and the rules by 97%, and made the whole
thing slower, because the optimiser made 3.3 million constraint matches at
775 ns each. Composer's CompilingMatcher compiles a constraint once; so
now does the facade, matching pre-parsed version keys in a few
comparisons, proven equivalent over the corpus and the recorded Packagist
history. The sort comparator had the same shape and got the same fix.
8.7 s became 5.1 s. What remains is a constraint parse cache in pool
building and the 251 metadata revalidations that take 1.5 s even when
every answer is a 304; both are filed under 0.6, whose stated target is
to beat riff on every column.

Process lesson, again: three of today's broken HEADs came from committing
a shared file by eye while another agent was editing it. Every landing now
goes through a worktree at HEAD with exactly the staged files copied in.

## 2026-09-07, evening: first sweep over real client projects

Thirteen WordPress projects from daily work, every one plugin-heavy
(`composer/installers`, core installers, patches, Altis), most with private
Satis or Packagist repositories. Copied only `composer.json` and
`composer.lock` into a staging dir; the sweep never touches the originals.

Ten install byte-identical to Composer in both modes. One did not:
`installed.php` listed one alias where Composer listed two. The lock's
top-level `aliases` array (root `dev-master as 1.0.0`) was parsed and
dropped. Composer's `Locker` turns each entry into an `AliasPackage` that
stacks on the package's own branch alias, root alias first. Fixed with a
fixture whose golden is real Composer output. The other two rows were sweep
bugs, not viv bugs: one project sets `config.vendor-dir`, and the diff
compared `vendor/` that neither side wrote; and a download failure was
labelled "platform" because Composer's text mentioned
`--ignore-platform-req`. The remaining three projects need
`ffraenz/private-composer-installer` to sign the Gravity Forms download URL,
so with `--no-plugins` Composer itself fetches an HTML page and fails: no
baseline to diff. Plugin-adapter candidate.

Speed on the same lock files, install from lock, `--no-plugins --no-scripts`,
three runs, means. riff could not install any of them: it has no way to
read the private-registry credentials that the project-level `auth.json`
holds, and it refuses `composer/installers`.

| Project (anonymised) | Packages | composer cold / warm / no-op | viv cold / warm / no-op |
|---|---|---|---|
| A | 26 | 25.1 s / 1.14 s / 0.39 s | 3.0 s / 0.042 s / 0.006 s |
| B | 473 | 31.8 s / 2.11 s / 0.48 s | 5.4 s / 0.093 s / 0.051 s |
| C | 442 | 28.7 s / 2.56 s / 0.48 s | 14.8 s / 0.296 s / 0.051 s |

Project C's cold number is a git clone: one package has no dist and viv
clones it serially after the archives. Two gaps surfaced for `update`:
`viv update` rejects `--no-plugins` and `--no-scripts`, which `install`
accepts, and it refuses any lock whose `composer.json` declares a `vcs`
repository, which every one of these projects does.

Housekeeping: the sweep's default `mktemp -d` scratch was never removed;
twenty of them held 18 GB. It now cleans up on exit unless
`COMPAT_SCRATCH` names the directory.
