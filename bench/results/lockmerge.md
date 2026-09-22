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

## 2026-09-22T10:55:16Z

Cap: 200 most recent qualifying merges per repository. viv binary: `/home/sander/dev/rust/vivace/.claude/worktrees/agent-ae9ffe51eb0a07055/target/release/viv` (viv 0.14.0, commit `fe0c6aeff73626f5394a71f57577269082d7dfab`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|
| 0 | 0 | n/a | 0 | n/a | 0 | n/a |

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 |

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 25 | 5 | 4 | 3 |

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 25 | 5 | 4 | 3 |

## Client corpus, anonymised (2026-09-22)

Four client projects with long `composer.lock` histories, replayed from
local clones with the same harness and the same `viv` binary as the public
run above. Names, commit ranges and paths are held outside the repository;
the table is reproducible by the maintainer from a scratch corpus and by
nobody else, and is included because the public corpus turned out to hold
almost no lock-conflicting history to replay.

| Project | Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| A | 200 (capped) | 126 | 52 | 1220 | 185 | 171 | 74 |
| B | 89 | 57 | 25 | 243 | 49 | 45 | 32 |
| C | 41 | 32 | 12 | 286 | 37 | 32 | 20 |
| D | 25 | 13 | 3 | 137 | 21 | 18 | 10 |
| **Client total** | **355** | **228** | **92** | **1886** | **292** | **266** | **136** |

With the public run: 379 merges, 235 conflicting under `composer.lock`
(62%), 96 under `viv.lock` (25%), 1911 hunks against 297, 270 real. In
every one of the five repositories `viv.lock`'s hunk count sits within 10%
of its real-conflict count.

## 2026-09-22T12:01:24Z

Cap: 200 most recent qualifying merges per repository. viv binary: `target/release/viv` (viv 0.14.0, commit `0c234e5fdbd3dfeb4a14a092b838c8ab70f4eb76`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|
| 0 | 0 | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 25 | 5 | 4 | 3 |

### Resolution archaeology

| Merges with real conflicts | Source conflict | Lock-only |
|---|---|---|
| 3 | 1 | 2 |

| Ours | Theirs | Neither | Removed | Higher version (of ours+theirs) | n/a (dev) |
|---|---|---|---|---|---|
| 4 | 0 | 0 | 0 | 3/4 (75%) | 0 |

| Median cascade | Max cascade |
|---|---|
| 0 | 0 |

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 25 | 5 | 4 | 3 |

### Resolution archaeology

| Merges with real conflicts | Source conflict | Lock-only |
|---|---|---|
| 3 | 1 | 2 |

| Ours | Theirs | Neither | Removed | Higher version (of ours+theirs) | n/a (dev) |
|---|---|---|---|---|---|
| 4 | 0 | 0 | 0 | 3/4 (75%) | 0 |

| Median cascade | Max cascade |
|---|---|
| 0 | 0 |
