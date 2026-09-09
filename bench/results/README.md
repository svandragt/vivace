# Install from lock, 101 packages (bench/laravel)

Machine: AMD Ryzen 9 7900X3D, ext4, Linux 7.0, PHP 8.4.24, 2026-09-07.
`bench/run.sh bench/laravel composer riff viv`, three runs each, means.
Vendor directory and caches on the same filesystem (hardlinks need that;
across filesystems `viv` falls back to copying with a warning).

Scenarios: cold is no cache and no `vendor/`; warm is cache present, no
`vendor/`; no-op is `vendor/` present and up to date; update, warm metadata
resolves with the metadata cache already populated.

| Tool | Cold | Warm cache | No-op | Update, warm metadata |
|---|---|---|---|---|
| composer 2.10.2 | 7.28 s | 1.05 s | 0.47 s | 1.23 s |
| riff 0.0.7 | 1.74 s | 0.24 s | 0.24 s | fails\* |
| viv 0.6.0 | 2.26 s | 0.043 s | 0.008 s | 1.12 s |
| presto 0.1.12 (earlier run) | 6.46 s | 6.33 s | 3.41 s | n/a |

\* see "Update, warm metadata" below.

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

## CI bench gate (#164)

The `bench` job runs two projects, gated against `baseline.json`'s
per-project entry: `tests/fixtures/monolog` (2 packages, a fast smoke
check) and `bench/laravel` (101 packages, closer to a real project's
warm/no-op times). `bench/compare.py --project <name>` keys the baseline so
either can be missing without failing the other.

The gate compares a ratio, not a raw mean: GitHub runners vary 30-45% run to
run on identical code (Laravel `warm` measured 64 ms one run and 93 ms the
next, confirmed by a local A/B), so a seconds-based baseline chases runner
noise, not regressions. `bench/compare.py` instead computes
`ratio = viv_mean / composer_mean` for the same scenario, measured in the
same job on the same runner in the same minute — the runner's speed cancels
out of the ratio even though it doesn't cancel out of either mean alone.
`baseline.json` stores these ratios, e.g. `{"monolog": {"warm": 0.09, ...}}`.

