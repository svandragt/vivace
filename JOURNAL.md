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
