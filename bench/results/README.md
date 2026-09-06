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
