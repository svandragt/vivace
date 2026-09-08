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
