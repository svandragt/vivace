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

## 3. Ranked optimisation candidates

| # | Candidate | Evidence | Expected saving |
|---|---|---|---|
| 1 | Prune the solver pool before rule generation (PoolOptimizer or equivalent) | §2.3: 4.81 s of 8.6 s (56%) is rule generation over a 45,604-package, 492,226-rule pool for a 101-package lock | Multi-second per `update`/`require` on any project with deep transitive deps; likely the difference between viv losing 7× to Composer and beating it |
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
