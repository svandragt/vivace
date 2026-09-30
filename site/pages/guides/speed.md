---
title: Speed
order: 100
summary: The benchmark numbers against Composer, riff and vivacity, and how every release since is gated against the last.
---

# Speed

10 projects from viv's compatibility corpus, 2026-09-15, from a local
mirror so no network is measured, AMD Ryzen 9 7900X3D (24 threads) on
ext4, viv 0.13.0, composer 2.10.2, riff 0.0.7, vivacity 0.6.0,
`--no-plugins --no-scripts` on every tool and `--no-fallback` on vivacity
so a run it hands to Composer can never count as its own, three runs each.
Each cell is the geometric mean of how many times faster viv is, with the
range across projects; below 1× viv is slower. Per-project numbers are in
[`bench/results/corpus.md`](bench/results/corpus.md), section
`2026-09-15T06:54:06Z`. riff's column measures the same job as viv and
Composer (checksums, `platform_check.php`, proxies, `installed.*`); the
cosmetic differences are listed in
[`bench/results/README.md`](bench/results/README.md). vivacity is the
closest comparison: it makes the same byte-identical promise. Its column
covers the 6 projects it installs; on the other 4 it exits rather than
install a lock naming a plugin outside its list, even with
`--no-plugins`.[^7]

| Scenario | viv vs Composer | viv vs riff | viv vs vivacity |
|---|---|---|---|
| Cold | 5.6× (2.8 to 11.1) | 2.5× (1.2 to 101.3)[^6] | 2.0× (1.8 to 2.8) |
| Warm | 17.8× (6.8 to 42.8) | 6.9× (2.7 to 149.8)[^6] | 2.0× (1.1 to 3.2) |
| No-op | 42.0× (19.0 to 105.5) | 13.9× (2.5 to 57.9) | 6.8× (3.0 to 15.2) |
| Update-warm | 1.9× (1.3 to 4.0) | n/a | 1.5× (1.1 to 1.8) |

The table has not been re-measured since 0.13.0; each release since is
gated against the previous one instead. v0.15.0 against v0.16.0 with
`make bench-ab` on laravel, symfony/demo and drupal: warm and no-op flat
or faster on all three (drupal warm 296 ms to 285 ms, laravel 41 ms to
39 ms, symfony 47 ms to 47 ms; no-op within 0.2 ms). v0.16.0 against
v0.17.0's #311 (one `Snapshot` per run keys every install cache): warm
within ±15 ms and no-op within ±1.1 ms on the same three projects, the
sign flipping between runs; a 60-run laravel no-op hyperfine gave 5.2 ms
to both binaries, the remaining spread coming from load on the machine,
not the change. v0.17.0 against v0.18.0's compatibility fixes (time
format, mixed-case install paths, the classmap tie-break, the alias
`self.version` listing): warm and no-op flat within noise on laravel,
drupal and symfony/demo. v0.19.0's changes were each gated the same way
as they landed: the plugin adapters as data (#340) measured laravel warm
37.5 ms to 36.8 ms, symfony/demo 48.1 ms to 44.7 ms, drupal 298.7 ms to
294.7 ms, no-op within 0.4 ms; the scaffold and patch bindings (#341)
and the root-version guess in the solve (#312) flat within noise.
v0.20.0's `viv.lock`-only install path (#344): no-op within 0.4 ms on the
same three projects.

A warm update still revalidates every package's metadata with the registry,
one conditional request each, even when nothing changed. `--metadata-ttl
<seconds>` on `update`, `add` and `rm` (or `VIV_METADATA_TTL`; the flag wins)
skips that revalidation for a package whose cached metadata is younger than
the window, so a second update run shortly after the first makes no metadata
requests at all. It's off by default (`0`, always revalidate, matching
Composer), so turn it on only where a slightly stale registry view for a few
minutes is an acceptable trade for the extra speed. `--offline` always wins
over a configured window.

## Speed

Composer, riff, vivacity and viv on the same corpus and the same
compatibility sweep. Every number below comes from a results file in the
repository, not a guess — see the sources under each table.

{{speed_table}}

{{speed_source}}

Skips recorded for this run:

{{speed_skips}}

## Capability

{{capability_table}}

`n/a` and "not measured here" cells are viv's own gaps too, not just a
missing measurement for someone else: see [`docs/stability.md`](https://github.com/svandragt/vivace/blob/main/docs/stability.md)
for what viv's output contract does and doesn't cover.

[^6]: riff's phpunit/phpunit cold and warm times (7.4 s and 7.3 s) are an outlier against its other rows in this corpus; kept in the range, not dropped; see [`bench/results/corpus.md`](bench/results/corpus.md).
[^7]: roots/bedrock, drupal/recommended-project, yiisoft/yii2-app-basic and craftcms/craft, recorded in [`bench/skips.txt`](bench/skips.txt) against vivacity 0.6.0 so a newer release is retried. A refusal is an `n/a` cell, never a slow one.
