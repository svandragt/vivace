# Research programme

vivace started as one question: can a person directing coding agents build
a faster, byte-compatible Composer? By 0.13 the answer is yes. The
[compat sweep](../compat/results/) finds an identical `vendor/` on every
corpus project, and the [bench corpus](../bench/results/corpus.md) puts a
cold Laravel install at a fifth of Composer's time. That work is finished
and the mode that does it, called compat mode below, is frozen as the
control for what follows.

viv is now a research vehicle for package-manager design. Each chapter asks
one question, states a hypothesis, keeps compat mode as the control,
measures, and ends in a write-up. Behaviour a chapter adds sits behind a flag
or a manifest setting so the default path stays byte-compatible, and
`composer.lock` stays exportable so viv never becomes a fork of itself.

## How chapters run

- The question and hypothesis are written here before code.
- The compat sweep and the bench gates run on every change, including
  research ones. A chapter that breaks compat mode is not finished.
- Measurements go in `bench/results/`, the way `profile.md` records the
  performance work.
- The write-up is the deliverable. A chapter that ends in "measured, not
  worth it" is a complete chapter.

Compat-fidelity items are worked only when a real user hits one; they sit in
the `compat: on demand` milestone.

The CI and bench harness the chapters run on is not itself a chapter: it
asks no question and measures nothing of its own. That work sits in the
`research tooling` milestone, and a chapter blocked on it says so in its
**Work** list.

## Chapter 1: a conflict-less lock

**Question.** Can a lock file be designed so that git merges of two branches
that both changed dependencies almost never conflict, and so the tool can
resolve the remaining conflicts itself?

**Why `composer.lock` conflicts.** Three structural properties:

- Aggregate fields. `content-hash`, `plugin-api-version` and `platform`
  are single lines that change on every requirements edit, so two branches
  that touched `composer.json` always collide there.
- Positional structure. Packages are multi-line objects in a
  comma-terminated JSON array. Additions that sort next to each other share
  lines; additions at the end fight over the trailing comma.
- Two lists. `packages` and `packages-dev` make a dev move a delete in one
  place and an insert in another.

**Hypothesis.** A lock of self-contained, name-sorted records with no
aggregate fields removes most textual conflicts. The residue (adjacent
additions, both sides upgrading the same package) is small enough for the
tool to resolve by taking the union of records and re-solving only the
divergent names, either on `install` when it sees conflict markers or in a
git merge driver.

**Format.** `viv update --lock native` writes `viv.lock` beside
`composer.lock`, unchanged, from the same resolution (`src/native_lock.rs`).
A TOML document holding one `[[package]]` block per resolved package,
sorted by name. Each record holds exactly:

- `name` and `version` (pretty, as the resolver picked it).
- `dist-url` and `dist-hash`, when the resolution has them (`dist-hash` is
  omitted, not written empty, when the provider file's own `shasum` is
  blank).
- `source-ref`, when the resolution has a source entry.
- `dev`, the record's own `true`/`false`: this is what the two-list
  `packages`/`packages-dev` split in `composer.lock` carries instead, so a
  dev move is one field flip, not a delete in one list and an insert in
  another.
- `root-requirement`, the constraint string root `composer.json`'s own
  `require`/`require-dev` states for that package's name, omitted when
  absent. This is the only honest source for "the root requirement that
  selected it": the solver keeps no per-package reason trail
  (`docs/resolver-design.md`), so a transitively pulled package — never
  named by root — carries no `root-requirement` field, rather than one
  guessed at.