`update-offline` (#165) runs `viv update --offline --no-install` against the
same warm metadata cache as `update-warm`, but `--offline` reads the cache
only and makes no revalidation requests at all, so it measures parsing the
closure and running the solver with no network in the loop. That isolation
is exactly what makes it gated, unlike `update-warm`: `update-warm`'s time
still includes hundreds of 304 round trips, so a run-to-run swing there could
be GitHub, not a regression in `viv` (#159); `update-offline`'s variance is
ours alone, the same reasoning that gates `warm` and `noop`. Composer has no
offline update flag, so `update-offline`'s ratio uses composer's
`update-warm` mean as its denominator instead: same runner, same minute, so
it's still a stable number to divide by, even though it isn't the same
scenario.

`baseline.json` never updates itself: every release downloads the CI run's
`baseline-candidate` artifact and commits it as the new `baseline.json` (see
"After a release" in `AGENTS.md`). A scenario's new baseline ratio may only
go down or stay within tolerance of the old one; a rise is a regression to
explain in the release notes, not a new floor.

## Local mirror (#165, widened)

`bench/mirror.sh <project-dir> <mirror-dir>` records a project once: every
locked package's dist zip, and p2 metadata (both the release and `~dev`
files) for every locked package plus everything named in `composer.json`'s
require/require-dev. Metadata comes from every repository the project
names, not just Packagist (#171): Packagist is tried first (unless disabled
with a `{"packagist.org": false}` entry), then each `"composer"`-type
repository in listed order, stopping at the first that has the package. A
v2 (`metadata-url`) repository is recorded as-is; a v1
(`providers-url`/`provider-includes`) one, e.g. `asset-packagist.org`'s
bower-asset/npm-asset packages, has its provider listing walked once to
find each package's hash, and the per-package `packages` object recorded
straight into the mirror's own p2 files. It then rewrites every dist URL in
the recorded p2 files to point at a not-yet-known local port
(`http://127.0.0.1:__PORT__/...`), derived from the package name and dist
reference so the rewrite never depends on what the URL already was.
Rerunning a complete recording changes nothing (dist files are skipped when
present; the p2 rewrite recomputes the same URL every time).

`bench/run.sh` and `bench/corpus.sh` (`BENCH_MIRROR=<dir>` and
`BENCH_MIRROR=1` respectively) serve that recording with `miniserve` on a
free port for the run, substitute the real port into a served copy, and
rewrite each scenario's scratch `composer.json`/`composer.lock` to match:
the lock's dist URLs point at the mirror, `repositories` is
`[{"type":"composer","url":"http://127.0.0.1:<port>"},
{"packagist.org":false}]`, and `config.secure-http` is turned off for that
copy (the mirror is deliberately plain HTTP on loopback). Composer and riff
both accept this; viv needs no change since it reads dist URLs straight
from the lock.

Serving it with python's `http.server` was tried first and measured
*slower* than the real network: `bench/laravel` cold went to 5.5s for viv
and 4.5s for riff, against ~1.1s for viv from real Packagist on a GitHub
runner. `http.server`'s `ThreadingHTTPServer` defaults to HTTP/1.0 (no
keep-alive, so every one of the 101 dists opens a fresh connection and
thread); `--protocol HTTP/1.1` alone dropped viv's median per-dist hop from
1050ms to 57ms and cold to ~0.21s, but the server still couldn't survive
raw concurrency — `curl --parallel` fetching all 101 dists at once against
it took over 90 seconds with connection resets. `miniserve` (added via
`devbox add`) serves the same 101 dists over `curl --parallel` in well
under a tenth of a second and needed no protocol flag, so it replaced
`http.server` outright rather than keeping the HTTP/1.1 flag as a fix.

This is why `cold` and `update-warm` become reproducible: `cold` used to
wait on real Packagist and GitHub, so its variance was partly network, not
`viv`; against the mirror, both are gated the same way `warm`/`noop`/
`update-offline` already are (see "CI bench gate" above). The honest
caveat: `cold`'s numbers no longer include real Packagist or GitHub
latency, so they're a lower bound on a real cold install, not a substitute
for one — the corpus run and any release-facing number should say so.

Two mirror-specific carve-outs, both because the mirror only records the
one locked dist per package, not every version Packagist could offer:

- viv's `update-warm` command has always run install too (unlike
  composer/riff's `--no-install` in this scenario); against a mirror a
  legitimate version bump has nothing to install from, so `run.sh` adds
  `--no-install` for viv only when `BENCH_MIRROR` is set, leaving the
  real-network baseline unchanged.
- riff's `update-warm` fails against the mirror: it issues a duplicate
  conditional GET for the same p2 file within one request burst, gets a
  `304` back for one of them, and treats that empty body as the package
  having zero versions instead of reusing its first, already-fetched
  response. This reproduces against both `http.server` and `miniserve`, so
  it's riff's own request handling racing itself, not one server's `304`
  behaviour; real Packagist never returns a `304` to a same-burst request,
  so this only shows up against a low-latency local mirror and isn't worth
  working around here. `run.sh` skips riff's `update-warm` under
  `BENCH_MIRROR` with the usual skip-note on stderr.

## Corpus

The same four scenarios run across the pinned public projects
(`compat/corpus.toml`) rather than just Laravel: results are in
`bench/results/corpus.md`, produced by `bench/corpus.sh`. Known
per-tool/per-project failures it skips instead of re-running every night are
in `bench/skips.txt`.

## Corpus aggregates

`bench/aggregate.py` turns the latest `## <timestamp>` section of
`bench/results/corpus.md` into the README's speed table: per scenario, the
per-project ratio composer/viv and riff/viv, then the geometric mean and the
min/max of those ratios. Geometric mean, not arithmetic, because these are
speedups (ratios), not additive quantities — a project 10x slower and one
0.1x slower should average to 1x, not 5x.

