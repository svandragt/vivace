---
title: Results
order: 80
summary: Where the raw compatibility and benchmark evidence lives, one line per file.
---

# Results

The write-ups above cite numbers; these are the files those numbers come
from, kept in the repository rather than copied here so they can't drift.

## Compatibility

- [`compat/README.md`](https://github.com/svandragt/vivace/blob/main/compat/README.md) — how the sweep works, and every environment variable it reads.
- [`compat/results/v0.21.0.md`](https://github.com/svandragt/vivace/blob/main/compat/results/v0.21.0.md) and one file per earlier release — the sweep's install and lock-resolve comparison for that tag, pinned corpus and random sample both.
- [`compat/hunted.md`](https://github.com/svandragt/vivace/blob/main/compat/hunted.md) — public projects swept once outside the pinned corpus, so a repeat hunt starts from what's already covered.
- [`compat/results/lock-age.md`](https://github.com/svandragt/vivace/blob/main/compat/results/lock-age.md) — how often a lock 1, 2 or 4 years old still installs today (candidate B).
- [`compat/results/platform-drift.md`](https://github.com/svandragt/vivace/blob/main/compat/results/platform-drift.md) — how often a locked package's own PHP floor drifts from the root's claimed support (candidate D).
- [`compat/results/g3-plugins.md`](https://github.com/svandragt/vivace/blob/main/compat/results/g3-plugins.md) — what the most-downloaded Composer plugins actually do at install time (candidate 3.4).

## Benchmarks

- [`bench/results/README.md`](https://github.com/svandragt/vivace/blob/main/bench/results/README.md) — the headline cold/warm/no-op/update table against Composer and riff.
- [`bench/results/corpus.md`](https://github.com/svandragt/vivace/blob/main/bench/results/corpus.md) — the same scenarios across ten corpus projects, including vivacity.
- [`bench/results/profile.md`](https://github.com/svandragt/vivace/blob/main/bench/results/profile.md) — where install and update spend their time, flamegraphs included.
- [`bench/results/lockmerge.md`](https://github.com/svandragt/vivace/blob/main/bench/results/lockmerge.md) — the merge-conflict replay behind [chapter 1](research/chapter-1.html).
- [`bench/results/workspaces.md`](https://github.com/svandragt/vivace/blob/main/bench/results/workspaces.md) — the aggregate-root measurements behind [chapter 3](research/chapter-3.html).
- [`bench/results/storeload.md`](https://github.com/svandragt/vivace/blob/main/bench/results/storeload.md) — boot cost and disk use of a linked `vendor/` against the store (chapter 2).
- [`bench/results/g3-toolchain.md`](https://github.com/svandragt/vivace/blob/main/bench/results/g3-toolchain.md) — how often the corpus needs a PHP version or extension the host lacks (candidate 3.1).
- [`bench/results/g3-isolation.md`](https://github.com/svandragt/vivace/blob/main/bench/results/g3-isolation.md) — what popular plugins bundle their own Composer dependencies (candidate 3.2).
- [`bench/results/g3-isolation-sites.md`](https://github.com/svandragt/vivace/blob/main/bench/results/g3-isolation-sites.md) — how often a plugin's bundled library clashes with the site's own copy, on five Composer-managed WordPress sites (candidate 3.2 follow-up).
- [`bench/results/g3-rules.md`](https://github.com/svandragt/vivace/blob/main/bench/results/g3-rules.md) — a feature census behind simplifying resolution rules (candidate 3.5).
