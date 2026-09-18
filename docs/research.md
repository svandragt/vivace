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

**Format sketch.** One record per package: name, version, dist URL and
hash, source ref, dev flag, and the root requirement that selected it.
Staleness is per record (the recorded requirement no longer matches
`composer.json`) rather than a file-wide hash. TOML blocks or one JSON
object per line; blocks read better in diffs.

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
`installed.json` and `InstalledVersions.php` shims that frameworks read,
and classes load straight from the shared store, what happens to install
time, disk use, and framework compatibility?

Not started. Depends on nothing in chapter 1. The two shims decide whether
real projects keep working, so the measurement is the compat corpus booting,
not only the install time.

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
  a cache.
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
