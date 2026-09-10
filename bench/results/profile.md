# Profile: install (#54) and update (#55)

Machine: AMD Ryzen 9 7900X3D, ext4, Linux 7.0, PHP 8.4.24, 2026-09-07 (same
machine as `bench/results/README.md`). `bench/laravel` (101 packages) and
`tests/fixtures/monolog` (3 packages), release build
(`devbox run -- cargo build --release`), isolated `XDG_CACHE_HOME`/`COMPOSER_HOME`
under a scratch dir on the same filesystem as `vendor/`, machine otherwise idle
(`uptime` load average 0.6–2.2 across the session, reported before each run
below).

**Flamegraphs: not produced.** `perf record`/`perf stat` need `CAP_PERFMON` or
`perf_event_paranoid <= 2`; this sandbox has `perf_event_paranoid = 4` and no
passwordless `sudo` to lower it or `setcap` the binary (`BLOCKER: environment`).
`valgrind` isn't installed either. Every question below that a flamegraph would
answer is instead answered from `-v`/`RUST_LOG=vivace=debug` timing spans (this
session added the ones that were missing) plus `strace -c -f` syscall counts,
which is what the tables below are built from. Re-run `cargo flamegraph` on a
machine with `perf` access to fill this gap; the spans stay either way (#54
asked for them alongside flamegraphs, not instead of them).

Code changes, all `-v`-gated `tracing::debug!` calls with no behaviour change:
`src/install.rs` (normalize check, lock read, plugin resolve, plan diff,
pre/post-install-cmd dispatch), `src/autoload/generator.rs` (classmap/PSR scan:
cache hits/misses, `scan_paths` time), `src/repository.rs` (metadata closure:
batches, requests, elapsed), `src/solver/solver.rs` (rule generation vs
propagate vs backjump/analyze time and call counts, pool/rule size),
`src/update.rs` (solve elapsed, lock-write elapsed).

## 1. Install (#54)

### 1.1 Wall-clock

| Scenario | bench/laravel (101 pkgs) | monolog (3 pkgs) |
|---|---|---|
| cold | 2.06–2.65 s | — (see `bench/results/README.md`) |
| warm | 43.7–43.9 ms | 6.7 ms |
| warm -o | 136.1 ms | 8.1 ms |
| no-op | 7.0–8.1 ms | 1.5 ms |

(`hyperfine --warmup 0`, 5 runs laravel / 8 runs monolog; ranges are two
separate sessions this profiling ran, not a single run's σ — see raw JSON
listed at the end of each subsection.)

### 1.2 Where warm time goes (`-v`, laravel, 101 packages)

```
checked composer.json normalization        0 ms
read and parsed composer.lock              2 ms   (101 packages)
resolved plugins                           0 ms
diffed lock against installed.json         0 ms   (plan: install=101)
dispatched pre-install-cmd                 0 ms
linked packages into vendor                20 ms  (101 packages, hardlink)
generated vendor/bin                       0 ms
  scanned classmap/PSR directories         20 ms  (25 cache hits, 7 misses, 2ms scan_paths)
generated autoload files                   26 ms  (includes the line above)
wrote installed.json/php and state         2 ms
dispatched post-install-cmd                0 ms
                                    total ≈ 50 ms  (matches "Installed ... in 0.05s")
```

Answering #54's first question directly: of the ~44–50 ms internal wall time,
**link_tree is ~40–45%** (20 ms, one `link()`/package·file) and
**autoload generation is ~50–55%** (26 ms, of which the classmap/PSR scan is
20 ms of that 26). Process start/shutdown (fork+exec, dynamic linking,
`tracing_subscriber` init) accounts for the remaining few ms between hyperfine's
43.7 ms wall mean and the ~50 ms the in-process "Installed ... in Xs" line
already reports — i.e. it's small, not a second `link_tree`-sized cost.
Normalize-by-default and the lock-freshness check are visible in the log but
both round to 0 ms: neither is worth chasing.

### 1.3 `-o` (optimize-autoloader): why it's 3× warm

```
linked packages into vendor                20-21 ms
generated vendor/bin                       0-1 ms
  scanned classmap/PSR directories         99-113 ms  (91 cache hits, 19 misses, 61 ms scan_paths)
generated autoload files                   113-116 ms
wrote installed.json/php and state         2 ms
```

The classmap-scan sidecar cache (`Cache scanned classmaps per store archive`,
landed this session) is doing its job for packages: **91 of 110 scanned
directories hit the cache** across three repeat runs, cache hit/miss counts
never drifting. But two things stand out:

