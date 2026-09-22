# Merge replay: composer.lock vs viv.lock (#273)

Chapter 1's measurement (`docs/research.md`): for every merge commit in a
corpus repo's history whose two parents both changed `composer.lock`
relative to their merge base, replay the three-way merge under
`composer.lock` as committed and under the native `viv.lock` conversion
(#272), and count textual conflicts (`git merge-file`'s exit code) under
each, plus real conflicts (packages both sides actually changed to
different results, computed from the JSON, format-independent -- the
control for whether a format's win was a real one).

Corpus and the range actually swept: `bench/lockmerge/corpus.toml`. Harness:
`bench/lockmerge/run.py`.

## 2026-09-22T10:41:09Z

Cap: 200 most recent qualifying merges per repository. viv binary: `/tmp/claude-1000/-home-sander-dev-rust-vivace/ce937e0b-be19-473c-89d9-aebb80828688/scratchpad/lockmerge-conv/target/release/viv` (viv 0.14.0, commit `299d8f8d8c86227f640e0847a7376aaf4720c73c`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Textual conflicts (composer.lock) | Textual conflicts (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|
| 0 | 0 | n/a | 0 | n/a |

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Textual conflicts (composer.lock) | Textual conflicts (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 |

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Textual conflicts (composer.lock) | Textual conflicts (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|
| 24 | 25 | 5 | 4 | 3 |

### Totals

| Merges examined | Textual conflicts (composer.lock) | Textual conflicts (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|
| 26 | 25 | 5 | 4 | 3 |
