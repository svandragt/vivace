# Install from lock, 101 packages (bench/laravel)

Machine: AMD Ryzen 9 7900X3D, ext4, Linux 7.0, PHP 8.4.24, 2026-09-06.
`bench/run.sh bench/laravel composer riff viv`, three runs each, means.
Vendor directory and caches on the same filesystem (hardlinks need that;
across filesystems `viv` falls back to copying with a warning).

Scenarios: cold is no cache and no `vendor/`; warm is cache present, no
`vendor/`; no-op is `vendor/` present and up to date.

| Tool | Cold | Warm | No-op |
|---|---|---|---|
| composer 2.10.2 | 8.28 s | 1.69 s | 0.50 s |
| riff 0.0.7 | 1.75 s | 0.26 s | 0.23 s |
| viv 0.1.0 | 2.22 s | 0.146 s | 0.010 s |
| presto 0.1.12 (earlier run) | 6.46 s | 6.33 s | 3.41 s |

Output check for the same lock: every file in `vendor/composer/` and
`vendor/autoload.php` that `viv` writes is byte-identical to Composer's, and
the package trees match, `vendor/bin` included.

Notes

- Warm: `viv` hardlinks each file from an extracted store, Riff and Composer
  unzip every archive into `vendor/`. System time tells the story: 107 ms
  against 258 ms for Riff.
- No-op: `viv` compares the lock with `installed.json` and a small state
  file, spawns no PHP and touches no network. Riff regenerates the
  autoloader and spawns `php` for platform detection (157 ms with
  `--no-audit`); Composer boots PHP and does the same.
- Cold: downloads dominate. `viv` is half a second behind Riff here; the
  download layer (concurrency, streaming) is the next thing to tune.
- Presto has no download cache despite its README and rewrites
  `composer.lock`; its autoloader is not Composer-compatible. Numbers kept
  for the record from the study run.
- Filesystem dominates install numbers, as uv's benchmark notes warn.

Raw hyperfine JSON: `composer.json`, `riff.json`, `viv.json`, `presto.json`.

## Cold install: closing the gap to Riff (#1)

Debug timings (`RUST_LOG=vivace=debug`) on the cold path showed the fetch
phase completely dominating: hashing and extraction together summed to under
300 ms across all 101 packages, while summed download time was tens of
seconds, spread over only `CONCURRENCY` (16) requests in flight. `viv` was
concurrency-bound, not CPU-bound.

Two changes, `bench/run.sh bench/laravel riff viv`, three runs each, same
session (network conditions vary between sessions, so compare cold numbers
within this table, not against the one above):

| Change | viv cold |
|---|---|
| before (concurrency 16, extraction inline in the download loop) | 3.98 s ± 0.12 s |
| after (concurrency 64, extraction overlapped via a bounded `JoinSet`) | 2.29 s ± 0.08 s |
| riff 0.0.7 (same session) | 1.69 s ± 0.11 s |

What changed, in `src/install.rs`:

- `CONCURRENCY` 16 → 64: downloads are a GitHub zipball round-trip each, not
  CPU work, so more requests in flight shortens the cold path close to
  linearly until the host's own limits take over; 64 measured near that knee
  with low run-to-run variance (128 measured about the same but noisier).
- `fetch_missing` spawns each `Store::add_zip` extraction onto a bounded
  `JoinSet` (8 concurrent) instead of awaiting it inline: awaiting extraction
  inline stalls `stream.next()`, and `buffer_unordered`'s in-flight downloads
  only make progress while their stream is polled, so every extraction (a few
  ms each, ~300 ms total) was serialising onto the critical path for no
  reason.
- HTTP/2 is already negotiated to both `api.github.com` and
  `codeload.github.com` (`RUST_LOG=h2=trace` shows the client SETTINGS
  handshake), and sha256 hashing was already negligible, so neither needed
  touching.

Raw hyperfine JSON for this comparison: `viv-cold-before.json`,
`viv-cold-after.json`, `riff-cold.json`.

### Closing the rest of the gap (#1, follow-up)

`src/fetch.rs` now times every hop (`tracing::debug!` per redirect and per
body-complete GET, plus a per-host count/min/median/max summary via
`Fetcher::log_hop_summary`). Two hypotheses from the issue turned out not to
hold, and one real bug did.

**Hypothesis: Riff skips the `api.github.com` redirect hop.** False. Riff's
`riff-core/src/downloader/archive.rs` mirrors Composer's
`Util\Url::updateDistReference` byte for byte: it keeps rewriting to
`https://api.github.com/repos/<owner>/<repo>/zipball/<ref>`, the same URL
`composer.lock` already gives `viv`, and never rewrites to
`codeload.github.com` (`strings` on the riff binary has no `codeload`
string at all). `strace -f -e trace=connect` on both tools during a cold
install confirms it: each opens 64 fresh connections to `api.github.com`
(20.26.156.210) in the same sub-15ms burst, then a smaller, staggered number
to `codeload.github.com` as redirects resolve. There is no codeload rewrite
to adopt; both tools pay the same redirect hop, the same way.

**Hypothesis: connection setup (TLS handshakes, `pool_max_idle_per_host`).**
No difference found. Both `viv` and Riff build their `reqwest::Client` on
`rustls-tls`, neither sets a custom `pool_max_idle_per_host`, and HTTP/2 was
already confirmed negotiated to both hosts. Per-hop timings varied enormously
run to run within the same session (`api.github.com` hop median from 190 ms
to over 1.7 s, `codeload.github.com` body-complete median from 170 ms to
900 ms+) — session-to-session (and even run-to-run) network/GitHub-side
variance dwarfs anything attributable to connection setup; see the caveat
below.

