# Upstream test fixtures

Copied verbatim from Composer's own test suites (MIT, see `LICENSE.*`).
Do not edit; refresh by re-copying and bumping the commits below.

| Directory | Source | Commit |
|---|---|---|
| `autoload/` | composer/composer `tests/Composer/Test/Autoload/Fixtures/` | 85ae025 (2026-09-03) |
| `repository/` | composer/composer `tests/Composer/Test/Repository/Fixtures/installed*.php` | 85ae025 |
| `installer/` | composer/composer `tests/Composer/Test/Fixtures/installer/*.test`, only `install` runs that have a `--LOCK--` section | 85ae025 |
| `classmap/` | composer/class-map-generator `tests/Fixtures/` | 9ba17a6 (2026-09-02) |

The case setups that drive these goldens are ported by hand into
`tests/autoload_goldens.rs` and `tests/classmap.rs`.