A project/tool cell that is `n/a` or missing a row (a known failure, see
`bench/skips.txt`) is skipped for that ratio, not treated as a zero; riff has
no `update-warm` numbers at all in this corpus, so that column is `n/a`.
When a ratio's range crosses 1×, the cell gets a footnote saying so rather
than a plain claim either way.

Regenerate with:

```sh
python3 bench/aggregate.py bench/results/corpus.md
```

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

riff (#87): `bench/run.sh` runs `riff update --no-interaction --no-progress
--quiet --no-install --no-blocking` (`--no-blocking` disables riff's
resolve-time security-advisory check, which otherwise rejects
`phpunit/phpunit` before it gets anywhere near the lock comparison). Even so,
riff 0.0.7 can't resolve `bench/laravel`'s own root requirement: it fetches
`laravel/framework`'s full provider metadata (990 KB, 1,290 versions) over
the network fine, but `riff_core::solver::rule_generator` then logs "Pool has
0 versions of laravel/framework" and fails, for every constraint tried
(`^12.0`, an exact `12.69.1`, even `^11.0`) and with or without a warm cache.
A minimal repro (a fresh project requiring only `laravel/framework`, any
constraint) reproduces it; the same repro shape with `monolog/monolog`
resolves normally, so this isn't riff being unreachable or the lock/flags
being wrong, it's specific to this package's metadata (likely riff's
expansion of Packagist's minified `composer/2.0` provider format choking on
laravel/framework's unusually long version list). `bench/run.sh` reports the
failure and moves on rather than aborting the whole run; the Numbers table's
`n/a` for riff's update column stands until upstream fixes it.

`bench/corpus.sh` skips known per-project failures like this one rather than
re-running them every night; see `bench/skips.txt` for the list. To retry a
skip once a tool's fixed it, delete the skip's line, or bump its pinned
version to match the tool's actual current release so the entry stops
matching.

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

## riff vs Composer output (#142)

2026-09-08. riff 0.0.7, composer 2.10.2, viv 0.7.0. One-off script,
`bench/riff-diff.sh` (not a permanent sweep column): for each project in
`compat/corpus.toml`, clone shallow (or `create-project` for
drupal/recommended-project), generate a lock if none is committed
(`composer update --no-install --no-scripts --no-plugins
--ignore-platform-reqs`), strip `.git`, then install three ways —
`composer install --no-scripts --no-plugins --no-interaction
--ignore-platform-reqs`, `riff install` with the same flags, `viv install
--no-scripts --no-plugins` — and `diff -rq --exclude=.git` riff's `vendor/`
against Composer's.

| Project | riff vs Composer `vendor/` |
|---|---|
| laravel/laravel | `installed.json`, `installed.php`, `InstalledVersions.php` differ |
| symfony/demo | `installed.php`, `InstalledVersions.php` differ |
| drupal/recommended-project | `autoload_real.php` differs; `include_paths.php` missing from riff's tree; `installed.json`, `installed.php`, `InstalledVersions.php` differ |
| roots/bedrock | `installed.json`, `installed.php`, `InstalledVersions.php` differ |
| composer/composer | `installed.php`, `InstalledVersions.php` differ |
| phpunit/phpunit | `autoload_classmap.php`, `autoload_files.php`, `autoload_real.php`, `autoload_static.php`, `installed.json`, `installed.php`, `InstalledVersions.php` differ |
| slimphp/Slim-Skeleton | `installed.php`, `InstalledVersions.php` differ |
| yiisoft/yii2-app-basic | riff install exits 1: `-p1: failed to apply patch to src/Iterator.php: error applying hunk #1` (pre-existing skip, `bench/skips.txt`) |
| statamic/statamic | `vendor/bin/sail` differs; `installed.json`, `installed.php`, `InstalledVersions.php` differ |
| craftcms/craft | `installed.php`, `InstalledVersions.php` differ |

`viv install` succeeded on every project above and is not in the table: a
separate byte-diff against Composer (already run for every release, see the
top of this file) is unaffected by this investigation.

What the differences actually are, traced with a manual re-run per file
(`command diff -u`, not just `-q`):

- **`installed.php`/`InstalledVersions.php`, every project.** Two distinct,
  reproducible causes, not per-project drift:
  - `installed.php`: when the root package has no VCS reference (true here
    because the corpus script strips `.git` before installing, the same as
    a tarball or CI checkout deploy), Composer's dumper writes the root
    package's `'reference' => null` (lower-case); riff's writes `'reference'
    => NULL` (PHP's `var_export` casing). Cosmetic — PHP's parser is
    case-insensitive for the `null`/`true`/`false` keywords — but it means
    riff's `installed.php` is not byte-identical to Composer's even when
    every package is.
  - `InstalledVersions.php`: riff's vendored template
    (`riff-core/src/autoload/InstalledVersions.php.template`) is missing a
    guard current Composer ships: `substr(__DIR__, -8, 1) !== 'C' &&
    is_file(__DIR__ . '/installed.php')` (Composer checks both; riff's
    template only checks the first half). Harmless when `installed.php` is
    always written alongside it, as it is here, but it is a stale copy of
    Composer's own runtime file, not a redesign.
- **`installed.json`, six projects.** The only content difference found
  (`phpunit/phpunit`'s copy, diffed field by field): one package,
  `phpunit/php-code-coverage`, is recorded `"installation-source": "dist"`
  by Composer but `"source"` by riff, even though neither `--prefer-source`
  nor a per-package `prefer-install` override is in play. Cause:
  `riff-core/src/downloader/manager.rs:216`,
  `DownloadPreference::Auto if package.is_dev() => [DownloadSource::Source,
  DownloadSource::Dist]` — riff defaults *dev-stability* packages (locked
  version resolves to `Stability::Dev`, `riff-core/src/package/package.rs:802`)
  to source over dist even outside `--prefer-source`. This matches
  Composer's own semantics for dev packages (Composer also prefers source
  for dev stability by default) rather than being a riff-only quirk; it
  wasn't reproduced with `--ignore-platform-reqs` alone and needed the real
  corpus lock to show up, so it's specific to a resolved-dev-version package
  landing in a real lock, not a broad riff/Composer disagreement.
- **`drupal/recommended-project`'s missing `include_paths.php`.** Composer
  only writes this file when a package's `autoload.include-path` (a
  deprecated Composer 1.x-era key still used by a couple of Drupal core
  dependencies) is set; riff's autoload generator has no equivalent, so the
  file is silently absent rather than empty. A real, narrow gap in autoload
  generation (not scripts, not `platform_check.php`, not the classmap).
- **`phpunit/phpunit`'s extra autoload file differences.** Composer and riff
  both classmap-scan `phpunit/php-code-coverage`'s `src/` (matching the
  `installed.json` finding above, this package installs from source under
  riff and from dist under Composer), which puts different file layouts —
  and hence different scanned class lists — under `autoload_classmap.php`,
  `autoload_static.php` and `autoload_files.php` for that one package. Not
  an independent bug: same root cause as the `installation-source` finding.