- The **19 misses cost 61 ms of `scan_paths` every single run**, and stay 19
  across repeated runs — these are the root package's own PSR-4 directories
  (`app/`, `config/`, `database/`, `routes/`, `tests/`, ...), which
  `archive_classmap_sidecar` deliberately never caches (no store archive to key
  on, and they're expected to change between runs). That's correct, not a bug,
  but it's now the single largest cost in the `-o` path — 45–61% of the 136 ms
  wall time by itself.
- The 91 cache **hits** still cost ~38–52 ms combined (100–113 ms total scan
  time minus 61 ms of `scan_paths` for the misses) — reading 91 sidecar files,
  building 91 exclusion regexes and re-deriving each `ScanKey` isn't free even
  when the tokenizing step it's guarding is skipped.

Raw hyperfine JSON: not checked in (ad hoc, `--optimize-autoloader` isn't a
`bench/run.sh` scenario); reproduce with
`hyperfine --prepare "rm -rf vendor" "$VIV install" "$VIV install -o"` against a
warm cache.

### 1.3.1 #77 after: the 19 misses were a cache-thrashing bug, not root dirs

Finer `-v` spans added this session (`cache_read_ms`, `merge_ms`, `setup_ms`,
`cache_write_ms` alongside the existing `scan_paths_ms`) plus a one-off
`abs_dir`/`has_cache_entry` trace line traced §1.3's "19 misses" to a real
bug, not the root-package explanation above: **all 19 had a store archive to
cache on** (`has_cache_entry=true`) — `laravel/framework`'s several PSR-4
namespaces (`Illuminate\Support`, `\Macroable`, `\Conditionable`, ...),
`nette/schema` listing `src` under both `classmap` and PSR-4, and
`symfony/polyfill-*`'s base dir plus its `Resources/stubs` subdir all scan the
*same archive* under *different* `ScanKey`s. The sidecar
(`archive_classmap_sidecar`) stored only the single most-recently-written key
per archive, so scanning a second subpath of the same archive evicted and
overwrote the first — every one of those keys missed and got rewritten on
*every single warm run*, not just once.

Fix: `Sidecar` (`src/autoload/classmap.rs`) now holds a `Vec` of
`(ScanKey, classes, ambiguous)` entries per archive instead of one, so two
subpaths of the same archive both stay cached; `Scanner` (`generator.rs`)
reads each sidecar file at most once per install (a `HashMap<PathBuf,
Sidecar>`, not a file read per `ScanKey`) and merges a miss into the existing
entries before rewriting, rather than overwriting them.

```
                                          before        after
scanned classmap/PSR directories         73-78 ms      14-15 ms   (110 cache hits, 0 misses)
  scan_paths_ms (misses)                  42-52 ms       0 ms
  cache_read_ms (sidecar read+parse)      3 ms           3-4 ms
  merge_ms (fold into self.map)           9-10 ms        9 ms
  setup_ms (regex/ArchiveIndex/key)       1 ms           1 ms
  cache_write_ms (sidecar rewrite)        14-17 ms       0 ms  (nothing left to rewrite once warm)
generated autoload files                  88-116 ms      29 ms
```

(`RUST_LOG=vivace=debug viv install -o -v`, isolated `XDG_CACHE_HOME`, 5+ runs
each, `bench/laravel`; machine had other agents' `cargo`/`nextest` running
concurrently this session — load average 3.7-5.5 — so absolute numbers carry
more noise than usual, but the before/after gap and the 0-miss/0-write result
were stable across every repeat.)

Wall clock (`hyperfine --warmup 2 --runs 15`, `viv install -o` warm, same
scratch copy, before binary built from this file's pre-#77 content, after
binary the one this session lands):

```
before (thrash)   119.8 ms ± 3.0 ms
after (fixed)      61.3 ms ± 2.2 ms   (1.95× faster)
```

`autoload_classmap.php` and `autoload_static.php`: byte-identical before and
after (`cmp`), confirming the merge-not-overwrite fix changes nothing about
*what* gets cached, only that both keys of a shared archive now survive.
`hyperfine`-measured cache hits still cost more than the ideal in-process
number above 10 ms in a couple of runs, wholly inside `merge_ms` (~9 ms
folding ~5,885 classmap entries into `self.map`) — that cost is identical on
a miss too (it is the classmap-building bookkeeping the cache is meant to
skip *ahead of*, not part of the sidecar itself), so it wasn't chased further
here: the entire "40-50 ms" #77 named was the cache-thrashing bug above, not
the hit path, and that's gone.

Plain `warm`/`no-op` (`--optimize-autoloader` off, no PSR/classmap scan of
vendor archives at all) are unaffected by this change: `bench/run.sh
bench/laravel viv` still reports 41.2 ms / 7.1 ms, at or below the recorded
43.7-43.9 ms / 7.0-8.1 ms baseline.

### 1.4 serde_json `preserve_order` and installed.json

Question: does `preserve_order`'s `IndexMap` backing cost measurable time on a
100-package lock? No — "read and parsed composer.lock" for the full 101-package,
283 KB lock is 2 ms end to end (parse + validation), and `plan::plan`'s read of
the existing `installed.json` is under 1 ms (rounds to 0 in the log). Not worth
a feature flag.

### 1.5 strace -c -f (warm, laravel, 101 packages)

```
% time   syscall        calls   errors   note
42.99%   linkat         7746     -       hardlinking every vendor file (#54's own hypothesis, confirmed)
20.60%   futex           2       -       thread pool wakeup, negligible call count
10.02%   mkdir          1309    176      #2 below: many of these are avoidable
 8.18%   getdents64     2202     -       directory walks (classmap scan + link_tree)
 4.18%   chmod          1103     -       one per linked file (vendor perms)
 4.11%   openat         1209     31
 3.77%   close          1178     -
 3.58%   fstat          1105     -
 1.20%   statx           582    104      plan diff + cache lookups
                            total: 0.919 s across 17,961 syscalls
```

`strace -c -f` (noop, same project): **276 syscalls, 1.3 ms total**, dominated
by `statx` (117 calls, 38%) checking `installed.json`/lock state — no network,
no PHP spawned, confirming the no-op fast path really is filesystem-state-only.

### 1.6 Cold path

Not re-profiled in depth here: `bench/results/README.md`'s "Cold install:
closing the gap to Riff" section already answers where cold waits (extraction
semaphore, concurrency, not hashing) with its own timing evidence from an
earlier session. This session's cold numbers (2.06–2.65 s) are consistent with
that investigation's conclusions.