Deliberately absent: `content-hash`, `plugin-api-version` and `platform`,
the file-level fields that change on every requirements edit and are why
this chapter exists; and the `packages`/`packages-dev` split, which `dev`
already carries. Staleness is per record — a record is stale when its
`root-requirement` no longer matches what `composer.json` currently states
for that name — rather than the file-wide `content-hash`; the staleness
check itself is later work (#274), not this one.

This chunk is the writer only. Reading `viv.lock` back into `install`/
`update` is separate work, and so is the merge behaviour this format is
meant to earn (`Measurement` below).

**Control.** `composer.lock` as Composer 2.10 writes it, produced by the
same resolution.

**Measurement.** Replay every historical merge commit in a corpus of
public applications with long `composer.lock` histories, with the lock
translated into the new format, and count textual and real conflicts under
both. Results land in `bench/results/lockmerge.md`.

**Work.** Format and writer (#272), merge replay harness (#273), marker
tolerance in `install` (#274), merge driver (#275). The format comes first:
it should remove most conflicts on its own and is a small change on top of
the existing lock writer.

## Chapter 2: autoload from the store, no vendor tree (planned)

**Question.** If `vendor/` is a generated autoloader plus the
`installed.json` and `installed.php` shims that frameworks read, and
classes load straight from the shared store, what happens to install time,
disk use, and framework compatibility?

**Why Composer needs a tree.** `vendor/` is not only where files land.
Four things resolve by path into it:

- PSR-4 and PSR-0 prefixes map a namespace to a directory, so every prefix
  needs a directory whose layout matches the namespace.
- `autoload.files` entries are `require`d at boot by path.
- A package's own relative includes and `__DIR__` walks assume the package
  sits at the depth Composer put it at. `__DIR__ . '/../../autoload.php'`
  is the standard way a vendored script finds the autoloader.
- `vendor/bin` proxies resolve their target relative to themselves.

Only the first two are the autoloader's own business, and viv already
writes both as generated path strings (`$vendorDir . '/...'` in the plain
maps, a baked absolute path in `autoload_static.php`). Pointing them at
`archive-v0/<hash>/` instead of `vendor/<name>/` is a change of value, not
of format. The last two are the risk.

**What the prize actually is.** The 2026-09-18 phase split
(`bench/results/profile.md` §9) bounds it before any code is written:

- Warm install is 43 ms, of which `link_tree` is 19 ms — 7746 `linkat`
  calls, 42.5% of syscall time. This is the prize, and it is warm-install
  only.
- Cold install is 2.11 s, of which linking is 33 ms. Fetch is 95%. A
  chapter that removes the tree does not make a cold install measurably
  faster, and should not claim to.
- Disk is the counter-intuitive one. Under the default hardlink mode
  `vendor/` already shares inodes with the store, so it costs directory
  entries and inodes, not data blocks. Real duplication only exists under
  `--link-mode copy` and on filesystems that cannot hardlink. The disk
  claim has to be measured as inode and dentry count, not bytes, or it
  will look like a win it isn't.

**Hypothesis.** A project can boot from a `vendor/` that holds only the
generated autoloader, the two shims and `bin` proxies, with every package
path pointing into the store, and this removes the link phase from a warm
install (19 of 43 ms) without breaking the compat corpus. The failures,
where they happen, are concentrated in packages that walk out of their own
directory expecting to land in the project, not in class loading.

**Design sketch.** Behind a flag, so compat mode is untouched. The
generated maps take store paths; `autoload_static.php` already bakes an
absolute path at generation time, so it needs no new mechanism.
`installed.json` and `installed.php` carry `install-path` relative to
`vendor/composer`, so a store path makes them long relative paths or
absolute ones — which form frameworks tolerate is a finding, not a
decision to take up front. `vendor/bin` stays a real directory of
proxies, since the depth a proxy resolves from is exactly what the store
breaks.

One consequence is structural rather than cosmetic: the store stops being
a cache. Today `prune` may delete an archive while a project keeps
working, because the hardlinked inode survives. With no tree, pruning an
archive breaks a live project, so the store needs to know which archives
a project loads from. That reference-keeping is part of the chapter, not
an afterthought.

**Control.** Compat mode on the same lock and the same warm store: the
linked `vendor/` tree, its phase split already recorded in
`bench/results/profile.md`.

**Measurement.** Per corpus project: warm and cold install wall time,
`linkat` count under `strace -c -f`, inode and directory-entry count of
`vendor/`, and — the one that decides the chapter — whether the project
boots. Booting means the compat corpus running its own test suite or
console entry point, not a successful install. A project that installs
and cannot boot is the result. Results land in
`bench/results/storeload.md`.

**Work.** Store-path autoload maps behind a flag (#284), the minimum
`vendor/` with `bin` proxies and the two shims (#285), store references
so `prune` cannot delete a live archive (#286), corpus boot harness and
measurement (#287). Depends on nothing in chapter 1. The boot harness is
worth building first: it decides whether the rest is worth writing.

## Chapter 3: workspaces (planned)

**Question.** When a repository holds several `composer.json` files, can
one lock and one solve serve all of them, so every member agrees on one
version of each dependency and the install costs the union of the
members, not the sum?

**Why Composer has no answer.** A Composer root is one `composer.json`
next to one `composer.lock` and one `vendor/`. A repository with several
roots, such as a platform with a plugin and a theme per directory or a
framework split into components, gets one of two workarounds:

- Independent roots. Each member keeps its own lock and `vendor/`. The
  same package is solved, downloaded and extracted once per member, and
  the members drift to different versions of it without anyone choosing
  to.
- One aggregate root. A top-level `composer.json` requires every member
  through a `path` repository with `symlink: true`. Members lose their own
  lock and cannot be installed alone, and the aggregate lock conflicts in
  git exactly as chapter 1 describes.

npm, pnpm, Cargo and uv solve this with a workspace: the root names its
members, one lock covers them, and a member that requires another member
gets it from the tree rather than a registry.

**Hypothesis.** A root that lists its members, one solve over the union
of their requirements, and one lock in chapter 1's format (each record
already carries the requirement that selected it, so it can carry the
member too) gives three measurable results against independent roots:

- zero version drift, because there is one resolved version per package
- install time and store size that grow with the union of the
  dependencies, not with the number of members
- each member still installable alone, with its own `vendor/` and
  autoloader against the shared lock

**Design sketch.** Members are declared in the root `composer.json` under
`extra.viv.workspace.members` as globs, so Composer ignores the setting
and compat mode is untouched. A member that requires another member by
name gets a symlink into the tree, the way `workspace:*` works in pnpm.
Each member gets a `vendor/` linked from the store and its own generated
autoloader, so `php members/x/bin/tool` works without the root. The
shared lock lives at the root. `composer.lock` is still exportable per
member, projected from the shared lock, so a member can leave the
workspace.

**Control.** Compat mode run once per member: N independent installs
against the same store, each writing its own `composer.lock`.

**Measurement.** A corpus of public repositories with several
`composer.json` files that are installable roots. Per repository: members
found, packages in the union versus the sum, distinct versions of the
same package across independent member solves (the drift count), warm
and cold install time for N installs versus one workspace install, and
store size. Results land in `bench/results/workspaces.md`.

**Work.** Member discovery and inter-member linking (#276), one solve and
one lock over the union (#277), per-member `vendor/` and `composer.lock`
projection (#278), corpus and measurement harness (#279). Depends on
chapter 1's record format (#272) for the lock; the discovery and solve
work does not wait for it.

## Candidate chapters

Opportunities that open once the contract is "a working project managed by
viv" rather than byte-identical output. Each is a chapter waiting for a
question and a measurement; none is scheduled.

- **Autoload as a store property.** Each immutable package version in the
  store carries its precomputed classmap and PSR map, so install is a
  concatenation and only the root package is ever scanned. The performance
  finding behind it (#269, the `-o` root scan) becomes a design rather than
  a cache. Separable from chapter 2: that chapter changes where the paths
  point, this one changes who computes the map, and either works without
  the other.
- **A lock that describes the whole install.** Chapter 1's format extended
  with content-addressed hashes of the extracted trees, the full resolved
  graph and a platform snapshot, so an offline install needs no mirror and
  reproducibility is checked against the trees, not the dist URLs.
- **Lock-seeded solving by default.** Start the pool from the locked
  versions and widen only on conflict, so an `update` that changes little
  costs little. Measured against the closure and solve phases the profile
  already logs.
- **A declared extension model.** The native adapters show that most
  Composer plugins are path mapping, file scaffolding and patching. A
  manifest that declares those directly replaces emulating plugin execution,
  and the adapter list becomes the migration table.
