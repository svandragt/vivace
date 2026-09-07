
## 2026-09-07T12:05:54Z

Partial run: stopped after the first project, with three build agents loading the machine, so the numbers are indicative only. Rerun with `make bench-corpus` on a quiet machine (#106).

viv 0.6.0, composer 2.10.2, riff 0.0.7, flags `--no-plugins --no-scripts`, 3 runs each.

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| laravel/laravel | 109 | composer | 8.324 | 1.537 | 0.925 | 1.819 |
| laravel/laravel | 109 | riff | 4.093 | 0.707 | 0.654 | n/a |
| laravel/laravel | 109 | viv | 3.312 | 0.080 | 0.008 | 8.714 |