- **`statamic/statamic`'s `vendor/bin/sail` proxy.** riff's Unix proxy
  (`riff-core/src/installer/binary.rs`'s `create_unix_proxy`,
  ~line 147) is a short `#!/usr/bin/env sh` wrapper that `cd`s via
  `$0`/`dirname` and execs the target directly. Composer's proxy (and
  viv's, matching it) is Composer's full-compatibility template: resolves
  `$BASH_SOURCE` with a `realpath` fallback, handles being `source`d, and
  runs the target through `php` explicitly when it has a `#!...php`
  shebang. Both work for a plain `sh script args` invocation; riff's is a
  smaller, less defensive proxy than Composer's, not a functional gap this
  investigation could break.

Not skipped: sha1/sha256 dist checksum verification
(`riff-core/src/downloader/checksum.rs`, called from `downloader/manager.rs`
whenever the lock has a `dist.shasum`), `platform_check.php` generation
(`installer.rs`'s `configure_platform_check`/`platform_check_requirements`,
~line 3201), `vendor/bin` proxies (`installer/binary.rs`, full PHP-shebang
detection included), and `installed.json`/`installed.php` (generated on
every install unless `--no-autoloader`, `installer.rs`'s
`generate_installed_metadata`, ~line 2364). None of the issue's five
hypothesised skips (sha1, `platform_check.php`, `installed.php`, the
classmap scan, `vendor/bin` proxies) turned out to be actually skipped;
every difference found above is either a formatting/staleness bug or a
smaller-but-present implementation of the same step.