## 2. Update (#55)

Full and partial updates now exist (`src/update.rs`, `src/solver/`), so this
section is no longer blocked on #41/#42.

### 2.1 Wall-clock: `viv update` vs `composer update --no-install`

| Fixture | composer | viv | ratio |
|---|---|---|---|
| monolog (3 pkgs, warm metadata cache, 3 runs) | 418.7 ms ± 1.6 | 205.1 ms ± 3.8 | viv 2× faster |
| bench/laravel (101 pkgs, cold metadata cache, single run) | 1.204 s | 8.64 s | **viv 7.2× slower** |

riff has no `update` command reachable from this sandbox (not on `PATH`), so
it's not in the table; `bench/run.sh`'s new `update-warm` scenario still adds
it when `riff` is present (see `bench/run.sh`).

Laravel is where it hurts: viv wins on a 3-package fixture where nothing else
matters, and loses by 7× the moment the metadata closure and solve are
non-trivial. Root cause below.

### 2.2 Metadata closure: request count and cold vs warm

| Fixture | cache | packages | batches | requests | elapsed |
|---|---|---|---|---|---|
| laravel | cold | 251 | 9 | 251 | 1.68 s |
| laravel | warm (all 304) | 251 | 9 | 251 | 2.35 s |
| monolog | cold | 3 | 1 | 3 | 36 ms |

Batch size (`LOAD_BATCH_SIZE = 50`) already matches Composer's
`PoolBuilder::LOAD_BATCH_SIZE`, so the batching itself isn't the gap. The
surprising result: **a warm metadata cache is not faster** — every provider
file still needs a full request/response round trip to get a 304, so
Packagist's per-request latency dominates either way (this matches #55's own
suspicion, now with a number: 2.35 s warm vs 1.68 s cold, within session
network noise of each other, not a a meaningful win). The 251 closure requests
themselves aren't the problem either (1.7–2.4 s of the 8.6 s total); the next
section is.

### 2.3 Solver: rule generation vs propagate vs backjump, and the dev split

`solved pool` debug lines, laravel full update (`viv update`, no package
filter):

| Solve | pool packages | rules | rule generation | propagate | backjump | sat total |
|---|---|---|---|---|---|---|
| 1st (merged) | 45,604 | 492,226 | **4.81 s** | 0 ms (72 calls) | 0 ms (0 calls) | 12 ms |
| 2nd (dev split) | 155 | 275 | 0 ms | 0 ms (1 call) | 0 ms (0 calls) | 0 ms |

Answers, in order:

- **Rule generation dominates the solve overwhelmingly** — 4.81 s vs a
  combined 12 ms for propagate+backjump (SAT itself), a ~400× gap. This one
  number, not the SAT algorithm, is the entire cost of `viv update` on
  Laravel: closure (1.7–2.4 s) + rule generation (4.8 s) + everything else
  (~50 ms) ≈ the observed 8.6–9.0 s.
- **The dev-split second solve does not double the cost.** It solves a pool
  built only from the first solve's own 155 accepted packages
  (`pool_builder::clone_package`), not the original 45,604-package pool, so its
  rule generation is instant (0 ms). The "second solve doubles it" worry in
  #55 doesn't hold once the pool is already small.
- **PoolOptimizer's number:** a merged solve for a 101-package lock builds a
  **45,604-package pool and 492,226 rules** — every version Packagist has ever
  published for every package in the transitive closure, not just the ones a
  constraint could still pick. Composer's `PoolOptimizer` (skipped in the
  port) exists precisely to prune this before rule generation; the 4.81 s
  rule-generation cost is the direct, measured bill for not having it. This is
  the single biggest lever in the whole profile: it's ~56% of the update's
  total wall time on a 101-package project, and would only get worse on a
  larger one (Symfony-sized projects report solves with hundreds of thousands
  of package versions in the pool).

Same breakdown, monolog full update (cold metadata cache): pool 158/95 rules
(1st solve), 57/59 rules (2nd), both effectively 0 ms — too small to show the
same effect, which is exactly why it needed the Laravel-sized fixture to
surface.

### 2.4 Lock writing / content-hash

`wrote composer.lock (content-hash + serialisation)`: **2 ms** on the
101-package laravel lock, 0 ms on monolog. Not a cost worth chasing.

### 2.5 Partial update (`viv update psr/log`)

- **monolog: 207 ms total** (1 metadata request, pool 68/65 rules first solve,
  57/59 second solve, both solves ~0 ms) — works as expected, dominated by one
  network round trip.
- **bench/laravel: failed** (`psr/log ... could not be found in any version`).
  This isn't a profiling artefact: `pool_builder::build_partial`'s allow-list
  expansion doesn't queue `psr/log`'s own metadata for a package that's
  otherwise only reachable via a *locked* (fixed, not re-walked) package's
  `require` — monolog's fixture happens to require `psr/log` from the root, so
  it's queued from `roots`, but laravel only pulls it in transitively through
  `laravel/framework`, which is fixed. Filed as a correctness bug, not a perf
  one (see issues below) — it blocked measuring laravel's partial-update
  number, so that cell is empty rather than guessed.

### 2.6 After #76: `PoolOptimizer` wired into `pool_builder::build`