**Real bug: `fetch_missing`'s extraction backlog stalled the whole download
stream.** The `tokio::select!` loop only polled `downloads.next()` when
`extractions.len() < EXTRACT_CONCURRENCY` (8). That guard doesn't just delay
*starting* new downloads — while false, `downloads.next()` isn't polled at
all, which means `buffer_unordered`'s up-to-64 already-in-flight downloads
make no progress either, since they only advance while their stream is
polled. Every time the extraction queue filled (extraction is a few ms each,
so this happens repeatedly across 101 packages), every in-flight download
paused. This is exactly the stall the bounded `JoinSet` above was meant to
avoid, just eight downloads wide instead of one.

Fix: `EXTRACT_CONCURRENCY` is now a `Semaphore` acquired *inside* each spawned
extraction task, not a guard on the download branch of `select!`;
`downloads.next()` is polled unconditionally until the stream ends, so
in-flight downloads are never paused by the extraction backlog.

A/B on this one change alone, alternating `viv` before/after in the same
`hyperfine` invocation (six runs each, so both sides see the same network
drift):

| Binary | viv cold |
|---|---|
| before (extraction backlog gates `downloads.next()`) | 2.52 s ± 0.17 s |
| after (extraction backlog is a `Semaphore` inside the task) | 2.13 s ± 0.16 s |

Raw hyperfine JSON: `viv-cold-extract-semaphore-ab.json`.

Caveat on same-session absolute numbers: this session's `api.github.com` and
`codeload.github.com` hop latencies drifted by well over 1 s across repeated
cold runs made minutes apart (visible in the per-host summary this change
adds), almost certainly GitHub-side throttling responding to the repeated
64-connection bursts this investigation itself generated. A `viv` vs `riff`
`hyperfine` comparison taken after that drift set in showed `viv` and `riff`
within noise of each other (sometimes `viv` ahead), where an earlier
comparison in the same session had `riff` ahead by the same ~0.6 s as the
table above — the fix's effect is real and reproducible (the A/B table),
but a fresh absolute `viv`-vs-`riff` cold number is not trustworthy from
inside one investigation session that hammers the same GitHub endpoints
repeatedly. Re-run `bench/run.sh bench/laravel riff viv` cold in a fresh
session for a comparable number.

## Real project: 105 packages, private repositories

A WordPress project with private GitHub dists and a private Composer
repository, 381 MB and 47,270 files in `vendor/`. Credentials from
`auth.json`. Same filesystem for vendor and cache, three runs each.

| Tool | Warm (cache, no vendor) |
|---|---|
| composer 2.10.2 (`--no-scripts --no-plugins`) | 3.19 s |
| viv 0.1.0 | 0.61 s |

viv's warm time is 526 ms of system time: one `link()` per file. The rerun
with `vendor/` present is a no-op. Cold install 8.3 s, dominated by the
private downloads. Output: every file under `vendor/composer` and the whole
package tree byte-identical to Composer's, `vendor/bin` included.

## Update, warm metadata

`update-warm` runs `viv update` (and `composer update --no-install`) with the
metadata cache already populated, so every Packagist file revalidates with a
304. It measures the resolver, not the network, but the revalidation round
trips still make it noisy enough to report rather than gate in CI.

| Release | Lock | viv | composer 2.10.2 | ratio |
|---|---|---|---|---|
| 0.4.0 | laravel, 101 packages | 8.7 s | 1.2 s | 7.2x slower |
| 0.5.0 | laravel, 101 packages | 4.9 s | 1.2 s | 4.1x slower |
| 0.4.0 | monolog, 3 packages | 0.20 s | 0.62 s | 3x faster |
| 0.4.0 + #76 (`PoolOptimizer`) | laravel, 101 packages | 14.1 s | 1.2 s | 11.8x slower |
| 0.4.0 + #76 (+ `CompiledConstraint`) | laravel, 101 packages | 8.2 s ± 0.3 s | 1.2 s | 6.9x slower |
| 0.4.0 + #76 (+ `VersionKey: Ord`) | laravel, 101 packages | 5.1 s ± 0.3 s | 1.2 s | 4.3x slower |

The Laravel gap was rule generation over a 45,604-version pool; #76 ports
`PoolOptimizer` to prune that pool before rule generation runs, and it works
as designed (rule generation 4.81 s → 0.58 s, pool 45,604 → 5,169 packages),
but `PoolOptimizer` itself then cost ~9.5 s, more than it saved. Two
follow-ups since: compiling each constraint's numeric bounds once instead of
re-parsing them on every `matches` call (`CompiledConstraint`, grouping loop
6.2 s → 0.8 s), then giving `VersionKey` an `Ord` so `DefaultPolicy` sorts on
a pre-parsed key instead of calling `crate::semver::compare` per pair
(selection loop 3.3 s → 0.15 s). Net effect: 14.1 s → 5.1 s, better than the
pre-#76 baseline for the first time, but still well above the 2 s target.
See `profile.md` §2.8 for the breakdown and the next hotspot found by
profiling (parsing every one of the pool's 45,604 packages' require/conflict
links, uncached, ~0.86 s — the same "re-parse the same string repeatedly"
shape both fixes above already addressed elsewhere, not yet touched here;
the metadata closure fetch itself, ~1.5 s, is now the single largest piece
and is network-bound, not algorithmic). Record this table for every release
next to the install numbers.