## Cold install profile: drupal/recommended-project and Slim-Skeleton (#142)

2026-09-08, same machine as above. Three alternating cold runs each
(`hyperfine --warmup 0 --runs 3`, empty cache, no `vendor/`, prepared before
every run), plus one `strace -f -e trace=connect,openat -c` run each and one
`RUST_LOG=vivace=debug` run for viv's per-host hop summary.

| Project | Tool | Cold (hyperfine) | `openat` calls | `connect` calls | Files in `vendor/` |
|---|---|---|---|---|---|
| slimphp/Slim-Skeleton (57 packages) | riff 0.0.7 | 1.628 s ± 0.122 s | 5,782 | 243 | 4,231 |
| slimphp/Slim-Skeleton (57 packages) | viv 0.7.0 | 2.049 s ± 0.076 s | 6,936 | 224 | 4,233 |
| drupal/recommended-project (67 packages) | riff 0.0.7 | 3.878 s ± 0.331 s | 25,297 | 291 | 24,329 |
| drupal/recommended-project (67 packages) | viv 0.7.0 | 4.202 s ± 0.191 s | 38,684 | 211 | 24,332 |

`viv`'s per-host hop summary (`RUST_LOG=vivace=debug`, request counts equal
the package count on a cold install with no cache — one `api.github.com`
redirect hop and one `codeload.github.com` body-complete hop per package):

- Slim-Skeleton: `api.github.com` 57 requests, 358–614 ms (median 398 ms);
  `codeload.github.com` 57 requests, 348–959 ms (median 498 ms).
- drupal/recommended-project: `api.github.com` 67 requests, 239–727 ms
  (median 626 ms); `codeload.github.com` 67 requests, 280–2,307 ms (median
  526 ms).

Both cold wins (riff 1.26x on Slim-Skeleton, 1.08x on drupal, in the same
range as the issue's 3.9 s/4.3 s and 1.7 s/2.1 s) are **mostly noise, not
skipped work or a materially different pipeline**. `connect` counts are the
same order of magnitude both ways (riff and viv both open one connection per
package to `api.github.com` and a smaller, redirect-driven number to
`codeload.github.com`, matching the existing `strace` finding above this
investigation reused rather than repeated), and the per-host hop medians
above show hundreds of milliseconds of run-to-run GitHub-side latency
variance on their own — comparable in size to the entire cold-time gap
between the two tools. The one repeatable structural difference is
`openat` volume: viv issues roughly 1.6 open calls per file written on both
projects, riff roughly 1.0–1.6 (1.04 on drupal, 1.34 on Slim-Skeleton); viv's
content-addressed store (`src/store.rs`) opens each extracted file again to
compute its sha256 store key and again to `link()` it into `vendor/`, where
riff unzips straight into `vendor/` without a separate keyed store. That
extra bookkeeping is real but its cost here is under a third of a second of
`openat` time on the larger project (0.36 s vs 0.33 s in the `strace -c`
summary), well inside the network-latency noise already documented for this
corpus. Nothing in the syscall counts or hop timings supports a pipeline
riff has and viv doesn't; the two cold-column projects in the issue are
better explained by the GitHub-latency variance `bench/results/README.md`
already flags for cold numbers than by any skipped or faster step.