Candidate #1 below, landed: `src/solver/pool_optimizer.rs` ports
`PoolOptimizer.php`'s two passes and runs between pool build and rule
generation (`pool_builder::build`/`build_partial`, exactly where
`PoolBuilder::buildPool` calls it). Same machine/session as above, warm
metadata cache, one `RUST_LOG=vivace=debug` run of `viv update` on
`bench/laravel` (single run, not a `hyperfine` mean — consistent with how
§1.3's ad hoc numbers are reported):

| Metric | Before (§2.3) | After |
|---|---|---|
| Pool packages (1st solve) | 45,604 | **5,169** (−89%) |
| Rules (1st solve) | 492,226 | **13,918** (−97%) |
| Rule generation | 4.81 s | **0.58 s** (−88%) |
| Metadata closure (warm) | 2.35 s (§2.2) | 1.55 s |
| **Total (`resolved metadata and solved`)** | ~8.7 s | **14.1 s** |

The pool/rule-count win is exactly the one predicted in §2.3 and §3's old
row 1, and rule generation itself got the expected 8× cut. **Total wall-clock
got worse anyway**: `PoolOptimizer::optimize` itself now costs ~9.5 s (68% of
the 14.1 s total), moving the bottleneck rather than removing it, so
`update` is still well over the 2 s target from #76 and is now slower than
before this change on this fixture.

Where the 9.5 s inside `optimize` goes (same run, function-level timing added
temporarily and removed again): `alias_groups` and
`optimize_impossible_packages_away` (no locked packages, so a documented
no-op) are both sub-millisecond; `add_disjuncts` is 40 ms. The rest —
**~9.5 s — is `optimize_by_identical_dependencies`**: grouping (6.2 s,
1,160,288 disjunct checks driving 3,287,911 `Constraint::matches` calls) plus
selecting the preferred package per group (3.4 s). A handful of names
dominate the disjunct count: `php` (146 distinct requiring constraint texts
in this closure), `phpunit/php-code-coverage` (103),
`symfony/http-foundation` (105), `sebastian/comparator` (59),
`symfony/http-kernel` (56), `symfony/console` (56) — each checked against
every historical version of that name still in the pool
(`docs/resolver-design.md`'s note on the same `||`-split trade-off already
flags this shape).

A standalone micro-benchmark of `Constraint::matches` alone (3,000,000 calls,
a two-branch `^6.4 || ^7.0`-shaped constraint against one version) measured
**~775 ns/call** — consistent with the 3.29M calls costing several seconds by
itself, with no per-call caching in `semver_php` (`SingleConstraint::new`
allocates and the match walks the whole parsed constraint tree every time).
That is almost certainly the real gap to Composer's `CompilingMatcher`
(compiled/cached comparator, same name), not this port's loop shape: the
nested-loop algorithm here is the same one `PoolOptimizer.php` runs, so a
real ecosystem-sized closure pays the same O(versions × distinct requiring
constraints) product in PHP too.

**Not implemented here, flagged instead (`AGENTS.md`'s "do not start a second
optimisation without saying so"):** cheapening `Constraint::matches`'s
per-call cost lives in `src/semver.rs`/the `semver_php` facade, outside this
task's owned files, and is a distinct piece of work (a leaner point-in-range
check, or a cache keyed on `(constraint text, version)` — plausible, not
attempted). `PoolOptimizer` is wired in as #76 asked; the acceptance test
(byte-identical locks) passes; the wall-clock target is not met, and this is
why.

### 2.7 After #76 follow-up: `CompiledConstraint` (`src/semver.rs`)

The candidate flagged in §2.6 (`Constraint::matches`'s per-call cost), now
owned by this lane too. `src/semver.rs` gained `CompiledConstraint`/
`VersionKey`/`parse_version_key`: a constraint's numeric bounds compiled once
via `Intervals::get` (already correct, already ported — this hoists it out of
the hot loop rather than re-deriving it), each bound parsed once into a
`Vec<Part>`; `matches` is then a handful of `Part` comparisons, no string
work, no allocation. Equivalence proven against the whole `satisfies`/
`satisfied_by` semver corpus (`tests/semver_corpus.rs`,
`compiled_constraint_agrees_with_matches_on_the_*_corpus`): every
(constraint, version) pair must agree with `Constraint::matches`, or the test
fails — this is the same corpus `semver.rs`'s existing tests already gate on,
not a new one. Wired into `pool_optimizer.rs`'s two hot loops (require and
conflict disjuncts), caching one `CompiledConstraint` per distinct constraint
string exactly as `ConstraintGroups` already deduplicated by string; also
hoisted the replace/conflict-parts computation (previously recomputed once
per require disjunct bucket for no reason — same result every time) out to
once per package/name.

Same run shape as §2.6 (warm metadata cache, `RUST_LOG=vivace=debug`, single
run):

| Metric | §2.6 (PoolOptimizer, uncompiled) | After (compiled) |
|---|---|---|
| `optimize_by_identical_dependencies`'s grouping loop | 6.2 s (3.29M `matches` calls) | **0.79 s** (1.23M calls, 0 fallbacks to the slow path) |
| Constraint compilation (`add_disjuncts`) | n/a | 54 ms |
| **`optimize_by_identical_dependencies`'s selection loop** | 3.4 s | **3.3 s (now the largest single cost)** |
| **Total (`resolved metadata and solved`)** | 14.1 s | **8.2–9.9 s** (`bench/run.sh`'s `update-warm`: 8.233 s ± 0.312 s, 5 runs) |

The grouping loop got the ~8× the isolated `matches` micro-benchmark
predicted (§2.6). Total wall-clock followed it down, roughly back to (very
slightly better than) the pre-#76 baseline (~8.7 s) — `PoolOptimizer` is no
longer a net regression, but it is not yet a net win either, and update-warm
is still nowhere near the 2 s target.

**Next hotspot, found by profiling rather than guessed at (the task's own
instruction: report it, don't start a third optimisation):**
`optimize_by_identical_dependencies`'s *second* loop — picking the preferred
package within each identical-dependency group via
`DefaultPolicy::select_preferred_packages` — is now the single largest cost
in the whole `update`, at 3.3 s. It never touches `Constraint::matches` at
all, so `CompiledConstraint` cannot help it: its own hot path is
`DefaultPolicy::version_compare` → `crate::semver::compare` →
`semver_php::greater_than`/`less_than`, each of which re-normalises both
version strings and builds a fresh `SingleConstraint` per call — the exact
same "re-parse on every comparison" shape `CompiledConstraint` just fixed for
constraint matching, just on the *sorting* side instead of the *matching*
side. Left as the next candidate rather than fixed here.

Raw hyperfine JSON from this run: not committed (this session's
`bench/results/viv.json`/`viv-update.json` were reverted with `git checkout`
after recording the numbers above, matching the recorded `update-warm`
baseline's own provenance note).

### 2.8 After #76 third follow-up: `VersionKey: Ord`

The hotspot §2.7 named, now fixed the same way `CompiledConstraint` fixed
matching: `VersionKey` (`src/semver.rs`) implements `Ord` directly on its
pre-parsed `Part`s (a plain part-by-part `version_compare`, folding to
`Equal` when both sides are dev branches, exactly `crate::semver::compare`'s
own documented gap). `DefaultPolicy::PackageRef` now carries a `VersionKey`
parsed once per candidate instead of a `NormalizedVersion` re-parsed on every
pairwise `crate::semver::compare` call inside `prune_to_best_version`.
Equivalence proven two ways (`tests/semver_corpus.rs`): every pair within
each `sort.json`/`rsort.json` row, and every pair within each recorded
Packagist fixture package's own version history (`p2/**/*.json`, ~2,000
versions across 41 files, stride-sampled to 80 per package so the two
several-hundred-version outliers — `phpunit/phpunit`, `symfony/yaml` — don't
dominate the test suite's own runtime).

Same run shape as §2.6/§2.7:

| Metric | §2.7 (compiled matching) | After (+ `VersionKey: Ord`) |
|---|---|---|
| `optimize_by_identical_dependencies`'s grouping loop | 0.79 s | 0.77 s (unchanged — this loop never sorted) |
| **`optimize_by_identical_dependencies`'s selection loop** | 3.3 s | **0.15 s** |
| **Total (`resolved metadata and solved`)** | 8.2 s ± 0.3 s | **5.1 s ± 0.3 s** (`update-warm`: 5.140 s ± 0.264 s, 5 runs) |

`bench/run.sh bench/laravel viv`: install warm 40.3 ms, no-op 6.8 ms —
unchanged. All `update`/`require`/`remove` byte-diff tests stay green
(523/523 in the shared tree).

**Still above the 2 s target. Next hotspot, by number, found by profiling
(not fixed here):** with both loops of `optimize_by_identical_dependencies`
now under a second combined (~0.9 s), the largest *remaining* piece inside
`pool_builder::build` is turning the closure into `Package`s in the first
place — `push_package_version`, called once per one of the 45,604 versions,
parsing every require/conflict/provide/replace link's constraint via
`semver::parse_constraint`: **863 ms**, measured directly
(`RUST_LOG=vivace=debug`-adjacent temporary timing, this session). Unlike
`pool_optimizer`'s `ConstraintGroups`, nothing caches this parse by string,
so the same constraint text (`"php": "^7.2.5 || ^8.0.0"`, written near-
identically by thousands of package versions) gets parsed thousands of
times over. The single largest piece overall is the metadata closure fetch
itself (**1.48 s** this run, 251 provider-file requests) — not an
algorithmic target, `bench/results/profile.md` §2.2 already found a warm
cache doesn't reliably beat a cold one there (Packagist's own round-trip
latency dominates either way).

## 3. Ranked optimisation candidates

| # | Candidate | Evidence | Expected saving |
|---|---|---|---|
| 1 | ~~Prune the solver pool before rule generation (PoolOptimizer or equivalent)~~ — **landed (#76), see §2.6**: rule generation dropped 4.81 s → 0.58 s as predicted, but `PoolOptimizer` itself now costs ~9.5 s on this fixture, a net regression | §2.3: 4.81 s of 8.6 s (56%) is rule generation over a 45,604-package, 492,226-rule pool for a 101-package lock | Multi-second per `update`/`require` on any project with deep transitive deps; likely the difference between viv losing 7× to Composer and beating it |
| 2 | Skip/cheapen the classmap-scan cache-hit path (91 hits still cost ~40-50ms) and reconsider mtime-checking the root package's own (never-cached) PSR-4 dirs | §1.3: 19 always-miss dirs cost 61 ms every `-o` run (45-61% of the 136 ms warm-o wall time); 91 hits still cost ~40-50ms | Tens of ms off `install -o`, the common case for a deployed build |
| 3 | `link_tree`'s `create_dir_all` issues far more `mkdir` than needed | §1.5: 1309 `mkdir` calls, 176 errors (EEXIST), 10% of warm's syscall time (92 ms) across 101 packages | Single-digit ms on warm install; small but a clean, isolated fix |

Issues filed (`gh issue create --milestone "0.5 reach" --label performance`):
see below. `viv update psr/log` failing on `bench/laravel` (§2.5) is a
correctness bug, not a perf candidate, so it's filed separately without the
`performance` label.

## 4. CI: update-warm regression gate

`bench/run.sh` now has an `update-warm` scenario (composer/viv always, riff
when on `PATH`); `bench/compare.py` and `.github/workflows/ci.yml`'s `bench`
job check it the same way as `warm`/`noop`. Baseline
(`bench/results/baseline.json`, monolog, three quiet local runs — see caveat
below): **204 ms** (± 5 ms across 3 runs). `warm`/`noop`/`cold` keep their
existing CI-runner-measured values unchanged.

Caveat: unlike its siblings, this `update-warm` baseline number was measured
on this sandbox, not a GitHub runner, because the task asked for one from
three quiet runs now rather than waiting for a green CI run's
`baseline-candidate` artifact. Replace it with a runner-measured number the
next time a maintainer downloads one, same as `bench/results/README.md`
already documents for `warm`/`noop`/`cold`.

## 5. What couldn't be measured, and why

- **Flamegraphs (both #54 and #55 ask for them):** blocked, see the top of
  this file — `perf_event_paranoid = 4`, no root, no `valgrind`. Docker was
  available but running it with the elevated capabilities (`CAP_SYS_ADMIN`/
  `CAP_PERFMON`) needed to bypass the host's `perf_event_paranoid` was refused
  by this session's own sandboxing, correctly — that would have been a sandbox
  escape, not a workaround.
- **Laravel partial update (`viv update psr/log`):** blocked on the correctness
  bug in §2.5, filed as an issue.
- **riff comparison** for both install and update-warm: `riff` isn't on
  `PATH` in this sandbox; `bench/run.sh` already skips it gracefully (install
  scenarios take an explicit tool list; `update-warm` checks `command -v`).

## 6. Offline update split (#159)

Same machine as above (AMD Ryzen 9 7900X3D, 24 vCPU, Linux 7.0), release
build (`devbox run -- cargo build --release`, commit `3c60df9` plus this
session's timing-only changes below), warm `XDG_CACHE_HOME` (one prior
`viv update` per fixture), `bench/laravel` (101 packages) and
`tests/fixtures/monolog`. Load average (`uptime`, 1-minute) stayed at
0.8–1.8 through every timed run except the 20-run advisory-delta hyperfine
below, which briefly touched 4.1 right at its start (a backup `rsync`); its
own σ (0.9–1.5 ms) shows no effect. `RUST_LOG=vivace=debug` for the phase
split, run via `devbox run --` so `platform.rs`'s `php` probe resolves the
right binary (outside devbox, `viv update` fails outright: `ext-filter` is
missing from a bare `php` on `PATH`).

Code changes, all `-v`-gated `tracing::debug!` calls with no behaviour
change, same idiom as the spans `bench/results/profile.md` §2's own table
already lists: `src/repository.rs` (two `AtomicUsize`s counting cached-file
reads and bytes, sampled before/after `load_closure_seeded` the same way its
existing `requests`/`request_count` delta already is), `src/solver/
pool_builder.rs` (advisory-filter step timed separately from the
closure-to-pool conversion it sits between; platform-package detection
timed separately, since it shells out to `php`), `src/solver/mod.rs` (the
dev-split second solve timed as a whole — pool clone, second solve,
partition — alongside `solver::solve`'s own existing per-solve rule
generation/SAT breakdown), `src/update.rs` (fetcher/repository construction
and the current lock's own read+parse, timed as the setup ahead of the
closure walk).

### 6.1 Wall-clock: offline vs with network (5 runs each, `hyperfine --warmup 1`)

| Fixture | `--offline` | with network (304s) |
|---|---|---|
| bench/laravel (101 pkgs) | **593 ± 7 ms** | 1041 ± 18 ms |
| monolog (3 pkgs) | **39.4 ± 0.6 ms** | 334 ± 50 ms (noisy — one of 5 runs came in at 425 ms, flagged by hyperfine itself; Packagist round-trip latency varies run to run, as §2.2 already found) |

Both figures land close to the issue's own two comments (1132/940 ms and
187/47 ms on a different session of this same machine), confirming nothing
drifted since. The network share is now the *minority* of laravel's wall
time (~43%) and the *majority* of monolog's (~88%) — exactly inverted from
each other, because monolog's whole offline cost is a handful of small
files while laravel's is a real pool build and solve.

### 6.2 Phase split, `--offline` (median of 5 runs, `RUST_LOG=vivace=debug`)

| Phase | laravel (101 pkgs) | % | monolog (3 pkgs) | % |
|---|---|---|---|---|
| Setup (fetcher/repo build, read current lock) | 5 ms | 0.9% | 3 ms | 9.1% |
| **Read + JSON-parse cached metadata** (closure walk) | **299 ms** (108 files, 7.93 MB) | **52.8%** | **5 ms** (3 files, 96 KB) | 15.2% |
| Post-closure drop/teardown (pool-size-dependent, see below) | 81 ms | 14.3% | ~0 ms | ~0% |
| Platform detection (shells out to `php`) | 26 ms | 4.6% | 24 ms | 72.7% |
| Pool building (closure → `Package`s) | 9 ms | 1.6% | 0 ms | 0% |
| Advisory filter (`--offline`: no advisories POST, abandoned-only scan) | 0 ms | 0% | 0 ms | 0% |
| Pool optimiser (`pool_optimizer::optimize`) | 21 ms | 3.7% | 0 ms | 0% |
| CDCL solve (merged + dev-split, SAT total) | 62 ms | 11.0% | 0 ms | 0% |
| Post-solve drop/teardown (pool-size-dependent) | 61 ms | 10.8% | 1 ms | 3.0% |
| Lock write | 2 ms | 0.4% | 0 ms | 0% |
| **Total** | **566 ms**¹ | 100% | **33 ms**¹ | 100% |

¹ Sum of the medians above; the hyperfine wall-clock in §6.1 (593/39.4 ms)
also includes process startup/exit (`fork`+`exec` of `viv` itself, dynamic
linking, shell/`sh -c` overhead from this session's own harness), which
these in-process spans can't see — the ~25 ms (laravel) / ~6 ms (monolog)
gap to the hyperfine mean is that outside-the-process overhead, not a
missing phase above.

**The two "drop/teardown" rows are not a phase anyone times explicitly.**
They're the gap between one `tracing::debug!` and the next when nothing
else in the source is scheduled to run: on laravel, `packages` peaks at
3,175 entries before the optimiser and settles at 647/186 after, each
carrying an `Arc<Value>` to that package's raw metadata plus several small
`Vec<Link>` fields — dropping that many heap-allocated structures takes real
time. The same gaps are ~0 ms on monolog, where the pool never gets past
~90 entries: the size dependence is the evidence, not a name in the
`tracing` output. Not investigated further (this task's own instruction:
name the phase, don't start a fix) — a candidate for `bench/results/
profile.md`'s existing ranked-candidates table (§3) if someone picks it up,
possibly the same `Arc`-sharing shape that already made `push_package_version`
(§2.8) and `PoolOptimizer` (§2.6) expensive, just paid on the way out instead
of the way in.

**Answering the issue's question:** on laravel, JSON parsing 7.93 MB across
108 cached provider files is **53% of the offline wall time** — the ceiling
on what a cache-format change (a binary/mmap-able format instead of
per-file JSON, an index instead of a full closure walk) could recover is
therefore *at most* about half of `update`'s offline cost, not the whole
thing. The other half (267 of 566 ms) splits unevenly: pool-size-dependent
drop/teardown is the single largest remaining piece (142 ms, 25% of the
total — both gaps combined, untouched by a parse-format change since it's
downstream of parsing), the solve pipeline itself is smaller (pool building
+ optimiser + CDCL + dev-split, 92 ms, 16%), and the rest (33 ms, 6%) is
fixed setup/platform-detection/lock-write that doesn't scale with project
size at all. On monolog, by contrast, there's essentially no pool to build
(parsing 96 KB and solving 90-odd platform+root packages is
sub-millisecond); its whole 33 ms offline floor is almost entirely the
fixed cost — mostly the `php` subprocess spawn for platform detection (24 of
33 ms, 73%), a cost every `update` pays regardless of project size and one
#159 didn't ask to be fixed here but is worth naming: a project small enough
that its own work is negligible pays almost entirely for probing its own
PHP environment.

### 6.3 `perf`/flamegraph: still blocked

Same blocker as §5: `perf_event_paranoid = 4`, no root, no `CAP_PERFMON`.
Re-checked for this task specifically — `devbox run -- perf record -o
/tmp/test.perf -- sleep 0.2` fails with the same `perf_event_paranoid`
error devbox's own bundled `perf` reports; `cargo flamegraph` is installed
(`flamegraph-flamegraph 0.6.14` via devbox) but shells out to the same
blocked `perf` on Linux. No top-15 symbol table for this section;
`BLOCKER: environment`, unchanged since §5.

### 6.4 Advisory-step delta (0d9993b)

`hyperfine --warmup 2 --runs 20`, monolog, `--offline`, this build (commit
`3c60df9` + this session's timing spans) vs a worktree built from `86378b8`
(the commit immediately before 0d9993b's advisory blocking landed):

| Build | Mean | σ |
|---|---|---|
| `86378b8` (pre-advisory-filter) | 38.6 ms | 0.9 ms |
| this build (post-advisory-filter) | 39.4 ms | 1.5 ms |

**+0.8 ms (~2%)**, not the +15 ms (47→62 ms) the CI gate's baseline update
saw. Both this session's own §6.2 (`filtered the pool for advisories`:
0 ms, median of 5) and this A/B agree the advisory step itself costs
nothing measurable under `--offline` (`filter_advisories`'s own early
`no_blocking`/`!block_insecure && !block_abandoned` check returns before
touching the network only when blocking is off; monolog's fixture has
blocking on, so it still walks the abandoned-check loop, just over 90-odd
packages instead of thousands). The CI gate's 47→62 ms figure most likely
reflects GitHub-runner-specific noise (a shared, often-throttled CPU) rather
than this step's real cost on quieter hardware — worth a second look on the
runner itself if the gate flags it again, not a regression to chase from
this session's numbers alone.

## 7. Splitting the 299 ms read+parse phase (#176)

Same machine/session as §6, `bench/laravel` copied to scratch with its own
`XDG_CACHE_HOME`, warmed with one `viv update --no-install` then measured
with `viv update --offline --no-install`, release build,
`RUST_LOG=vivace=debug`, median of 5 runs. New `-v`-gated `tracing::debug!`
accumulators in `src/repository.rs` (`STAGE_READ_NS`/`STAGE_JSON_PARSE_NS`/
`STAGE_EXPAND_NS`/`STAGE_CONVERT_NS`, same idiom as §6's
`CACHE_FILES_PARSED`/`CACHE_BYTES_PARSED`) split the phase at: (a) file read
(`fs_err::read` in `read_cache_file`), (b) `serde_json::from_slice` to
`Value` (`parse_json_blocking`), (c) `expand_minified`, (d) `PackageVersion::
from_owned_value` (version normalisation plus the `require`/`require-dev`/
`replace`/`provide`/`conflict` map clones). Left in place, matching this
file's existing spans.

| Stage | Median | Share of stage sum |
|---|---|---|
| (a) file read | 25 ms | 3% |
| (b) JSON parse to `Value` | 107 ms | 13% |
| (c) `expand_minified` | **519 ms** | **64%** |
| (d) convert to `PackageVersion` | 158 ms | 20% |
| Stage sum | 809 ms | — |
| **Wall-clock (`loaded metadata closure`)** | **375 ms** | — |

The stage sum (809 ms) is well over the wall-clock (375 ms): (c)/(d) run on
tokio's blocking pool and (a)/(b) inline on the multi-threaded async
executor, up to `LOAD_BATCH_SIZE` (100) fetches concurrently on this
machine's 24 vCPUs, so these are CPU-seconds spent, not serial wall-time —
real, just diluted by parallelism already present in the closure walk.

Counts: 108 cached files, 7,931,447 bytes (matches §6's 7.93 MB); 13,996
`PackageVersion`s produced by (c)+(d); only 3,175 survive
`pool_builder`'s constraint filter into the pool (its own "converted the
metadata closure into pool packages" line) — 77% of the parse/expand/convert
work on this fixture is discarded immediately after; 0 calls to
`version::normalize` (every cached entry already carries
`version_normalized`, so that regex path is free here).

**The issue's own suspicion is confirmed, and sharper than guessed:** raw
JSON parsing (b) is only 13% of the split, not the dominant cost — `serde_json`
itself runs at a normal ~264 MB/s in isolation (see experiment 1). The actual
weight is (c) `expand_minified`, 5x the cost of parsing: its `expanded.push
(next.clone())` (`src/repository.rs`, the minifier's expand loop) does a full
deep clone of the *entire* cumulative merged object — every field seen so
far, not just this version's diff — once per version, 13,996 times on this
fixture. That's the real reason 7.9 MB takes hundreds of ms: not the JSON
grammar, the O(n) full-object clone chain the minified format's inheritance
forces.

**Experiment 1: typed struct instead of `Value`.** Standalone microbench
(`serde_json::from_slice` over the same 108 cached files, single-threaded,
20 iterations) parsing to `serde_json::Value` vs a `#[derive(Deserialize)]`
`{ packages: HashMap<String, Vec<Value>>, minified: Option<String> }`: **30.15
ms/iter vs 30.38 ms/iter — no measurable difference.** Not applicable as
framed: every per-version entry has to stay a generic `Value` regardless of
the outer struct's shape, because `PackageVersion::raw` keeps it verbatim for
the lock/dumper later (`src/repository.rs`'s own doc comment on
`from_owned_value`); typing only the two outer keys (`packages`/`minified`)
covers too small a share of the parse to matter.

**Experiment 2: skip expansion for versions the constraints/lock can't
select.** Not applicable as a cheap pre-filter, and viv already matches
Composer here: minified diffs are a cumulative chain (`expand_minified`'s
`current` merges forward, entry N only decodable from entry N-1's already-
expanded form), and `is_version_loaded`'s constraint check needs
`version_normalized`, which only exists after that entry's own expansion —
there's no field to test before paying the clone. `ComposerRepository::
isVersionAcceptable` has the identical shape: Composer's `PoolBuilder` also
fetches and expands a needed name's whole provider file, then filters
version-by-version after, so this isn't a place viv fell behind Composer's
own loader. What profiling *did* surface (not guessed, per this task's own
instruction): 77% of the fully expanded-and-converted versions above are
discarded immediately by that same filter — real waste, just not one a
"skip before expand" rule can reach without changing `expand_minified`'s own
shape, which is a design change, not a quick experiment; flagged here, not
attempted.

**Recommendation for #176:** a cache keyed on the *raw parsed `Value`* (the
issue's literal proposal, `bincode`/`postcard` next to the cached JSON) only
removes stages (a)+(b) — 132 ms of the 809 ms stage sum, ~16%, and the
smaller half of the phase. It leaves (c) `expand_minified`'s clone chain and
(d)'s field extraction — 677 ms, 84% — completely unrecovered, because both
run *after* the point that cache would return. A cache keyed on the fully
expanded-and-converted `PackageVersion` form removes all four stages on a
warm hit (up to the full 809 ms/375 ms), but only on a warm hit — every cold
`update` (no prior cache, or a 200 replacing a changed file) still pays
`expand_minified`'s clone in full. The cheaper, cache-independent fix is
`expand_minified`'s `next.clone()` itself: it's the single biggest line item
here (64% of the split) and fixing it helps every run, cold or warm, not
just a cache hit — a real lever this task didn't attempt (design change,
flagged per §2.6/§2.7's precedent), and one worth ranking above the cache
format question #176 opened with.
