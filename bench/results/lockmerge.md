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

## 2026-09-22T12:18:51Z

Cap: 200 most recent qualifying merges per repository. viv binary: `target/release/viv` (viv 0.14.0, commit `f08c5b76dda58d4544ba5c4e5a09bebbf73e8eb9`).

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Client corpus, resolution archaeology (2026-09-22)

Same four projects, same harness, `viv` at `f08c5b7`/`7ca7a0c`. Counts only.

| Project | Merges with real conflicts | Source conflict | Lock-only |
|---|---|---|---|
| A | 46 | 7 | 39 |
| B | 23 | 7 | 16 |
| C | 11 | 2 | 9 |
| D | 2 | 1 | 1 |
| **Client total** | **82** | **17** | **65** |

Ten further merges conflict under `viv.lock` with no package changed on
both sides: adjacency in the record format, no re-solve needed.

| Project | Ours | Theirs | Neither | Removed | Higher version (of ours+theirs) | n/a (dev) |
|---|---|---|---|---|---|---|
| A | 56 | 81 | 11 | 8 | 91/112 (81%) | 25 |
| B | 25 | 17 | 3 | 0 | 8/17 (47%) | 25 |
| C | 5 | 24 | 0 | 3 | 12/23 (52%) | 6 |
| D | 10 | 2 | 6 | 0 | 12/12 (100%) | 0 |
| **Client total** | **96** | **124** | **20** | **11** | **123/164 (75%)** | **56** |

| Project | Median cascade | Max cascade |
|---|---|---|
| A | 0 | 12 |
| B | 0 | 5 |
| C | 0 | 1 |
| D | 7 | 14 |
| **Client total** | **0** | **14** |

| Project | Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation |
|---|---|---|---|
| A | 7 | 7 | 0 |
| B | 7 | 5 | 2 |
| C | 2 | 2 | 0 |
| D | 1 | 1 | 0 |
| **Client total** | **17** | **15** | **2** |

No lock-only merge becomes a source conflict after normalisation.

## 2026-09-22T14:13:50Z

