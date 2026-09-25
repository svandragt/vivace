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
