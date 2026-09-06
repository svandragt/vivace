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
the package trees match. `viv` does not yet write `vendor/bin`.

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