Cap: 200 most recent qualifying merges per repository. viv binary: `./target/release/viv` (viv 0.14.0, commit `3ab072e187b882e7842e6e05f8fd575be2f61db7`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 3 | 25 | 5 | 4 | 3 |

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 3 | 25 | 5 | 4 | 3 |

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

## 2026-09-22T14:52:53Z

Cap: 200 most recent qualifying merges per repository. viv binary: `./target/release/viv` (viv 0.14.0, commit `5ebcf036f522bed59a9682c8d682ea91c15278fd`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 3 | 25 | 5 | 4 | 3 |

- eae2dafcacd7: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 35285b73a30b: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 58520bca2aa1: viv lock merge re-solve did not finish: - Root composer.json requires intervention/image-driver-vips ^1.0 -> satisfiable by intervention/image-driver-vips[1.0.10].

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 3 | 25 | 5 | 4 | 3 |

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

## 2026-09-22T15:21:26Z

Cap: 200 most recent qualifying merges per repository. viv binary: `./target/release/viv` (viv 0.14.0, commit `34e51677cc29ad6f47d0c1750762560db06af931`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 3 | 25 | 5 | 4 | 3 |

- eae2dafcacd7: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 35285b73a30b: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 58520bca2aa1: viv lock merge re-solve did not finish: - Root composer.json requires intervention/image-driver-vips ^1.0 -> satisfiable by intervention/image-driver-vips[1.0.10].

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 3 | 25 | 5 | 4 | 3 |

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

## 2026-09-22T15:29:05Z

Cap: 200 most recent qualifying merges per repository. viv binary: `./target/release/viv` (viv 0.14.0, commit `b1dd848d7d66576d1d8262a009cc1680b7eb4347`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 3 | 25 | 5 | 4 | 3 |

- eae2dafcacd7: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 35285b73a30b: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 58520bca2aa1: viv lock merge re-solve did not finish: - Root composer.json requires spatie/laravel-backup ^9.2.9 -> satisfiable by spatie/laravel-backup[9.3.6].

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 3 | 25 | 5 | 4 | 3 |

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

## 2026-09-22T15:35:58Z

Cap: 200 most recent qualifying merges per repository. viv binary: `./target/release/viv` (viv 0.14.0, commit `f83f95c6f081747ebf0e47c86ff1f5b63e5245bf`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 2 | 25 | 5 | 4 | 3 |

- eae2dafcacd7: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 35285b73a30b: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 2 | 25 | 5 | 4 | 3 |

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

## 2026-09-22T16:00:42Z

Cap: 200 most recent qualifying merges per repository. viv binary: `./target/release/viv` (viv 0.14.0, commit `53e40ad1d48a7a73c08999b18ed2a59dda9e59a2`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 2 | 25 | 5 | 4 | 3 |

- eae2dafcacd7: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 35285b73a30b: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 2 | 25 | 5 | 4 | 3 |

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

## 2026-09-22T16:22:35Z

Cap: 200 most recent qualifying merges per repository. viv binary: `./target/release/viv` (viv 0.14.0, commit `94757f74b12af83c7fff74ce7908a64c7864cf26`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 2 | 25 | 5 | 4 | 3 |

- eae2dafcacd7: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.
- 35285b73a30b: viv lock merge re-solve did not finish: - Root composer.json requires pragmarx/google2fa, it could not be found in any version, there may be a typo in the package name.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 2 | 25 | 5 | 4 | 3 |

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Client corpus, `viv lock merge` (2026-09-22)

Same four projects, driver at `5e020d0` with the platform declaration and
`--as-of` the merge commit's date. Counts only.

| Project | Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) |
|---|---|---|---|---|
| A | 200 (capped) | 126 | 52 | 32 |
| B | 89 | 57 | 25 | 21 |
| C | 41 | 32 | 12 | 7 |
| D | 25 | 13 | 3 | 1 |
| **Client total** | **355** | **228** | **92** | **61** |

Without re-solving (record merge only) the driver column is 88. The 61
by leaf cause: 30 `dev-*` branches whose current head conflicts where the
historical head did not (unreplayable: Packagist serves only today's
head); 16 inline `package` repositories viv refuses (#294); 6 the
platform heuristic declaring php too low; 4 packages gone from Packagist;
4 constraint chains that may be genuine; 1 malformed historical manifest.
`--as-of` changed nothing: the residue is branches, not releases.

### Client corpus, `viv lock merge` with package repositories (2026-09-22)

Same four projects, driver at `6fadb8d` (#294 landed). Counts only.

| Project | Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) |
|---|---|---|---|---|
| A | 200 (capped) | 126 | 52 | 29 |
| B | 89 | 57 | 25 | 15 |
| C | 41 | 32 | 12 | 7 |
| D | 25 | 13 | 3 | 1 |
| **Client total** | **355** | **228** | **92** | **52** |

The sixteen merges blocked on `package` repositories now solve: nine
clean, seven through to `dev-*` branches whose heads have moved. Leaf
causes of the 52: 37 `dev-*` heads moved (unreplayable); 6 the platform
heuristic's php floor; 4 packages gone from Packagist; 4 constraint chains
(#296's target); 1 malformed historical manifest.

## 2026-09-22T22:54:00Z

Cap: 200 most recent qualifying merges per repository. viv binary: `/home/sander/dev/rust/vivace/.claude/worktrees/agent-a917d73785f4a8b39/target/release/viv` (viv 0.14.0, commit `e314829cbd04ae3fcbfb2e08021c80cf1a52ae0b`).

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 0 | 25 | 5 | 4 | 3 |

Resolved rung 1 (closure): 1, rung 3 (seeded): 2. 1 merge(s) moved ≥1 package outside the divergent set.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 0 | 25 | 5 | 4 | 3 |

Resolved rung 1 (closure): 1, rung 3 (seeded): 2. 1 merge(s) moved ≥1 package outside the divergent set.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

## Client corpus, anonymised, after #296 and #297 (2026-09-23)

Same four projects, same 355 merges, `viv` at main `e197d67` (three
escalation rungs, #296; `viv.lock` as a companion to `composer.lock`,
#297). Counts only, as above.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts |
|---|---|---|---|---|---|---|
| 355 | 228 | 92 | 51 | 1886 | 292 | 268 |

Resolved: rung 1 (closure) 36, rung 3 (seeded) 1; 2 merges moved a
package outside the divergent set. Rung 2 never finished a merge that
rung 1 could not.

Leaf cause of the 51 the driver still cannot finish:

| Leaf cause | Merges |
|---|---|
| A `dev-*` branch whose current head conflicts with a pinned release (the registry serves only today's head) | 43 |
| Root requires a package Packagist no longer lists | 7 |
| Malformed `composer.json` (trailing comma) | 1 |

None of the 51 is a constraint chain or the harness's platform heuristic
any more: the deeper rungs push those to a `dev-*` or missing-package
leaf, which no replay can reproduce.

## Ledger lock, candidate A (#306), 2026-09-25

Candidate A (`docs/research.md`): the lock written as an ordered ledger of
package-record changes, `git merge-file --union`'d and folded, compared
against `viv lock merge`'s own result on the same merge
(`bench/lockmerge/run.py --ledger`). Same 355 client merges as above, plus
the public corpus (koel, pixelfed; flarum has no committed lock, monica
has no qualifying merge). viv `0.16.0`, commit `59366290e69106c8fccf7514d83a990d662e65b3`.

| Corpus | Merges examined | Identical | Silent fold | Fold refused, driver succeeded | Both refused | Fold succeeded, driver refused |
|---|---|---|---|---|---|---|
| Client (anonymised, four projects) | 355 | 254 | 0 | 49 | 51 | 1 |
| Public | 26 | 22 | 0 | 4 | 0 | 0 |
| **Total** | **381** | **276** | **0** | **53** | **51** | **1** |

Silent fold is zero: no merge where the fold succeeded despite a real
conflict (a package both sides changed to different results), and no
merge where the fold's successful result disagreed with the driver's.

The 104 merges where the fold refused (53 "fold refused, driver
succeeded" plus 51 "both refused") split cleanly into two kinds. 90 are
genuine real conflicts, a package both sides changed to different
results -- the fold refusing every one of these, with no exception, is
the silent-fold count above being zero. The other 14 (one in the public
corpus, thirteen in the client corpus) are not real conflicts at all:
both sides moved a package to the identical version and source
reference, but one side's lock entry carried a metadata field the
other's did not (`notification-url`, confirmed by hand on one client
case) -- the fold's full-record hash reads that as a fork and refuses,
where the driver's version/reference/dev identity check does not. The
single "fold succeeded, driver refused" merge is the malformed-
`composer.json` case already named in the leaf-cause table above: the
fold never reads `composer.json`, so a manifest parse error that blocks
the driver's re-solve doesn't block it.

## Hybrid against driver only, candidate A (#306), 2026-09-25

Same 381 merges (355 client, 26 public), same `bench/lockmerge/run.py
--hybrid` (implies `--ledger`). Adds an identity-hash fold variant --
`(op, name, version, source reference, dist reference when there is no
source, section)` instead of the full record -- and, per merge, times the
fold (building each side's ledger lines against an already-computed base
state, the union, and the fold; excludes the one-off cost of building
that base state, paid once per merge) against the driver's existing `viv
lock merge` call. Option 1 is the driver alone; option 2 ("hybrid") is
the identity-hash fold first, falling back to the driver only when the
fold refuses. viv `0.16.0`, commit `36de58067d4c36fa39e5b698b9c758a65b431ec3`.
Load average at the start of the client run: 3.45, 3.20, 2.66; at the
end: 2.48, 2.32, 2.33.

| Corpus | Merges examined | Finished, driver only | Finished, hybrid | Fold alone, full hash | Fold alone, identity hash | Silent fold (identity hash) | Fold picks, metadata-only | Hybrid ≠ driver (both finished) | Driver median / p95 (ms) | Hybrid median / p95 (ms) |
|---|---|---|---|---|---|---|---|---|---|---|
| Client (anonymised, four projects) | 355 | 303 | 304 | 255 | 268 | 0 | 1 | 0 | 42 / 15114 | 6 / 10017 |
| Public | 26 | 26 | 26 | 22 | 23 | 0 | 1 | 0 | 49 / 3135 | 7 / 3143 |
| **Total** | **381** | **329** | **330** | **277** | **291** | **0** | **2** | **0** | n/a (medians don't pool across two separately run corpora) | n/a |

Silent fold (identity hash) is zero on both corpora: no merge finishes
over a real conflict under the coarser hash either. Hybrid result ≠
driver result is also zero: every merge both paths finish agrees on
package identity (name, version, reference per section). The identity
hash closes exactly the 14 false forks the full-record hash gave (291 vs
277 finished by the fold alone): 2 needed the fold's deterministic pick
(the record whose canonical JSON sorts first) because both sides carried
the same version and reference but a metadata field only one side had;
the other 12 needed no pick, because the identity hash already read one
side as unchanged from base. Separately, in 20 client and 3 public merges
(23 total) the hybrid and driver package identity sets agree but the full
lock record still differs -- the driver rewrites the content hash and
platform declaration, and the fold never does, so
agreement on which versions to keep does not mean byte-identical output.

Timing: the driver's median stays under 50ms per merge on both corpora,
but its p95 runs into seconds (15.1s client, 3.1s public) where a real
re-solve hits the network. The fold's median is roughly a seventh of the
driver's (6-7ms) because most merges never reach the driver at all; its
p95 tracks the driver's own, since the slow tail is the same real
conflicts a re-solve either way must pay for.

## 2026-09-26T10:06:29Z

Cap: 200 most recent qualifying merges per repository. viv binary: `target/release/viv` (viv 0.16.0, commit `5b6a5acfd6d9abdab8d45ac01f797292fd522197`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 0 | 25 | 5 | 4 | 3 |

Resolved rung 3 (seeded): 3. 3 merge(s) moved ≥1 package outside the divergent set.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 0 | 25 | 5 | 4 | 3 |

Resolved rung 3 (seeded): 3. 3 merge(s) moved ≥1 package outside the divergent set.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Offline rung vs registry escalation (#314)

| Merges examined | Residue cleared | Residue remaining | Safety number | Network avoided | Median ms, no flag | Median ms, --offline-rung |
|---|---|---|---|---|---|---|
| 26 | 0 | 0 | 0 | 0 | 47.4 | 47.8 |

Residue cleared, by the leaf cause it cleared:

None.

Residue remaining, by leaf cause:

None.

Safety number: 0. No merge where rung 0's pin and the registry's own re-solve, when both finished, chose a different package identity.

## Offline rung (#314), 2026-09-26

`--offline-rung` (`src/lock_merge.rs`): once the registry escalation
(rungs 1-3) has already failed at every rung `--max-scope` allowed, try
one parent's own pinned record for every divergent name -- `ours` first,
then `theirs` -- against a pool built from nothing but the two locks'
`require`/`conflict`/`replace`/`provide`/platform data
(`solver::solve_partial_update` with an empty allow list, so
`pool_builder::build_partial_seeded`'s "everyone locked out, loaded
straight from its own lock entry" path never fetches), reported as its
own rung ("offline_pin") rather than a fourth choice among the three. The
safety number is defined once: a merge where the offline pin was accepted
and a registry rung also finished with a different choice. Trying the
pin first (measured earlier the same day) put it ahead of a registry
answer in 34 client merges, disagreeing with it in 23 of those; moving it
behind the registry escalation is why the count below is zero, by
construction rather than by measurement.

`viv lock merge --offline-rung` on the 355 client merges
(`bench/lockmerge/run.py --offline-rung`, `LOCKMERGE_CAP=200`, matching
the 2026-09-23 client section) plus the 26-merge public corpus, run twice
per merge, without and with the flag.

| Corpus | Merges examined | Residue cleared | Residue remaining | Safety number | Network avoided | Median ms, no flag | Median ms, `--offline-rung` |
|---|---|---|---|---|---|---|---|
| Client (anonymised, four projects) | 355 | 37 | 15 | 0 | 0 | 39.5 | 40.2 |
| Public | 26 | 0 | 0 | 0 | 0 | 48.2 | 47.8 |
| **Total** | **381** | **37** | **15** | **0** | **0** | n/a (medians don't pool across two separately run corpora) | n/a |

37 cleared plus 15 remaining is 52, the client corpus's own "Merges
conflicting (viv lock merge)" count above -- every merge the flag touches
is accounted for. Cleared: 34 `dev-*` heads, 3 packages the registry no
longer lists. Remaining: 10 `dev-*` heads, 4 packages gone, 1 malformed
`composer.json` -- the offline pin cannot read a manifest that doesn't
parse, or turn up a pin neither parent recorded for a name Packagist never
served either parent. Network avoided is zero by construction now: the
offline pin only ever runs once the registry escalation has already
failed, so there is no merge left where escalation also finishes for it
to have skipped. None of the 26 public merges reach the offline pin at
all: the public corpus's own residue is already zero (2026-09-22 section
above), so there is nothing left to clear. Two of project A's 200 client
merges hung past a 90-second watchdog during this replay and were killed
(likely `reqwest`'s own 5-minute request timeout plus retries running long
under real Packagist load, `src/fetch.rs`, not a code defect); the harness
treats a killed subprocess the same as any other crash, silently excluded
from every count above rather than footnoted, a pre-existing gap in
`bench/lockmerge/run.py`'s crash handling this replay surfaced but did not
fix.

**Verdict.** The chapter's own rule holds: the safety number is zero, so
`--offline-rung` never disagrees with a registry answer that also
finishes. Whether to turn it on by default is a separate call for the
maintainer -- this measurement only says the check is safe, not that it
should ship enabled.

## 2026-09-26T13:43:02Z

Cap: 200 most recent qualifying merges per repository. viv binary: `target/release/viv` (viv 0.16.0, commit `37f32fff151e1461ae54a6871f77e91fc390f253`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | n/a | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 0 | 25 | 5 | 4 | 3 |

Resolved rung 3 (seeded): 3. 3 merge(s) moved ≥1 package outside the divergent set.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 0 | 25 | 5 | 4 | 3 |

Resolved rung 3 (seeded): 3. 3 merge(s) moved ≥1 package outside the divergent set.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Offline rung vs registry escalation (#314)

| Merges examined | Residue cleared | Residue remaining | Safety number | Network avoided | Median ms, no flag | Median ms, --offline-rung |
|---|---|---|---|---|---|---|
| 26 | 0 | 0 | 0 | 0 | 48.2 | 47.8 |

Residue cleared, by the leaf cause it cleared:

None.

Residue remaining, by leaf cause:

None.

Safety number: 0. No merge where rung 0's pin and the registry's own re-solve, when both finished, chose a different package identity.

## 2026-09-26T20:16:33Z

Cap: 200 most recent qualifying merges per repository. viv binary: `target/release/viv` (viv 0.17.0, commit `d5bb48c91afe8edfb33fad885373fe184e5366f5`).

### flarum/flarum

Skipped: no committed composer.lock at HEAD.

### monicahq/monica

Examined `no qualifying merge found (no merge had both parents touch composer.lock)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Crashed (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|---|
| 0 | 0 | n/a | 0 | 0 | 0 | n/a | 0 | n/a |

### Resolution archaeology

No merges with real conflicts.

### koel/koel

Examined `0ad670ffff00..fd5f79ee6392 (2 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Crashed (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|---|
| 2 | 0 | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

### Resolution archaeology

No merges with real conflicts.

### pixelfed/pixelfed

Examined `4aa5454067c6..8b6eee19cf85 (24 qualifying merges)`.

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Crashed (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|---|
| 24 | 7 | 4 | 0 | 0 | 25 | 5 | 4 | 3 |

Resolved rung 3 (seeded): 3. 3 merge(s) moved ≥1 package outside the divergent set.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Totals

| Merges examined | Merges conflicting (composer.lock) | Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | Crashed (viv lock merge) | Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | composer.lock conflicted, viv.lock did not |
|---|---|---|---|---|---|---|---|---|
| 26 | 7 | 4 | 0 | 0 | 25 | 5 | 4 | 3 |

Resolved rung 3 (seeded): 3. 3 merge(s) moved ≥1 package outside the divergent set.

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

| Source conflicts (as committed) | Remaining after normalisation | Prevented by normalisation | n/a (normalize failed) |
|---|---|---|---|
| 1 | 1 | 0 | 0 |

No lock-only merge becomes a source conflict after normalisation.

### Offline rung vs registry escalation (#314)

| Merges examined | Residue cleared | Residue remaining | Crashed | Clean | Safety number | Network avoided | Median ms, no flag | Median ms, --offline-rung |
|---|---|---|---|---|---|---|---|---|
| 26 | 0 | 0 | 0 | 26 | 0 | 0 | 48.0 | 47.8 |

Residue cleared, by the leaf cause it cleared:

None.

Residue remaining, by leaf cause:

None.

Crashed, by reason (either the plain or the `--offline-rung` call):

None.

Safety number: 0. No merge where the offline pin (rung 4) and the registry's own re-solve, when both finished, chose a different package identity.

### Offline pin install check (#314 follow-up)

No merge had an `--install-check` result (none of the offline-rung merges in this run reached rung 4, the offline pin).

## Offline pin, does the merged lock install (#314), 2026-09-26

**Question.** Of the merges `--offline-rung` finishes that would otherwise
end in conflict markers, how many produce a lock that actually installs
today, and what does keeping the pin cost?

**Result.** `viv lock merge --offline-rung --install-check`
(`bench/lockmerge/run.py --offline-rung --install-check`) on the same 355
client merges (four anonymised projects, `report16.md`) plus the
26-merge public corpus, both 2026-09-26: for every merge the offline pin
(rung 4) finished, the merged `composer.lock` and `composer.json` are
written into a fresh scratch dir and installed with an empty store
(`viv install --no-scripts --no-plugins --ignore-platform-reqs`); `viv
audit --locked` runs on the merged lock and on each parent's own lock.

| Corpus | Finished by the pin | Installs | Dist gone | Source ref gone | Shasum mismatch | Other | Crashed |
|---|---|---|---|---|---|---|---|
| Client (anonymised, four projects) | 37 | 30 | 0 | 0 | 0 | 7 | 0 |
| Public | 0 | 0 | 0 | 0 | 0 | 0 | 0 |

Advisory count: 10 of the 37 merges keep a pin `viv audit --locked`
flags (14 packages total); in every one of those 14, the side not kept
carried the same advisory, so switching sides would not have avoided any
of them.

Revert count: 13 of the 37 merges keep at least one divergent package
below a version the discarded side had already reached (45 packages
total revert this way).

Sides kept: of the 37, 32 keep `ours`, 5 keep `theirs`; none mix within
a merge, and none resolve to neither parent's own pin.

30 of 37 install cleanly. The 7 that don't all fail the same way: a
downloaded dist archive that isn't a valid zip (`invalid Zip archive:
Could not find EOCD`), all for the same single commercially licensed
package -- consistent with a paywalled plugin whose dist URL no longer
serves an archive without a licence key this replay doesn't have, not a
Packagist or VCS host gone missing. No install crashed or timed out, and
none failed on a dist 404, a moved source reference or a shasum
mismatch in this replay.

**What this means for the default.** The maintainer has decided
`--offline-rung` stays opt-in, not the default. This measurement is what
an opt-in user gets from turning it on: of the 37 merges it finishes, 30
install; the other 7 fail on a licensed plugin's dist archive, a cause
unrelated to the pin itself. No flagged advisory here would have been
avoided by keeping the other side's pin instead, but 13 of the 37 merges
do keep an older version than the discarded side had already reached --
a cost of resolving offline from whichever parent's own history is
closest, not a defect specific to this corpus. The replay resolves these
merges today, often years after they happened: a `dev-*` branch head the
registry now serves differently was usually still current at merge time,
which is most of why 34 of the 37 finished merges were a `dev-*` head at
all (the offline-rung section above). In live use the pin only ever
fires when a branch head has moved or a package has been removed between
the lock and the merge -- both rarer events than replaying years-old
merges against today's Packagist makes them look.
## Generation 3: dev-* as commits

2026-09-28T16:05:40Z


Candidate 3.3 (`docs/research.md`, issue #331): replays only the 52 merges chapter 1's driver (`viv lock merge`) left in conflict (`--only-conflicting`, filtered from a prior full client-corpus replay's own footnotes), not all 355. A `dev-*` record's three-way identity is decided directly off its commit (`source.reference`), never sent to the driver's re-solve; every other record follows the unmodified driver, `--offline` (a cache miss is `needs fetch`, never a live fetch). Cap: 200 most recent qualifying merges per repository, same corpus as chapter 1's. viv binary: `/home/sander/dev/rust/vivace-lanes/main/target/release/viv` (viv 0.18.0, commit `4d2f7659d56bddfd5d91d1c3c165cd1818c59887`). Wall time: 107.6s.

### Rule A: both-moved is always a conflict

A `dev-*` record both sides moved to different commits is a real conflict for a person, full stop -- the rule this run's own `dev_commit_pick` implemented (commit `3d62b8c`).

| Group | Merges | Finished | Real conflict | Other conflict | Needs fetch | Timed out |
|---|---|---|---|---|---|---|
| All | 52 | 0 | 45 | 1 | 6 | 0 |
| dev-* leaf (chapter 1) | 44 | 0 | 40 | 0 | 4 | 0 |
| Other leaf (chapter 1) | 8 | 0 | 5 | 1 | 2 | 0 |

No merge finished, so there is no fetch-cost count: every finished-merge install check this candidate's third measurement asks for is moot on this replay. The install check was not run.

Reasons, per merge:

- project A 149a3202d7fc (real conflict): roave/security-advisories
- project A 16956017309c (real conflict): roave/security-advisories
- project A 1b8532df9e95 (real conflict): humanmade/sc-shared-publish-workflow
- project A 258632533266 (real conflict): roave/security-advisories
- project A 387965e65ee2 (real conflict): roave/security-advisories
- project A 3eb4b805f5ef (real conflict): roave/security-advisories
- project A 44de1678a1cb (real conflict): roave/security-advisories
- project A 671472e243a3 (real conflict): roave/security-advisories
- project A 69de2588d656 (real conflict): roave/security-advisories
- project A 71a4036709e4 (real conflict): roave/security-advisories
- project A 7b15e46a9c1b (real conflict): roave/security-advisories
- project A 7bccadf74a76 (needs fetch): viv lock merge: re-solving nikic/php-parser, symfony/string, symfony/translation against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 7f45843cc838 (real conflict): roave/security-advisories
- project A 805012f95c22 (real conflict): roave/security-advisories
- project A 81257073c74e (real conflict): roave/security-advisories
- project A 819883426171 (real conflict): humanmade/sc-shared-publish-workflow, roave/security-advisories
- project A 81dea67fa970 (real conflict): roave/security-advisories
- project A 84a30ec33622 (needs fetch): viv lock merge: re-solving altis/cloud, altis/cms, altis/dev-tools, altis/dev-tools-command, altis/media, altis/security against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 989c23547fd6 (real conflict): roave/security-advisories
- project A bd7dcaf2b1e3 (real conflict): humanmade/sc-shared-publish-workflow, roave/security-advisories
- project A bfca269167fd (real conflict): roave/security-advisories
- project A c3740b4723e8 (real conflict): humanmade/sc-shared-publish-workflow
- project A c655ae41ef69 (real conflict): roave/security-advisories
- project A c750aa891095 (real conflict): roave/security-advisories
- project A dc9092cb427e (real conflict): roave/security-advisories
- project A e1c8544243d8 (real conflict): humanmade/sc-shared-publish-workflow, roave/security-advisories
- project A efd0d62cc570 (real conflict): roave/security-advisories
- project A f415c1b9489b (other conflict): parsing composer.json: trailing comma at line 208 column 5
- project A fe3911aa9431 (real conflict): roave/security-advisories
- project B 00fcb9d416b8 (real conflict): roave/security-advisories
- project B 2efebd03b513 (real conflict): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B 3697ab0794f6 (real conflict): roave/security-advisories
- project B 3aeb7e197e60 (real conflict): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B 3c4dc0b43482 (real conflict): wikimedia/shiro-wordpress-theme
- project B 47bbe9947f65 (real conflict): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B 4a72f7322508 (real conflict): roave/security-advisories
- project B 9e8946673175 (real conflict): roave/security-advisories
- project B a34613f08896 (real conflict): wikimedia/shiro-wordpress-theme
- project B b7adc8bbe763 (real conflict): wikimedia/shiro-wordpress-theme
- project B bf0ba0d1795f (real conflict): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B d5a9c64dabad (real conflict): roave/security-advisories
- project B e3298da342a3 (real conflict): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B f9efbcc1646f (real conflict): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project C 04e48f571d3d (real conflict): roave/security-advisories
- project C 41dadb89a70f (real conflict): roave/security-advisories
- project C 51bfcf249caa (needs fetch): viv lock merge: re-solving nesbot/carbon against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project C 60a67275d6e3 (real conflict): roave/security-advisories
- project C ba2dcc2ec368 (needs fetch): viv lock merge: re-solving wpackagist-plugin/stream against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project C bd845ad7cb05 (real conflict): unison-theme/unison
- project C cc918961871c (needs fetch): viv lock merge: re-solving roots/wordpress, roots/wordpress-no-content against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project D 5ebff5e0fa5a (needs fetch): viv lock merge: re-solving altis/cms, altis/core, altis/dev-tools, altis/local-server, altis/security, aws/aws-sdk-php against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project D fab5af602563 (real conflict): roave/security-advisories

Corpus, cache and the `--only-conflicting only-conflicting-52.txt` filter (one `<project>\t<sha12>\t<leaf cause>` line per merge, built from a prior full client-corpus replay's own "re-solve did not finish" footnotes) are held outside the repository, same as the client corpus above; reproducible by the maintainer from that clone cache and by nobody else. Reproduce: `LOCKMERGE_CORPUS=<client corpus.toml> BENCH_CACHE=<client clone cache> bench/lockmerge/run.py --dev-as-commits --only-conflicting <path to the filter file>`.

### Rule B: a later `time` wins

2026-09-28T16:16:27Z


Both sides moving a `dev-*` record to different commits compares the two records' own `time` field, taking the later one (`resolved by time`) rather than an automatic conflict; a real conflict is only when either side lacks a `time` or the two tie. Same 52-merge filter, same corpus, same cap as Rule A. viv binary: `/home/sander/dev/rust/vivace-lanes/main/target/release/viv` (viv 0.18.0, commit `3d62b8cb1e1ab56d908aa9d46f34b2e6e7d00ce8`). Wall time: 116.2s.


| Group | Merges | Finished | Resolved by time | Real conflict | Other conflict | Needs fetch | Timed out |
|---|---|---|---|---|---|---|---|
| All | 52 | 0 | 21 | 1 | 1 | 29 | 0 |
| dev-* leaf (chapter 1) | 44 | 0 | 17 | 1 | 0 | 26 | 0 |
| Other leaf (chapter 1) | 8 | 0 | 4 | 0 | 1 | 3 | 0 |

Of the 21 merges that now finish (plain or by the time tie-break), 47 `dev-*` record(s) across them have a commit not in the cached provider data -- the cost an install would pay to fetch it. The install itself was not run (the brief: Packagist's own metadata for an old branch head is gone, so confirming an install would need a real fetch).

Reasons, per merge:

- project A 149a3202d7fc (resolved by time): roave/security-advisories
- project A 16956017309c (resolved by time): roave/security-advisories
- project A 1b8532df9e95 (resolved by time): humanmade/sc-shared-publish-workflow
- project A 258632533266 (resolved by time): roave/security-advisories
- project A 387965e65ee2 (resolved by time): roave/security-advisories
- project A 3eb4b805f5ef (needs fetch): viv lock merge: re-solving lucatume/wp-browser, phpunit/phpunit against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 44de1678a1cb (resolved by time): roave/security-advisories
- project A 671472e243a3 (resolved by time): roave/security-advisories
- project A 69de2588d656 (needs fetch): viv lock merge: re-solving humanmade/smart-media, johnbillion/query-monitor, lucatume/wp-browser, phpunit/phpunit against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 71a4036709e4 (resolved by time): roave/security-advisories
- project A 7b15e46a9c1b (needs fetch): viv lock merge: re-solving aws/aws-sdk-php, humanmade/publication-checklist, phpunit/phpunit against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 7bccadf74a76 (needs fetch): viv lock merge: re-solving nikic/php-parser, symfony/string, symfony/translation against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 7f45843cc838 (needs fetch): viv lock merge: re-solving altis/cloud, altis/cms, altis/core, altis/documentation, altis/local-server, altis/security, aws/aws-sdk-php, carbonphp/carbon-doctrine-types, johnpbloch/wordpress, johnpbloch/wordpress-core, nesbot/carbon, phpunit/phpunit, symfony/string against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 805012f95c22 (needs fetch): viv lock merge: re-solving symfony/process, symfony/string against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 81257073c74e (needs fetch): viv lock merge: re-solving altis/cloud, altis/local-server, behat/gherkin, league/uri, league/uri-interfaces, lucatume/wp-browser, nikic/php-parser, psy/psysh, symfony/finder against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 819883426171 (resolved by time): humanmade/sc-shared-publish-workflow, roave/security-advisories
- project A 81dea67fa970 (needs fetch): viv lock merge: re-solving illuminate/collections, illuminate/conditionable, illuminate/contracts, illuminate/macroable, illuminate/support against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 84a30ec33622 (needs fetch): viv lock merge: re-solving altis/cloud, altis/cms, altis/dev-tools, altis/dev-tools-command, altis/media, altis/security against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A 989c23547fd6 (needs fetch): viv lock merge: re-solving symfony/string against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A bd7dcaf2b1e3 (needs fetch): viv lock merge: re-solving illuminate/collections, illuminate/conditionable, illuminate/contracts, illuminate/macroable, illuminate/support, johnbillion/user-switching, symfony/deprecation-contracts, symfony/service-contracts, symfony/translation-contracts, voku/portable-ascii against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A bfca269167fd (needs fetch): viv lock merge: re-solving illuminate/collections, illuminate/conditionable, illuminate/contracts, illuminate/macroable, illuminate/support against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A c3740b4723e8 (resolved by time): humanmade/sc-shared-publish-workflow
- project A c655ae41ef69 (resolved by time): roave/security-advisories
- project A c750aa891095 (resolved by time): roave/security-advisories
- project A dc9092cb427e (needs fetch): viv lock merge: re-solving altis/cms, altis/core, altis/dev-tools, altis/local-server, altis/media, altis/security, darylldoyle/safe-svg, guzzlehttp/guzzle, humanmade/sc-user-reports, psr/log, symfony/string, symfony/translation against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A e1c8544243d8 (needs fetch): viv lock merge: re-solving altis/cloud against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project A efd0d62cc570 (resolved by time): roave/security-advisories
- project A f415c1b9489b (other conflict): parsing composer.json: trailing comma at line 208 column 5
- project A fe3911aa9431 (needs fetch): viv lock merge: re-solving altis/local-server, composer/pcre, guzzlehttp/guzzle, mck89/peast, symfony/console, symfony/string, wp-cli/config-command, wp-cli/i18n-command against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project B 00fcb9d416b8 (needs fetch): viv lock merge: re-solving alleyinteractive/wordpress-fieldmanager, composer/installers, dealerdirect/phpcodesniffer-composer-installer, squizlabs/php_codesniffer, wpackagist-plugin/co-authors-plus, wpackagist-plugin/safe-redirect-manager, wpackagist-plugin/safe-svg against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project B 2efebd03b513 (resolved by time): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B 3697ab0794f6 (needs fetch): viv lock merge: re-solving squizlabs/php_codesniffer, wpackagist-plugin/co-authors-plus against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project B 3aeb7e197e60 (resolved by time): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B 3c4dc0b43482 (resolved by time): wikimedia/shiro-wordpress-theme
- project B 47bbe9947f65 (resolved by time): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B 4a72f7322508 (resolved by time): roave/security-advisories
- project B 9e8946673175 (needs fetch): viv lock merge: re-solving wpackagist-plugin/safe-svg against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project B a34613f08896 (resolved by time): wikimedia/shiro-wordpress-theme
- project B b7adc8bbe763 (resolved by time): wikimedia/shiro-wordpress-theme
- project B bf0ba0d1795f (needs fetch): viv lock merge: re-solving humanmade/asset-loader, wpackagist-plugin/safe-redirect-manager against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project B d5a9c64dabad (resolved by time): roave/security-advisories
- project B e3298da342a3 (needs fetch): viv lock merge: re-solving wpackagist-plugin/wordpress-seo against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project B f9efbcc1646f (needs fetch): viv lock merge: re-solving wpackagist-plugin/broken-link-checker, wpackagist-plugin/co-authors-plus, wpackagist-plugin/wordpress-seo against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project C 04e48f571d3d (needs fetch): viv lock merge: re-solving justinrainbow/json-schema against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project C 41dadb89a70f (needs fetch): viv lock merge: re-solving illuminate/collections, illuminate/conditionable, illuminate/contracts, illuminate/macroable, illuminate/support, nesbot/carbon, symfony/filesystem, symfony/polyfill-php83, symfony/process, symfony/string, symfony/translation against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project C 51bfcf249caa (needs fetch): viv lock merge: re-solving nesbot/carbon against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project C 60a67275d6e3 (needs fetch): viv lock merge: re-solving humanmade-pro/gravityforms, justinrainbow/json-schema, squizlabs/php_codesniffer, wpackagist-plugin/jetpack, wpackagist-plugin/redis-cache, wpackagist-plugin/wp-accessibility against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project C ba2dcc2ec368 (needs fetch): viv lock merge: re-solving wpackagist-plugin/stream against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project C bd845ad7cb05 (real conflict): unison-theme/unison
- project C cc918961871c (needs fetch): viv lock merge: re-solving roots/wordpress, roots/wordpress-no-content against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project D 5ebff5e0fa5a (needs fetch): viv lock merge: re-solving altis/cms, altis/core, altis/dev-tools, altis/local-server, altis/security, aws/aws-sdk-php against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers
- project D fab5af602563 (needs fetch): viv lock merge: re-solving altis/aws-analytics, altis/local-chassis, altis/local-server, altis/media, aws/aws-sdk-php, humanmade/publication-checklist, humanmade/s3-uploads, humanmade/smart-media, humanmade/workflows, symfony/polyfill-mbstring, symfony/yaml against the merged composer.json did not finish (https://wpackagist.org/packages.json: Network disabled, request canceled: https://wpackagist.org/packages.json); falling back to conflict markers

Reproduce Rule B: `LOCKMERGE_CORPUS=<client corpus.toml> BENCH_CACHE=<client clone cache> bench/lockmerge/run.py --dev-as-commits --rule-b --only-conflicting <path to the filter file>` (same filter file as Rule A).

**Reading.** Of the 52 merges Rule A left entirely unresolved (0 finished), Rule B's time tie-break resolves the `dev-*` side of 21 (all `resolved by time`, none finish outright) and leaves 1 real conflict (no `time` field or a tie) and 1 unrelated malformed-manifest conflict; the other 29 need a registry fetch this replay's offline cache doesn't have (`needs fetch`), well past Rule A's 6, because letting more merges reach the residual driver call also grows how much it needs to solve. Of the 21 that now finish, 47 `dev-*` records across them still have a commit not in the cached provider data -- the install-time fetch cost Rule A's zero finishes made moot.

### Rule B, online

2026-09-29T08:42:51Z


Exactly the 29 of Rule B's 52-merge filter that came back `needs fetch` offline (`--only-conflicting only-conflicting-29.txt`, filtered from Rule B's own footnotes above), replayed with `--online` (drops `--offline` on the residual driver call; `--cache-dir` is a scratch copy of the metadata cache, so the committed offline Rule A/B results stay reproducible from the untouched default store). Same `timeout 300` per merge, 4 in parallel, same corpus and cap as Rule A/B. viv binary: `/home/sander/dev/rust/vivace-lanes/main/target/release/viv` (viv 0.18.0, commit `d6e572d04906c6850d15cafed34d23a4f8dfd223`). Wall time: 146.3s.


| Group | Merges | Finished | Resolved by time | Real conflict | Other conflict | Needs fetch | Timed out |
|---|---|---|---|---|---|---|---|
| All | 29 | 1 | 15 | 0 | 13 | 0 | 0 |
| dev-* leaf (chapter 1) | 26 | 1 | 15 | 0 | 10 | 0 | 0 |
| Other leaf (chapter 1) | 3 | 0 | 0 | 0 | 3 | 0 | 0 |

Of the 16 merges that now finish (plain or by the time tie-break), 24 `dev-*` record(s) across them have a commit not in the cached provider data -- the cost an install would pay to fetch it. The install itself was not run (the brief: Packagist's own metadata for an old branch head is gone, so confirming an install would need a real fetch).

Reasons, per merge:

- project A 3eb4b805f5ef (resolved by time): roave/security-advisories
- project A 69de2588d656 (resolved by time): roave/security-advisories
- project A 7b15e46a9c1b (resolved by time): roave/security-advisories
- project A 7bccadf74a76 (other conflict): viv lock merge: re-solving nikic/php-parser, symfony/string, symfony/translation against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/wordpress-seo$d9169785af2851d8b57fe4c7eb440b0011b5e0ba314ab834f6ab891d8b93b12c.json: not found); falling back to conflict markers
- project A 7f45843cc838 (resolved by time): roave/security-advisories
- project A 805012f95c22 (resolved by time): roave/security-advisories
- project A 81257073c74e (resolved by time): roave/security-advisories
- project A 81dea67fa970 (resolved by time): roave/security-advisories
- project A 989c23547fd6 (resolved by time): roave/security-advisories
- project A bd7dcaf2b1e3 (resolved by time): humanmade/sc-shared-publish-workflow, roave/security-advisories
- project A bfca269167fd (resolved by time): roave/security-advisories
- project A dc9092cb427e (other conflict): viv lock merge: re-solving altis/cms, altis/core, altis/dev-tools, altis/local-server, altis/media, altis/security, darylldoyle/safe-svg, guzzlehttp/guzzle, humanmade/sc-user-reports, psr/log, symfony/string, symfony/translation against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/wordpress-seo$d9169785af2851d8b57fe4c7eb440b0011b5e0ba314ab834f6ab891d8b93b12c.json: not found); falling back to conflict markers
- project A e1c8544243d8 (resolved by time): humanmade/sc-shared-publish-workflow, roave/security-advisories
- project A fe3911aa9431 (resolved by time): roave/security-advisories
- project B 00fcb9d416b8 (other conflict): viv lock merge: re-solving alleyinteractive/wordpress-fieldmanager, composer/installers, dealerdirect/phpcodesniffer-composer-installer, squizlabs/php_codesniffer, wpackagist-plugin/co-authors-plus, wpackagist-plugin/safe-redirect-manager, wpackagist-plugin/safe-svg against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/safe-svg$16d95c88df413a8014bb0727cf5325477fa593f3c658b41c32e2850f985dc3f4.json: not found); falling back to conflict markers
- project B 3697ab0794f6 (resolved by time): roave/security-advisories
- project B 9e8946673175 (other conflict): viv lock merge: re-solving wpackagist-plugin/safe-svg against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/safe-svg$16d95c88df413a8014bb0727cf5325477fa593f3c658b41c32e2850f985dc3f4.json: not found); falling back to conflict markers
- project B bf0ba0d1795f (resolved by time): roave/security-advisories, wikimedia/shiro-wordpress-theme
- project B e3298da342a3 (other conflict): viv lock merge: re-solving wpackagist-plugin/wordpress-seo against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/wordpress-seo$d9169785af2851d8b57fe4c7eb440b0011b5e0ba314ab834f6ab891d8b93b12c.json: not found); falling back to conflict markers
- project B f9efbcc1646f (other conflict): viv lock merge: re-solving wpackagist-plugin/broken-link-checker, wpackagist-plugin/co-authors-plus, wpackagist-plugin/wordpress-seo against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/wordpress-seo$d9169785af2851d8b57fe4c7eb440b0011b5e0ba314ab834f6ab891d8b93b12c.json: not found); falling back to conflict markers
- project C 04e48f571d3d (resolved by time): roave/security-advisories
- project C 41dadb89a70f (other conflict): viv lock merge: re-solving illuminate/collections, illuminate/conditionable, illuminate/contracts, illuminate/macroable, illuminate/support, nesbot/carbon, symfony/filesystem, symfony/polyfill-php83, symfony/process, symfony/string, symfony/translation against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/wp-accessibility$6c625248efd0131b6f37cebda9e1a1cd372790b7a097416a2bdda9c677dd0528.json: not found); falling back to conflict markers
- project C 51bfcf249caa (other conflict): viv lock merge: re-solving nesbot/carbon against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/two-factor$e6594eb1e8d37893c96e190aa778d4b721c555b0420cb14a5bafef6c751749ed.json: not found); falling back to conflict markers
- project C 60a67275d6e3 (other conflict): viv lock merge: re-solving humanmade-pro/gravityforms, justinrainbow/json-schema, squizlabs/php_codesniffer, wpackagist-plugin/jetpack, wpackagist-plugin/redis-cache, wpackagist-plugin/wp-accessibility against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/wp-accessibility$6c625248efd0131b6f37cebda9e1a1cd372790b7a097416a2bdda9c677dd0528.json: not found); falling back to conflict markers
- project C ba2dcc2ec368 (other conflict): viv lock merge: re-solving wpackagist-plugin/stream against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/restricted-site-access$3a9620944609fb5f4389f5367dbaff3a39fb6b96593f787d2ecb9bc7848d517d.json: not found); falling back to conflict markers
- project C cc918961871c (other conflict): viv lock merge: re-solving roots/wordpress, roots/wordpress-no-content against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/wordpress-seo$d9169785af2851d8b57fe4c7eb440b0011b5e0ba314ab834f6ab891d8b93b12c.json: not found); falling back to conflict markers
- project D 5ebff5e0fa5a (other conflict): viv lock merge: re-solving altis/cms, altis/core, altis/dev-tools, altis/local-server, altis/security, aws/aws-sdk-php against the merged composer.json did not finish (re-solving at rung 3 (seeded) did not finish; the registry may be unreachable (cached metadata was tried first via --cache-dir): https://wpackagist.org/p/wpackagist-plugin/wordpress-seo$d9169785af2851d8b57fe4c7eb440b0011b5e0ba314ab834f6ab891d8b93b12c.json: not found); falling back to conflict markers
- project D fab5af602563 (other conflict): - Root composer.json requires wpackagist-plugin/content-control, it could not be found in any version, there may be a typo in the package name.

#### Rule B, all 52, with network


| Merges | Finished | Resolved by time | Real conflict | Other conflict | Needs fetch | Timed out |
|---|---|---|---|---|---|---|
| 52 | 1 | 36 | 1 | 14 | 0 | 0 |

The 23 non-`needs fetch` merges are Rule B's own committed counts above, unchanged (this run never replays them); the other 29 are this run's own outcomes, just above.

Install check (the issue's third number, cheap form: does the merged lock accept `viv install --dry-run` cleanly from the now-warm cache, no further network): skipped. Dev-as-commits mode never rejoins a `dev-*` pick with the residual driver's own merged lock into one composer.lock -- `resolve_offline_stripped` only ever writes the dev-stripped residue back over `ours.lock`, so there is no single merged-lock artifact for a finished merge to install-check without new merge-writing logic this replay doesn't have.

Reproduce: `LOCKMERGE_CORPUS=<client corpus.toml> BENCH_CACHE=<scratch copy of the client clone cache> bench/lockmerge/run.py --dev-as-commits --rule-b --online --only-conflicting <path to only-conflicting-29.txt>` (built from Rule B's own 29 "needs fetch" reasons).


**Reading.** Of the 29 merges Rule B left `needs fetch` offline, going online resolves 16 (1 finished outright, 15 by the time tie-break), leaves 0 real conflict and 13 other conflict, and 0 still `needs fetch` even with network on. Combined with Rule B's other 23, all 52 under Rule B with network now stand at 1 finished, 36 resolved by time, 1 real conflict, 14 other conflict, 0 needs fetch, 0 timed out.

### Shipped driver, 2026-09-29

The four build steps of candidate 3.3 (#343 dev-* records identified by
commit, #347 `viv.lock` records carry `time`, #344 `composer.lock` from
`viv.lock`, #345 fetching a pinned commit the registry no longer
describes) landed on `main` at `879fddc`. The same 52 merges, run with
the shipped `viv lock merge` and no research model in the loop: a
release build, each merge at `--as-of` its own commit date, the platform
declared per merge as `run.py`'s `driver_conflict()` does, a cold copy of
the metadata cache, network and git reachable, `timeout 300`, four in
parallel, 231 s.

| Pass | Finished | Conflict | Timed out | `fetching` lines |
|---|---|---|---|---|
| 1, cold cache | 46 | 6 | 0 | 22 |
| 2, same cache | 46 | 6 | 0 | 1 |

The research model (rule B, online) had 37 finished, 1 real conflict and
14 registry residue. The driver does better because #343 settles a
`dev-*` name from the two records' own commits and never asks the
registry for it, where the model stripped those names and sent the rest
to a solve that still wanted provider files wpackagist no longer serves.

The six that remain, by project letter and merge: three where a
wpackagist plugin no longer exists in any version (C 51bfcf249caa, D
5ebff5e0fa5a, D fab5af602563), one where the pinned commit is gone from
its upstream repository (C 41dadb89a70f; fetched again on pass 2, #348),
one malformed `composer.json` with a trailing comma (A f415c1b9489b), and
one that ends in conflict markers with nothing on stderr (C bd845ad7cb05),
not classified. Chapter 1's floor of 51 unfinished merges in 355 is now
6, 1.7%. Two harness findings became issues: #348 and #349 (a git-cache
race under four parallel merges).

Reproduce: the scratch scripts are not in the repository; `run.py` gains
a `--driver-only` mode in the next tooling change, and this section is
the reference until then.
