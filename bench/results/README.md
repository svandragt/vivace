# Baseline: install from lock, 101 packages (bench/laravel)

Machine: AMD Ryzen 9 7900X3D, ext4, Linux 7.0, 2026-09-06. PHP 8.4.24.
`bench/run.sh bench/laravel composer riff presto`, 3 runs each, means.

| Tool | Cold (no cache, no vendor) | Warm (cache, no vendor) | No-op (vendor present) |
|---|---|---|---|
| composer 2.10.2 | 7.30 s | 1.09 s | 0.50 s |
| riff 0.0.7 | 1.74 s | 0.23 s | 0.23 s |
| presto 0.1.12 | 6.46 s | 6.33 s | 3.41 s |

Notes

- Presto has no download cache despite its README, so warm equals cold. It
  also rewrites `composer.lock` and emits a non-Composer autoloader with no
  classmap and stubbed `ClassLoader`/`InstalledVersions`.
- Riff's warm and no-op times are the same. `--no-audit` brings no-op to
  157 ms. strace shows 25 `php` processes spawned during a no-op install
  (platform detection), which is most of the remaining floor. Warm time is
  dominated by system calls from unzipping every archive into `vendor/`.
- Riff's `vendor/composer/*` differs from Composer's on this lock only in:
  ignoring `config.autoloader-suffix` (uses the lock content-hash),
  `provide` placed after `require-dev` in `installed.json`, `NULL` instead of
  `null` in `installed.php`, and an `InstalledVersions.php` from a newer
  Composer commit. Everything else is byte-identical.

Raw hyperfine JSON: `composer.json`, `riff.json`, `presto.json`.
