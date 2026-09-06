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
