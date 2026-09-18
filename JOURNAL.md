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

**Normalisation moved off `install`.** Since 0.2 `viv install` rewrote
`composer.json` into normalised form before reading it. That was landed
with four other changes and never argued for in this journal. It breaks
the contract that install writes only under `vendor/`: a CI checkout came
back with a dirty `composer.json`. Now `update`, `require` and `remove`
normalise when they write, which is where merge conflicts arise anyway,
and `install` and `dump-autoload` leave the file alone. `--no-normalize`
on install is a hidden no-op for one release (#95).

## 2026-09-07, afternoon: milestone 0.6 closed

Four hours, fourteen issues, one morning's sweep of thirteen client
projects driving the priorities. The milestone was renamed from "faster
than riff" to "client projects and update speed" once it was clear that
half the value was making `update` work on real projects at all.

**Update works on real projects.** Every client project declares a `vcs`
repository, and `viv update` refused all of them. A VCS repository is now a
source like a Composer one: list refs, read `composer.json` at each, feed
the same `PackageVersion` list to the pool. A git driver mirrors the remote
under the cache; a GitHub driver uses the API through the existing
transport with `auth.json`'s token and falls back to git. The byte-diff
test against Composer caught one thing on the first pass: Composer strips
`file://` from a local URL before the driver sees it, so `source.url` in
the lock never carries the scheme.

**Update installs.** `viv update`, `require` and `remove` wrote the lock
and stopped. Composer installs in the same run and fires the update
events. They now chain into the install path, `pre-update-cmd` before
resolving and `post-update-cmd` after the install, never the install-cmd
pair. `--no-install` keeps the old behaviour. Found while landing flag
parity for `--no-plugins` and `--no-scripts`, which `update` rejected.

**Speed.** Parsed constraints are cached across the pool builder and the
optimiser; warm update on the Laravel lock went from 5.18 s to 4.55 s
with an identical lock. The metadata closure fetch turned out to be a
latency floor, not a hotspot: 251 provider requests over nine dependency
levels, already concurrent within a level. Removing the per-level barrier
landed as a tidy-up and moved nothing. The real fix is to seed the closure
from the lock's package names so nine levels become two; filed for 0.7.
riff gained an update-warm column and could not fill it: riff 0.0.7 reports
zero versions of laravel/framework whatever the constraint. Tracked locally
for an upstream report.

**Adoption.** php-http/discovery gets a native adapter for its generated
strategy file, from a real-Composer golden. `viv add` and `viv rm`.
`make install` no longer puts the composer shim on PATH. A weekly
scheduled sweep opens a drift issue when a pinned project starts
differing. Debian packages and a Homebrew formula ship with each release,
and release notes come from the milestone's closed issues. A stability
policy says what viv promises.

**Corpus check before the tag.** All ten testable client projects are
byte-identical to Composer in both modes; the three that need
`ffraenz/private-composer-installer` stay skipped (#98).

Not closed: the four-column target. viv beats riff warm and no-op by an
order of magnitude and Composer on update is the remaining loss; cold
install sits at 2.2 s against riff's 1.6 s, bounded by GitHub's zipball
throttling rather than anything in the pipeline. Carried to 0.7 with the
resolver-speed work.

**Bench on the three largest client projects**, install from lock,
`--no-plugins --no-scripts`, three runs, network included so cold moves
with the day:

| Project | Packages | composer cold / warm / no-op | viv cold / warm / no-op |
|---|---|---|---|
| A | 26 | 30.3 s / 1.58 s / 0.68 s | 7.9 s / 0.043 s / 0.006 s |
| B | 473 | 33.0 s / 2.11 s / 0.49 s | 8.8 s / 0.100 s / 0.050 s |
| C | 442 | 26.3 s / 2.49 s / 0.55 s | 23.1 s / 0.301 s / 0.049 s |

riff installed none of them. And the bench found the next bug: `viv
update` on all three fails on wpackagist's metadata, whose provider files
key versions by string instead of the v2 list. `install` is unaffected.
Filed as the first P1 of 0.7 (#105); 0.6 ships with update working on
Packagist, Satis and VCS repositories and not yet on wpackagist.

**Correction, same afternoon.** The 0.6 tag above was withdrawn within
minutes. Two release criteria were set: `update` must work on the client
projects, and viv must be faster than Composer on every benchmark column.
The first was met the same afternoon: wpackagist's provider files key
versions by label rather than listing them, and the client rejected them;
fixed with a recorded fixture and a lock-identity test against the real
registry. The second is open: warm update on the Laravel lock is 3.99 s
against Composer's 1.16 s after sharing package metadata by `Arc`, with
the metadata round trips and the pool optimiser's string keys as the next
two targets. A corpus benchmark over the pinned public projects landed so
riff can be compared on projects it can install; its first run is partial.
0.6 stays open on #88, #90, #91 and #106.

## 2026-09-07, late: codebase audit and the update-speed work

**Audit.** A structural audit of the repository at 299aa28 found the
source tight (4% duplication, one dead public function, clippy clean at
pedantic) and one habit worth naming: parallel agent lanes copied helpers
across module boundaries rather than widening a private function's
visibility, and wrote a comment saying so. `lock_writer.rs` carried a
verbatim copy of `lock.rs`'s content-hash and PHP-exact JSON encoder;
`pool_builder::build` restated `build_partial`; three test files each
declared the same fixture transport. The six findings are #107 to #112.
Five landed today; #111 (folding `build` onto `build_partial`) waits on a
bench run. The same habit appeared again during the session in the new
`diagnose` module, which arrived with its own copy of the cache-dir
resolver; it now calls the shared one, and the remaining duplicate between
`install.rs` and `update.rs` is parked.

**0.7 confidence, landed.** `viv show` prints Composer's released, license,
suggests and provides lines, with `composer/spdx-licenses`' resource file
embedded so every identifier renders as Composer does, and `outdated
--format=json` carries the release-age fields (#94). `viv diagnose` reports
cache, auth sources by host only, tool versions, platform packages and the
plugin decision per lock entry (#81).

**0.6 adoption, the update column.** Two attacks on the 3.99 s warm update.
Seeding the metadata closure from the lock collapsed nine fetch waves to
one and changed nothing: the fetch phase stayed at 1.47 s. Measuring each
request showed why: 251 conditional requests, all honest 304s over a single
HTTP/2 connection, median 130 ms server latency each. Composer makes 109
requests for the same project, and those are 108 distinct files. viv's
closure discovers 251 distinct package names where Composer's needs 108,
because Composer's pool builder only loads versions that satisfy the
constraints accumulated for a name and only queues those versions'
requires, while viv walks the requires of every version. That port is the
next step for #90 and shrinks the pool #91 optimises.

The pool optimiser's string identity keys became hashed `u64`s: the
optimiser span went from 1432 ms to 589 ms and warm update from 4.28 s to
3.50 s on the same tree snapshot, lock byte-identical (#91).

**Method note.** Four lanes on one working tree worked with named file
ownership, but the bench gate suffers: a before/after `cmp` of the lock
fails when another lane's edit lands between the two builds, and hyperfine
means drift while others compile. Lanes isolated by building the "before"
binary from a `git worktree` at HEAD and swapping only their own file for
the "after". The corpus benchmark (#106) still needs a quiet machine.

Full suite under devbox at the end of the session: 562 passed, 6 network
tests skipped. 0.6 stays open on #88, #90, #91 and #106.

**Addendum, same evening.** Porting Composer's constraint-filtered
discovery closed the gap: the closure walk now tracks the union of
constraints per name, loads only versions that satisfy it, and queues only
those versions' requires. bench/laravel: 108 metadata requests instead of
251 (Composer's own count), pool 615 packages instead of 5169, optimiser
613 ms to 19 ms, warm update 3.42 s to 1.19 s, lock byte-identical. A
quiet-machine run of all three tools puts viv ahead of Composer on every
Laravel column, update-warm 1.12 s against 1.23 s; riff keeps cold by half
a second. #90 and #91 closed.

The corpus run tells a less tidy story: viv wins warm and no-op on every
project, but update-warm trails Composer on laravel/laravel at HEAD and on
symfony/demo, is n/a on three projects for reasons the script lost when
it aborted on yii2-app-basic, and cold is mixed against riff. #106 stays
open with those two follow-ups; #88's release criterion against Composer
is met on bench/laravel.

Also today: a question about composer-patches and shared hardlinks. The
adapter relinks the patched package as a copy through `link_tree`'s
temp-directory swap, so the store is never written through; a regression
test asserting that is parked.

## 2026-09-07, night: the corpus closes the update gap

The corpus bench, fixed to write footnotes per project and to keep going
past a failing tool, turned the evening's "update is slower on three
projects" into five bugs and three profiles.

**Bugs, all found by benching real projects and all closed today.** The
constraint-filtered closure walk parsed `self.version` literally (#115,
drupal and bedrock). The pool optimiser remapped `request.fixed` after
pruning but not each alias's `alias_of` (#116, phpunit panicked).
The solver ignored the root package's `replace` and `provide`, so
symfony/demo's lock gained four polyfills Composer omits (#117). The
platform repository stopped at `php` and `ext-*`; `php-64bit` and `lib-*`
are now ported from PlatformRepository, 84 of 84 names and versions
matching `composer show --platform` (#118, statamic and craft). A v2
repository's `available-package-patterns` was ignored, so bedrock asked
repo.wp-packages.org about every name (#119). And the lock's `time`
field went out verbatim where Composer rewrites it as RFC 3339 with
`+00:00` (#121, one wp-theme entry).

**Speed, from measurement rather than guesswork.** A request timeline
showed the lock-seeded prefetch opening 153 streams on one HTTP/2
connection whose server limit is 128, with per-request latency rising
from 136 ms under 50 in flight to 905 ms above 100; the burst now goes
through a cap of 100, chosen by a sweep (#120). Provider bodies over 64 KB
are parsed on the blocking pool (an ungated version slowed bench/laravel
by 18%, so the gate matters). And the constraint scan re-tested every
rejected version against the whole constraint list on every widen;
symfony/http-foundation alone saw 280k `matches` calls in statamic's
closure. Testing only the constraints added since the last scan cut
statamic's closure from 1565 ms to 977 ms.

**Where the corpus stands.** Warm update: viv ahead of Composer on every
project that completes, from laravel HEAD (1.43 s vs 1.75) to statamic
(1.88 s vs 2.40). Warm install and no-op an order of magnitude ahead of
both tools everywhere. Cold: ahead of Composer everywhere, level with
riff within noise. yii2-app-basic drops out because riff applies a
dependency's patches under `--no-plugins` and fails; Composer and viv
install it. #106 closed with a corpus table in the README.

**Method notes.** Four lanes on one tree with named file ownership held
up, but two lanes copied a helper rather than widen one, the audit's
finding repeating in miniature; one lane's mis-typed test truncated a
committed JSON and restored it. Bench numbers under concurrent compiles
are unusable; every before/after tonight was taken on a quiet machine or
from a worktree build of HEAD against the working tree. riff was
reinstalled so its column is measured, not carried over.

**Release.** Cutting the tag surfaced two more contract gaps the sweep had
never reached: drupal's `include_paths.php` follows Composer's install
order rather than the lock's, and archives may carry symlink entries,
which both extractors had been dropping (gotenberg/gotenberg-php ships
three). Symlinks are now written after every regular entry, with escaping
targets refused. The shim tests' one-in-three failure turned out to be
`ETXTBSY` from a copy still open when another test forked; a mutex
settles it. Sweep at the tag: 36 rows identical, none differ. v0.6.0
tagged.

## 2026-09-08, morning: parked follow-ups and a perf audit

**Audit residue cleared.** The night's four-lane session left two helper
copies behind, exactly the current the audit had named: `vcs.rs` carried a
third Hinnant `civil_from_days` and `install.rs` a private
`default_cache_dir` beside `update.rs`'s shared one. Both now route through
the one copy; the diff is thirteen lines in, thirty-nine out.

**Store safety under composer-patches.** `patches::apply` breaks the
hardlink with a `LinkMode::Copy` relink before `git apply`, but nothing
asserted the store's side of that. A new e2e test resolves the
`dists-v0` pointer for `psr/log` after a patched install and checks the
store file is unpatched, single-linked and still read-only.

**Perf audit, not yet acted on.** A read-only pass over the install path
found `composer.json` parsed three times before the no-op check (typed
root, content hash, scripts value) and `plan()` deep-cloning every kept
package's raw JSON that a no-op then discards. Store lookups touch the
pointer mtime serially per package. All three are tracked locally; the
fix is one parse in `run_impl` and a borrowing `plan.keep`, measured with
`make bench-check` before it lands.

## 2026-09-08, day: milestone 0.7 closed in one sitting

**Scope first.** Triage of 0.7 moved the research and slimming items to a
new 0.8 and Windows out of any milestone, leaving seven confidence items.
Three more arrived from checking the adapters on real projects rather than
fixtures: the sweep was passing `--no-plugins` to both sides, so no
adapter had ever been diffed against Composer; every git checkout without
a `version` got `1.0.0+no-version-set` where Composer reads the branch;
and yii2-app-basic refused on codeception/c3.

**Adopt by default.** A Composer-written `vendor/` is relinked from the
store without `--adopt`; the shim still asks on a terminal. The first cut
fetched every archive and so failed outright on a private dist, which the
old warning path had tolerated. Adoption is now best-effort per package.
The install stays Composer-consistent at every step: `installed.json` and
the state file are written only after the last link succeeds.

**Resolver.** `--minimal-changes` had parsed and done nothing. Wiring the
pin set into the solver was not enough: the pool optimizer pruned the
locked version as a duplicate before the solver saw the preference, the
same policy object has to reach both, as `Installer::createPolicy` does.
Problem messages gained the reason sort, `formatDeduplicatedRules` and
`condenseVersionList`, with goldens from a local repository; the conflict
line had also been naming the wrong side.

**Adapters.** yii2-composer, craft plugin-installer, private-composer-
installer, codeception/c3, drupal core-composer-scaffold and symfony/
runtime; core-project-message and core-recipe-unpack are inert for
install. The yii2 file exposed the fixture blind spot: one package cannot
tell lock order from install order, craftcms/craft could. The sweep now
runs plugins on both sides where every plugin is native and counts the
rest; drupal/recommended-project reads `plugins: native`, identical.

**Method.** Four lanes on one tree with named file ownership again; the
plugin registry was shared append-only without incident. Two helper copies
were left behind and are tracked locally, the audit's current in
miniature. The perf gate caught one regression before it landed: a `git`
subprocess on a non-repo cost a millisecond on the warm path until a
`.git` existence check guarded it.

## 2026-09-08, evening: 0.7.0

Six native plugin adapters landed this cycle (Yii2, Craft, private-
composer-installer, codeception/c3, Drupal scaffold, Symfony runtime),
alongside automatic best-effort adopt of a Composer-written `vendor/` and a
root package version guessed from git the way Composer's own
`VersionGuesser` does. The compat sweep now runs plugins on both sides of
every native-adapter project instead of `--no-plugins` on both, so an
adapter finally gets diffed against the real plugin it replaces. The
release checklist gained a step: reinstall the local `viv` binary after
tagging, since a stale one refuses plugins the new release already
adapts.

The first sweep under those conditions failed twice before it passed:
pinned checkouts are detached, so the root version needs Composer's
nearest-branch search, and the root's `branch-alias` belongs in its
`aliases`. The third difference was Composer's own: yii2-composer writes
its extensions map as archives finish extracting, and two Composer runs
on one lock disagree, so the sweep compares that file and Craft's
order-insensitively. At the tag: 24 rows identical, none differ, 4 skipped
for upstream reasons, and 2 of 19 pinned projects would still refuse
without `--no-plugins`.

## 2026-09-08, night: README, naming and four spikes

**Reading the tool cold.** The README was rewritten for a Composer user
who has never heard of viv: highlights first, then how to try it and how
to stop, with the technical detail moved to footnotes. Two sections came
out of that review: reasons to use viv, and reasons not to, gathered from
limitations that had been scattered across the page. The version-by-
version feature list went; a "works with" checklist replaced it.

**Fewer commands, viv spellings.** Nine Composer-parity tickets were
triaged down to two: `viv new` (with `create-project` as the alias) and
`viv init`, neither a like-for-like port. `add` and `rm` are now the
documented spellings, `require` and `remove` the aliases. The rule
recorded for later: a command earns its place when a project we run needs
it in viv, not because Composer has it.

**Spikes.** pnpm's per-file store and rehash apparatus do not transfer to
whole-archive Composer dists; its clone-then-hardlink probe does, and is
now #19's template. bun streams extraction under the download and still
round-trips every manifest for a 304, so its binary cache buys parse time
only. riff does the same job as viv on the corpus, with cosmetic diffs, and
its two cold wins sit inside GitHub's hop variance; the one measurable
difference is viv opening each extracted file about 1.6 times. The
slimming audit found two candidate deletions and one false one, void
because a doc line still said `--minimal-changes` was unwired.

**Also.** A progress line during download and link, on stderr and only on
a terminal; `cache clean` names the entry it refuses on. The corpus-wide
speed table waits for a quiet machine.

## 2026-09-08, late: init, new, and a hunt

**Two ways in.** `viv init` writes a normalised, validated `composer.json`
from inferred defaults, no prompts; `viv new` creates the directory first,
empty or from a package skeleton, with the constraint after a colon as in
`viv add`. `create-project` stays as the alias. A Laravel skeleton's lock
and `vendor/` diff clean against Composer's.

**Less code.** The JsonManipulator port is gone: `add` and `rm` always
normalise, so its only customer, `--no-normalize`, is now the same
deprecated no-op it already was on `install`. The unreachable libsolv
"impossible packages" pass went too. The store opens each extracted file
once. Fuzz targets build again and CI builds them on every push.

**A hunt.** Eight popular skeletons and fifteen random packages went
through the sweep with plugins on where possible: 32 rows identical, one
real bug (a dist-less metapackage fails to install), one sweep bug, and
five skeletons whose plugins viv refuses. `compat/hunted.md` records what
was tried so the next hunt starts elsewhere.

## 2026-09-09, small hours: the rest of 0.8

**One seam for adapters.** Every native adapter implements one trait with
a method per install phase, and `install.rs` calls the registry without
naming any adapter. Adding one is a new module and a registry line. The
adapter tests run as their own CI job, so a port that drifts from
upstream goes red on the adapters, not on the install path. The drift
check itself is in flight.

**Reflinks.** `--link-mode clone` copies-on-write from the store on
btrfs, XFS and APFS, so a project that patches `vendor/` gets writable
files at hardlink cost. Anywhere else it falls back to hardlinks with
one warning. ext4 here, so the fallback is what local tests exercise.

**add and rm read the project's repositories.** They used to resolve
against Packagist alone and ignore `--offline`; both the partial update
and the bare-name constraint lookup now share update's repository
construction. That surfaced the next gap: a `file://` composer
repository is rejected outright, which is what the fixtures use and
what turned CI red. Fix in flight.

**Committing in a shared tree.** Two agents at once means `git commit
-a` grabs the other one's half-done edits. Commit by path.

**Looked at, not used: swoole/typephp.** An AOT PHP-to-C++ compiler,
active and well built, GPL-3.0. Its output embeds the Zend engine and
its PHP subset drops the dynamic features Composer's plugin machinery
uses, so it neither speeds up viv, which runs no PHP on the hot path,
nor lets us compile plugins in place of hand-ported adapters.

## 2026-09-09, evening: 0.8.0

**The network came out of the bench.** A recorded local mirror serves all
three tools, so cold and update numbers are ours to gate. The first
mirrored corpus run changed the story twice: on a real-sized lock viv's
warm update is half Composer's speed with no network to blame, and half
of that time is parsing cached JSON. The README speed table is now ten
projects and says where riff and Composer are ahead.

**Field bugs the mirror found.** A redirecting repository broke `update`
outright. Composer blocks advisory-covered versions before it solves,
and viv did not, so locks could differ; ported, with `--no-blocking`.
A branch alias that alone satisfies a root constraint vanished from the
second solve. Each surfaced only because the mirror made the update
scenario run for Composer and fail for viv alone.

**Two regressions in one day.** The redirect fix treated 304 as a
redirect; the advisory filter left alias indices stale. Both caught
within the hour, both now tested. The lesson was already written down
earlier today: a hunt that dispatches every finding at once pays for it
in focus and in regressions. And watch CI after every push.

## 2026-09-10: 0.9.0, the update gets fast

**Where the time was.** A phase profile said "JSON parse, 53 percent".
A finer one said the JSON parser was 13 percent of that and the rest was
Packagist's minified format being expanded by deep-cloning the running
merge once per version, fourteen thousand times, for three thousand
versions the walk would accept. Expanding lazily, only on acceptance,
took Laravel's warm update from 607 to 260 ms; forgetting caches at
exit, caching platform detection and indexing the pool by name took it
to 230. Composer does the same update in 311 ms on the same mirror.

**What almost went wrong.** The first design filtered versions at parse
time. It would have passed every test and written a wrong lock the day
a second requirer widened a constraint. The agent that found that
stopped and said so. A fixture for that shape is parked.

**Priority.** Speed, then what the maintainer hits, then reasons to
choose viv over Composer. Compatibility is the floor, not the pitch.
Adapters wait until a project the maintainer runs needs one.

## 2026-09-13: 0.10.0, and four tickets that were wrong

**The pattern.** Nine issues went into 0.10 and four of them described
a symptom correctly and a cause incorrectly. The adapter-drift column
named the wrong row, not the wrong directory. The constraint-widening
test as specified would have passed whether or not the bug existed,
because the example narrowed rather than widened. The fuzz workflow's
exit 127 was a missing libstdc++ under devbox, with the corpus problem
it named sitting one layer further down, and two more failures behind
that. The PSR-4 ordering report had viv's output and Composer's the
wrong way round — editing to the ticket would have turned correct code
into a regression, frozen in a byte-for-byte fixture. Reproducing first
is not diligence, it is the only thing that distinguishes a fix from a
confident wrong one.

**Where the time went.** The metadata closure spends 80 ms of 143 in a
walk that cannot overlap anything, not in the JSON parse everyone
assumed. Forcing every file onto the blocking pool burned 40 percent
more CPU for identical wall-clock. The classmap scan was genuinely
serial and genuinely parallelisable, and still only bought 1.4x,
because one directory in the batch is large enough to set the floor
alone. Two speed tickets, one real win, one redirected.

**The sweep grew a second table.** Every release until now proved viv
installs what Composer installs, from a lock Composer wrote. It never
proved viv's resolver reaches that lock. It does now, on all ten pinned
projects — and the first run of it failed two of them because the
harness gave Composer `--no-scripts` and viv nothing, which looked
exactly like a resolver divergence until the flag lists were read side
by side.

**What it cost.** A bench gate that failed one run in three was a
baseline taken from a single run, sitting at the edge of its own
distribution rather than the middle. The machine suspended mid-sweep
because the inhibitor holding sleep off was recreated per turn, so it
covered the seconds of thinking and lapsed through the half hour of
work. Both were measurement problems wearing the costume of a bug.

## 2026-09-14: milestone 0.11 closed, and a release that is mostly a flag

**What 0.11 actually contained.** Almost nothing, by the time it was
tagged. The milestone closed seventeen issues, but the tag for 0.10.0
was cut in the middle of it, so the parallel classmap walk, the cached
advisories feed and the re-centred bench baseline all shipped in 0.10.0
and were closed into 0.11 afterwards. Of the fourteen commits after the
tag, exactly one changes anything a user can observe: matching a version
against a compiled constraint rather than walking a boxed tree, which
took constraint matching from about 27 ms across 33,000 calls to 4 or 5
ms. A milestone is a bucket for attention, not a manifest of a release,
and reading it as the second produces release notes that claim work the
release does not contain.

**The headline is a two-word deletion.** `release.yml` passed
`--prerelease` on every `gh release create`, so ten releases existed and
`releases/latest` returned 404 to all of them. Every consumer that wanted
a binary — a CI step, a Dockerfile build stage — had to pin a version by
hand or reimplement "latest". 0.11.0 is the first release without the
flag, which matters more to anyone adopting viv than the speedup does.

**Three investigations, two of which found nothing.** `platform_check`
was reported to compile ten regexes even when a lock has no platform
requirement; it does not, the branches already guard it, and the early
return added to exploit the premise measured 6.7 ms against 6.6 ms. The
second diagnosis, that the shared `semver.rs` parser has no regexes and
should absorb the duplicate, was also wrong: it is a facade over
`semver-php`, which carries its own ten. `chain.expand`'s 38 ms split
into 9.5 us of map clone and 2.6 us of conversion per call, with the
memoisation doing exactly what it claimed, and nothing worth cutting
against a closure that cannot reach its target from this slice alone.
Two issues closed having produced a test and a paragraph each.

**The counter was lying.** `expand_ms` shared its accumulator with
`DeltaChain::from_deltas`, a fixed per-name setup pass, inflating it by
about a quarter. Three issues reasoned from that number before anyone
measured the call directly. A stage counter that aggregates two stages
is worse than no counter, because it is trusted.

## 2026-09-14, evening: 0.12's plugin tickets, and two comments that were wrong

**A comment caused the bug.** `phpcs.rs`'s `run_phpcs` warned and returned
`Ok` when `php` was not on `PATH`, justified in a comment as "the same
tolerance `src/scripts.rs` gives a missing interpreter". The analogy does
not hold. A Composer script is user code with a documented `--no-scripts`
opt-out, so skipping it is the user's call; a plugin adapter's post-install
step is part of producing a correct `vendor/`, which is the thing viv
promises. The install reported success and the failure surfaced later, in
whatever next ran `phpcs`. Anyone with viv on the host and PHP inside devbox
hit it every time.

**The fix was to delete the dependency, not handle it.** The issue offered
three options — fail the install, record the skipped work for a later
install to repair, or warn louder — and all three keep a second code path
for a failure that does not need to exist. `--config-set` exists to write
one file, and viv already emulates `var_export` byte-exactly in
`phpstan.rs` for `craft` and `yii2`. phpcs was the only adapter shelling
out to `php` at all; every other plugin subprocess in `src/` is `git`. So
the precedent was already set and phpcs was the outlier, not the pioneer.

**The safety net was decorative.** `tests/fixtures/plugins/phpcs/expected/
CodeSniffer.conf` has been in the tree since the adapter landed, and no
test read it. The compat sweep does not cover this adapter either: it runs
`--no-plugins` by default and no project in `compat/corpus.toml` or `compat/hunted.md`
uses phpcs. A fixture nothing asserts against is a file, not a test. Filed
as #226 — one corpus project covers both that gap and the fact that
`CodeSniffer.conf`'s format is now a second upstream `adapter-drift.yml`
does not track.

**The ticket's guess was wrong and the source was one fetch away.** #131
speculated that `pestphp/pest-plugin` writes
`vendor/pestphp/pest-plugin/src/Plugins.php`. It writes
`vendor/pest-plugins.json`, and no `Plugins.php` exists anywhere in the
package. What decided adapter-versus-inert was `getSubscribedEvents`
returning `post-autoload-dump` — an event a plain install always fires —
and a handler that unconditionally writes into `vendor/`. A plugin that
only registered a command provider would have been inert.

**Closing five adapters nobody asked for.** #151 recorded five skeletons
whose plugins viv refuses: TYPO3, CakePHP, Contao, Silverstripe, Bolt.
Closed as unneeded. Writing them would add five upstream versions to track
in `adapter-drift.yml` for no user who is waiting on them, against a rule
that says an adapter gets written when a project we run needs it.

**The baseline, from fourteen runs.** Post-release step 3 for 0.11.0. Cold
install is where 0.11's work landed: monolog moved from 0.070 to 0.048 of
Composer's time, laravel from 0.155 to 0.115. The sub-10ms scenarios —
warm, noop, monolog's update-offline — rose by at most 10.3%, inside the
gate's 15% tolerance. At three to five milliseconds a run, that ratio moves
on scheduler noise rather than on work viv does, which is the reason the
baseline is a median of fourteen runs and not one.

**The sweep caught what the fixture could not.** #131 was closed on a green
`make check` and a fixture byte-diffed against real Composer, and it was
still wrong twice. Ordering: `getCanonicalPackages()` returns the local
repository's order, which is install order — `installed.json` is a
by-name-sorted view written from that repository, not the order itself — so
`roots/bedrock` got `pestphp/pest`'s nineteen entries ahead of the three
sibling packages Composer emits first. Presence: `pestphp/pest` is a dev
dependency, so `--no-dev` never installs the plugin and Composer writes no
`pest-plugins.json`; viv wrote an empty one.

Both were already solved in this repo. `super::in_install_order` exists for
the first and is what craft and yii2 use, with a doc comment recording the
same bug costing 54 lines of `vendor/yiisoft/extensions.php`. `phpcs.rs`
ports `MESSAGE_NOT_INSTALLED` for the second. A new adapter written from
upstream source alone walks past both.

The fixture could not have caught either: its three packages happen to
install in lock order, and it has no dev-only package. A fixture proves the
shape you thought to build into it. `roots/bedrock` was in the corpus the
whole time — the sweep just is not run per change, only before a release.
Twice in one evening a test existed and proved nothing: the phpcs golden
file nothing read, and this.

## 2026-09-14, late: 0.13 folded into 0.12, and a shim that wasn't one

**The migration path did not survive being used.** 0.13's four tickets moved
into 0.12, and setting up the smallest of them turned up two bugs in the
`composer` shim that no test covered. The shim dropped `--no-plugins`, so a
project with an unadapted plugin refused, told the user to pass
`--no-plugins`, and refused again when they did — the one flag a user reaches
for *because viv told them to*. And `translate` classified each argument on
its own, so `-d /app` classified `-d` as `Keep`, classified `/app` as
`Unknown`, and handed the whole command to the real Composer. Only the `=`
spelling ever ran viv.

**The second one fails in the worst direction.** On a machine that still has
Composer installed, the fallback succeeds and the output looks normal. A
migrated CI job or Dockerfile keeps paying for Composer while everyone
believes the migration landed. `-d /app` is what people write in a
Dockerfile, not `-d=/app`. A drop-in replacement whose failure mode is
"silently not replacing anything" is worse than one that errors.

**A flag that was already plumbed.** `--ignore-platform-reqs` needed no new
mechanism: `IgnorePlatform::All`/`List` existed and `platform_check()`
honoured them, but the one construction site hard-coded `None` and no CLI
flag could reach the rest. Dead plumbing waiting for a switch. Its
second-order effect fixed itself too — `write_autoload` sets the generator's
`platform_check` bool from the same call, so `autoload_real.php`'s `require`
line was already conditional on the same decision. One call site, both files.

**Static is not self-contained.** #213 wanted `FROM scratch` on the grounds
that the binary is static musl. viv links `rustls-platform-verifier` with
`rustls-native-certs` and no `webpki-roots`, so it reads the host CA store:
`SSL_CERT_DIR=/nonexistent viv update` gives `No CA certificates were loaded
from the system`. Static linking removes the libc dependency and says nothing
about trust anchors. Settled on `gcr.io/distroless/static`. Linking
`webpki-roots` instead would have made `FROM scratch` honest and broken
anyone running a private Satis over an internal CA, which is this tool's
own audience.

**The refusal now answers the question it provoked.** "Pass --no-plugins, as
Composer would" said the install matches Composer run with the same flag, not
what the plugin would have done. It now says whether taking the remedy costs
anything, on the same line as the remedy so the two are read together, and
defaults to "no adapter yet" for any plugin with no record — falsely claiming
safety is the asymmetric failure.

## 2026-09-14, night: 0.12.0

**Nine issues, and two of them found by using the thing.** #227 and #228
were not on the milestone when the evening started. They turned up while
building a scratch project to reproduce #211's premise, which is the only
reason anyone looked at the shim from outside. #228 is the one that
matters: the shim understood `--working-dir=/app` but not `-d /app`, and an
unrecognised argument makes it exec the real Composer silently. On a machine
that still has Composer installed, that succeeds and looks normal. A
migrated CI job keeps paying for Composer while everyone believes otherwise.

**The sweep was clean and the docs were not.** 44 rows identical, 0 differ,
6 skipped, and all 10 pinned projects resolve the same lock. Reading the
README against it turned up three stale claims, one of which predates this
release: it said viv has 20 pinned compatibility projects. It has 10, in 20
rows. The footnote had been counting rows and calling them projects for
several releases. `roots/bedrock` also stopped needing `--no-plugins` this
release, so "two of the pinned projects" became one — and the remaining one,
symfony/flex, is refused by design rather than not yet ported, which the old
wording ran together.

**v0.10.0 and v0.11.0 never committed a sweep result.** `compat/results/`
jumps from v0.9.0 to v0.12.0. Step 3 of the release checklist says to commit
one, and two releases skipped it without anything noticing. The README's
own footnote linked to `compat/results/v0.11.0.md`, a file that has never
existed in the tree.

**The speed table says 0.11.0 on purpose.** The corpus bench runs
`--no-plugins --no-scripts`, so an adapter, a flag, a shim fix and an image
are all off the path it measures. Restamping the version onto numbers that
were not re-taken would be the kind of small lie that is impossible to catch
later. The table says which version produced it and why that still holds.

## 2026-09-15: 0.13.0

**Twenty-two issues in a day, and the theme held.** The milestone had six:
every place viv reported success for work it did not do. `rm` on a package
that was never required printed "Writing lock file" and exited 0. `run` with
no script name listed scripts instead of erroring, and on a project with no
scripts printed nothing at all. `cache clean` refused viv's own metadata
bucket because the known-bucket list did not know it, and `cache prune`
deleted that bucket every run for the same reason. `validate` passed a
manifest Composer rejects. The shim fell back to real Composer without a
word. Sixteen more shipped from the milestones behind it because the lanes
were already open: the apcu prefix, `bump-after-update`, the problem-message
branches, `--ignore-platform-reqs` on `update`/`require`/`remove`, the bench
harness, Dependabot, the action pins.

**Four of the tickets were wrong in a way only fixing them showed.** #240
said `cache clean` exited 0 on refusal; it exited 1, and a snapshot already
asserted it — the bucket half of the ticket was right, the exit-code half
was written from the theme rather than from `$?`. #239 said `validate` and
`install` shared one `STALE_LOCK_WARNING` constant; `validate` had an inline
literal with different wording, so the trap the ticket warned about did not
exist, though the fix — a named constant with its stream and constraint in a
comment — was still the right one. #220 proposed three `skips.txt` entries;
the exit-code switch tripped on *any* footnote, known skips included, so the
run was already failing on seven existing entries and three more would have
changed nothing. #221 called widening `record_p2`'s early return "the smaller
change"; it would have added a network round trip to every run for every
package without a dev branch, against the mirror's whole point. Every one
was caught by a brief that said *reproduce the claim first* and *verify both
directions*. The tickets were filed by the same author the same day. A strong
theme makes the next instance easier to see and easier to see wrongly.

**The licence was wrong from the first drupal release.** `Cargo.toml` said
MIT. `src/plugins/drupal_scaffold.rs` says in its own module doc that it is
ported, class by class, from `drupal/core-composer-scaffold`, which is
GPL-2.0-or-later; three of its error strings are near-verbatim translations
of upstream text. Both WordPress core installers are GPL too. `cargo deny`
checks dependency licences, not the provenance of hand-written ports, so
every gate was green. It was found by reading a competitor's `NOTICE.md`:
they had made the same port, removed it in their 0.6.0 for exactly this
reason, yanked three releases, and written down why. viv is GPL-3.0-or-later
from this release; the ports' `or later` is what permits it. `NOTICE.md`
records every adapter's upstream and licence, and `adapter-drift.yml` now
fails on a mismatch or a missing row, so the next port cannot ship without
the question being asked. `cargo deny` then rejected `GPL-3.0` as an
unmatched allowance: it compares the exact SPDX expression.

**The competitor is real, and the name is coming back.** `Adelagric/vivacity`
started four days after this repo, under this repo's name, with the same
byte-identical promise and no shared code. On finding the clash they renamed
and, when asked, deleted the `vivace` crates rather than yank them, since a
yank keeps the name. It is publishable from about 23:36 UTC today. Their
harnesses run against any binary taking Composer's arguments; two
implementations checked against one Composer is the best outcome here.
They are also the fourth tool in `bench/corpus.sh` now: viv is 2.0× faster
cold and warm, 6.8× on a no-op, 1.5× on a warm update, on the six projects
vivacity installs. It refuses the other four — WordPress, Drupal, Yii,
Craft — because their locks name a plugin outside its list, and it does so
even under `--no-plugins`. That last part is a bug in their help text, parked
until the name is settled.

**The speed table was re-measured this time.** 0.12.0 shipped 0.11.0's
numbers on purpose and said so. This release's table is from a run taken
today with all four tools from the local mirror, and it states the section
of `corpus.md` it came from. Footnote 5 ("the range crosses 1×") retired: riff's
worst cold ratio is now 1.2×, not 0.9×.

**Sweep.** 40 rows: 34 identical, two of them under `--no-plugins` for
symfony/flex, 0 differ, 6 skipped — three packages Composer itself cannot
resolve — and all 10 pinned projects resolve the same lock. Then the README,
read against it as the checklist says: three bullets in "Reasons not to use
viv" were false. One said 20 pinned projects with two needing `--no-plugins`
— the footnote was fixed in 0.12.0, the bullet beside it was not. One said
riff leads cold installs on two of ten; today's run has viv ahead on all
nine where riff ran, so the bullet is gone rather than reworded. One listed
two unported error messages, both of which this release ports. Third release
running where the sweep was clean and the prose was not; the checklist's
"read it after, not before" earns its place each time.

**Memory, briefly.** Four concurrent `make check` runs across worktrees drove
the machine to 25 GB and 5 GB of swap the day before. `test-threads = 8` in
`nextest.toml` was the first commit of the day, before any lane opened, and
was then measured under four lanes: anonymous memory 1.5–3.4 GB, swap growth
875 MB over half an hour. The `memory.current` figure of 23.8 GB was page
cache. `memory.peak` cannot be reset without root, so on a machine that has
already blown past a ceiling once it reports that incident forever — sample
`memory.current` on a timer and split `memory.stat` instead.

## 2026-09-15, afternoon: the first screen (#247)

Twenty minutes on 0.14. The README opened with "proof of concept ... not yet
at 1.0" thirteen releases in, then a good install block that the sentence
before it undid. The top now leads with the description's own pitch, one
sourced number from today's corpus run (cold laravel/laravel 0.30 s against
Composer's 1.58 s), the compat sweep, and the install block in the first
twenty lines. The "not covered" sentence stays, as a sentence pointing at
`docs/stability.md`, not a disclaimer.

One thing found while writing it: the draft told binstall readers to run
`make install-shim`, which needs a checkout they do not have. The release
tarball already ships `composer` beside `viv` and binstall's `bin-dir`
template installs every binary in it, so the shim arrives with the first
command; the README now says so instead.

## 2026-09-15, afternoon: CI back on the table (#195)

The README commit went red twice on tests it could not have touched, which
turned the observation window back into work. Three findings, three commits.

**What mints the cache.** Not rust-cache: `compat.yml` runs on tags and
its devbox action saved the same 648 MB nix store under four tag refs,
2.6 GB of one file, because GitHub scopes caches per ref and the action has
no save-if. And every Cargo.lock or devbox.lock bump leaves the previous
generation of every job's entry for seven days. Both cache actions in
`compat.yml` now skip saving on a tag, and `cache-prune.yml` runs daily,
keeping the newest entry per key family on main. First run: 34 entries and
10.76 GB to 14 and 3.8 GB.

**Duration.** `check-macos` sat behind `needs: check`, so the critical path
was 2.2 plus 4.9 min. Both it and `bench` now run alongside. 7.8 min median
to 4.9 min on the first green run, against a 4 min target; the rest is
devbox install on macOS, 159 s of a 295 s job.

**Red.** Three runs in a row failed with 504 from `api.github.com` zipball
downloads, Linux runners only, while the macOS job passed the same tests
and the URLs answered 200 from here. viv and the Composer the tests shell
out to were both fetching unauthenticated. `COMPOSER_AUTH` with the
workflow token, set once at workflow level, is read by both; viv already
maps a `github.com` token onto `api.github.com`. `NEXTEST_RETRIES: 2`
covers a single blip. The next run was green.

## 2026-09-18: the tap, six framework pages, a spike, and a course correction

**Packaging.** The Homebrew tap exists and the release workflow has the
token to push to it; the formula for 0.13.0 checks out against every
release tarball. Untested on a Mac still.

**Site.** One generated page per framework (Laravel, Symfony, Drupal,
Bedrock, Craft, Statamic), every number read at build time from the corpus,
the compat sweep and the plugin inventory; a plugin the inventory does not
name fails the build.

**The first CPU profile.** `perf_event_paranoid` dropped to 1 for the first
time, so the flamegraphs sections 5 and 6.3 of `profile.md` said could not
be taken now exist. Laravel only: the corpus script deletes its checkouts.
The metadata closure on an offline update was 110 ms of 211; the flamegraph
showed malloc, indexmap pushes and string clones, which is a JSON tree per
version delta, 13,996 of them for 3,114 accepted versions and about 100 in
the lock. Two steps: keep deltas as raw slices and decode a three-key header
for the acceptance screen (215 to 170 ms), then defer the verbatim object
until a solve winner reads it (170 to 131 ms). #210 had measured the
snapshot clone and called it irreducible; the difference was reading the
consumer list, which showed only two pre-solve readers. An untagged serde
enum for the list-or-object entry cost back half of step one and was
replaced by a first-byte sniff. Lock byte-identical throughout.

**Course correction.** The question this project started with, whether a
person directing coding agents can build a faster byte-compatible Composer,
is answered. Compat mode is frozen as a control; the open fidelity items
moved to an on-demand milestone. New work follows `docs/research.md`: one
chapter at a time, behind a flag, measured against compat mode, written
up. Chapter 1 is a lock format that does not conflict in git, measured by
replaying historical merges. Chapter 2, autoload from the store without a
vendor tree, is sketched.

## 2026-09-18, afternoon: 0.14.0 and chapter 3

**Chapter 3 scoped.** Workspaces: one lock and one solve for a repository
with several `composer.json` roots, the thing npm, pnpm, Cargo and uv have
and Composer does not. Control is compat mode run once per member. Four
issues (#276 to #279); only the shared lock waits on chapter 1's record
format.

**0.14.0.** The last release before the research chapters change what
`viv` writes. Sweep: 20 of 20 pinned rows identical, 10 of 10 pinned locks
identical, 0 differ. The random sample drew six projects Composer itself
cannot install (dev-master under a stable `minimum-stability`, a private
dependency, a guzzle 6 blocked by advisories), so 12 skipped rows against
last release's 6; the sweep records why per row.

**A gate that never gated.** `make bench-check` passed only `viv.json` to
`compare.py`, so the ratio had no Composer denominator and every scenario
reported `skip`, which the target treats as success. CI passes all five
files and has been the real gate since #159. Fixed the target. Run
properly on this machine it fails warm: 9 ms against the 3.6 ms the
baseline ratio allows here, because Composer's warm install on this box
is 0.72 s and the ratio was set on runners where it is 2.5 s. A ratio
cancels machine speed only while both numbers are far from the floor; a
100-file hardlink pass is not. CI on the same commit passes warm at the
baseline ratio exactly, so the release cites CI. A virus scan was running
during the local run as well; noise, but not the cause.
