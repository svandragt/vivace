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

## Chapter 1: a conflict-less lock (format and driver measured, hold)

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

`viv.lock` is a companion to `composer.lock`, never a replacement — a record
carries no `require`/autoload/metadata, which `installed.json`/
`installed.php` need — so `install`/`update` (#297) read both and refuse,
naming every package, when a record's `(version, source-ref, dev)` disagrees
with `composer.lock`'s own entry for that name, rather than installing from
one side alone.

Deliberately absent: `content-hash`, `plugin-api-version` and `platform`,
the file-level fields that change on every requirements edit and are why
this chapter exists; and the `packages`/`packages-dev` split, which `dev`
already carries. Staleness is per record — a record is stale when its
`root-requirement` no longer matches what `composer.json` currently states
for that name — rather than the file-wide `content-hash`; the staleness
check itself is later work (#274), not this one.

**What staleness cannot yet answer.** Only records root names directly
carry a `root-requirement`, so only those can be judged stale this way. On
`bench/laravel` that is roughly 30 of 109 resolved packages; the rest say
nothing about their own freshness. Since #274 and #275 both re-solve "only
the divergent names", the rule as it stands can reason about the minority.
Closing that needs either a provenance trail through the solver or a
derived rule for transitive records, and which one is worth it should be
decided from #273's conflict counts rather than up front (#290).

This chunk is the writer and, since #297, the reader: `install`/`update`
accept `viv.lock` as a companion to `composer.lock`. The merge behaviour
this format is meant to earn is `Measurement` below.

**Control.** `composer.lock` as Composer 2.10 writes it, produced by the
same resolution.

**Measurement.** Replay every historical merge commit in a corpus of
public applications with long `composer.lock` histories, with the lock
translated into the new format, and count textual and real conflicts under
both. Results land in `bench/results/lockmerge.md`.

**Result, 2026-09-22.** The format half holds. Across 379 merges in five
repositories where both sides changed dependencies
(`bench/results/lockmerge.md`: one public project, four client projects
anonymised), `composer.lock` conflicted on 235 merges, 62%. `viv.lock`
conflicted on 96, 25%. Conflict hunks fell from 1911 to 297. Real
conflicts, both sides changing the same package to different results, were
270, and in every repository `viv.lock`'s hunks sit within 10% of that
count: what the format leaves is what no format can merge. 139 merges that
someone resolved by hand at the time would have merged clean.

That sizes the second half, and the archaeology on those 96 merges
(`bench/results/lockmerge.md`, "Resolution archaeology") says what it is.
Ten conflict only in the record format's adjacency, no package changed on
both sides. Sixty-five are real conflicts with a `composer.json` that
merged clean: the lock diverged, the source agreed, and re-solving the
divergent names against the merged manifest re-derives the answer.
Seventeen are source conflicts, both sides editing the same constraint,
which no lock format touches; normalising `composer.json` on all three
sides first prevents two of them and never creates one. So the driver
(#275) can reach 15 of 355 merges, 4.2%, from `composer.lock`'s 62%.

The human resolutions rule out a heuristic. Among orderable picks people
took the higher version 75% of the time overall but 47% and 52% in two of
the four projects, and 8% of the time chose a version neither side had.
There was no rule; re-solving is the rule. Resolutions dragged a median of
0 and a maximum of 14 other packages along, so the driver re-solves the
closure of the divergent names, not the names alone. The format's win needed no
staleness rule at all, so the coverage gap in #290 does not touch this
result; it matters only for how the re-solve decides what is divergent.

**Result, driver, 2026-09-22.** `viv lock merge` (#275) merges by record
and re-solves the divergent closure against the merged `composer.json`,
falling back to markers on those names only when the solve cannot finish.
On the same 355 client merges: `composer.lock` 228 conflicting, `viv.lock`
92, record merge 88, record merge with re-solve **61**, 17%; with inline `package`
repositories solvable (#294), **52**, 15%; with the three escalation rungs
(#296), **51**, 14%. On the public corpus the driver finishes every real
conflict, one at rung 1 and two at rung 3.

The 51 are the replay's floor rather than the driver's. Forty-three are
`dev-*` requirements, 28 of them `roave/security-advisories dev-latest`.
A tagged release is one immutable Packagist entry; a branch is one entry
the registry overwrites on every push, so the solver only ever sees
today's head. In 28 merges today's head declares
`conflict: wp-coding-standards/wpcs <3` while the merged manifest still
requires `wpcs ^2.3`, and the solver reports those two names:

```
roave/security-advisories dev-latest conflicts with wp-coding-standards/wpcs 2.3.0
```

The head at commit time is not lost, each side's `composer.lock` records
its `source.reference`, and that commit's `composer.json` is still in the
package's git repository. Only Packagist's copy is gone. A replay could
recover it by fetching that commit and feeding its metadata to the solver
in place of the registry entry, the path Composer takes for `type: vcs`
repositories. That is not built, because it would only improve the
measurement. For a merge happening today the head as served is the right
input: `wpcs ^2.3` with today's `dev-latest` is unsatisfiable, so
`viv lock merge` writes markers for those two names and exits 1, and
`composer update` fails with the same message. The failure is the correct
one. The advisories package is saying wpcs 2.3.0 has an advisory, and the
fix is a `composer.json` change, wpcs to 3.x. That is the chapter's
principle holding: the lock has nothing left to decide, the remaining
human decision lives in `composer.json`, and viv names the two packages
it concerns.

Seven merges require a package Packagist no longer lists, one manifest is
malformed. The constraint chains and platform-heuristic cases of the
earlier run are gone: rung 3 either finishes them or pushes them to a
`dev-*` leaf. `--as-of`, resolving against the registry as it stood at
the commit, changed nothing, which is the confirmation: the residue is
branches, not releases. Excluding what a replay cannot reproduce, the
driver-attributable residue is zero of 355 (`bench/results/lockmerge.md`,
2026-09-23 section).

So, for a project viv can solve: the format alone takes a dependency
merge from a 62% chance of conflict to 25%, and the driver takes it to a
few percent, with what remains being the conflicts no tool can decide.

**Work.** Format and writer (#272), merge replay harness (#273), marker
tolerance in `install` (#274), merge driver (#275). Done: #272, #273,
#274, #275, #294, #295, #297, #298, #299; #290 closed as superseded, the
driver decides divergence by three-way identity and never reads staleness.
#274 refuses `install` when `composer.lock` or `viv.lock` still carries
git conflict markers, naming the file, the first marker's line and the
packages in conflict, and pointing at `viv lock merge`, instead of the raw
parse error a leftover marker used to produce. #295 gave `viv.lock`'s re-solve
its pinned set's requires from the sibling `composer.lock` a record never
carries, so the format reaches the driver's same closure re-solve
`composer.lock` gets, falling back to markers (with the reason) when that
sibling file is missing. #296 escalates the re-solve in three rungs, the
divergent closure, then pinned packages that directly require it, then a
full solve seeded with every non-divergent locked version as `preferred`
(`--max-scope` caps it), and reports the rung reached and every package
that moved outside the divergent set. Not a full update: `preferred`
keeps every package the merge does not force. #297 makes `viv.lock` a
companion `install` and `update` read: `install` refuses when the two
files disagree in either direction, `update` implies `--lock native` once
`viv.lock` exists. #298 has `viv init` write the `.gitattributes` line and
`install` set `merge.viv.driver` in each clone that has it, so nobody
configures git by hand. #299 covers the clone that merges before either
has run: `install` reads the conflicted lock's own git index stages, runs
the same merge and re-solve the driver would, and wires the clone so it
doesn't happen again. Nothing queued; the chapter's open question is
adoption, not mechanism.

## Chapter 2: autoload from the store, no vendor tree (measured, not pursued)

**Question.** If `vendor/` is a generated autoloader plus the
`installed.json` and `installed.php` shims that frameworks read, and
classes load straight from the shared store, what happens to install time,
disk use, and framework compatibility?

**Result, 2026-09-21.** Not pursued. The control arm
(`bench/results/storeload.md`, #287) settled both claims before the flagged
arm was built.

- The disk saving is nil under the default link mode. `vendor/` on laravel
  is 1328 directories and 8834 files that share the store's inodes, so
  removing it reclaims directory entries and no package bytes. The first
  harness run reported this wrongly, counting distinct inodes, which equals
  the dentry count however many links a file has; the corrected measure
  counts link counts and cross-checks against `linkat`.
- The speed saving is real and bounded: 19 ms of a 43 ms warm install on
  laravel, nothing cold. Drupal, the corpus's file-count extreme, would show
  more. Nobody would feel either.
- The cost is a new failure class. With no tree the store is load-bearing at
  runtime, so `prune` can break a running project (#286).

A saving nobody perceives, bought with a failure nobody had. The harness and
the corrected disk measure are the chapter's output; #284–#286 stay open as
candidates should the boot question matter for another reason.

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

## Chapter 3: workspaces (measured, not pursued)

**Question.** When a repository holds several `composer.json` files, does
a workspace, one root that names its members and one lock over all of
them, give anything that one aggregate root built from `path`
repositories does not?

**Why the question changed.** The chapter first framed the alternative
as independent roots, one lock and one `vendor/` per member, and promised
three results against that control: one resolved version per package,
install cost that grows with the union of the dependencies rather than
the sum, and each member still installable alone. Practitioners asked
about `wikimedia/composer-merge-plugin` (2026-09-21) answered that the
real alternative is one aggregate root: a top-level `composer.json` that
requires every member through a `path` repository. Against that control
the first two results already hold in Composer today. One solve gives one
version per package, and one `vendor/` costs the union. Only the third
result is left standing. A member installed alone from an aggregate root
re-solves against the registry and can land on a different version of a
shared package than the platform runs. A workspace would install the
member alone against the shared lock.

**Hypothesis, narrowed.** Members are installed or tested alone often
enough, and drift when they are, that a shared lock every member reads
is worth a solve mode Composer does not have. If the corpus shows members
are not installed alone, or that they do not drift when they are, the
chapter ends with those numbers and no build.

**Control.** One aggregate root per repository: a generated top-level
`composer.json` that requires every member through a `path` repository
with `symlink: true`, installed once with `viv` in compat mode.
Independent roots stay in the table as the second column, because that
is what the repositories in the corpus do today.

**Measurement (#279).** A corpus of public repositories with several
`composer.json` files that are installable roots. Per repository:

- members found, and how many have their own `composer.lock` and CI
  (the evidence that members are installed alone)
- packages in the union versus the sum of the members
- distinct versions of the same package across the members' own locks
  (the drift count as the repositories live today)
- for each member: the versions its standalone solve picks versus the
  versions the aggregate solve picks (the drift a workspace would remove)
- warm and cold install time and store size for N independent installs
  versus one aggregate install

Results land in `bench/results/workspaces.md`. The build, #277 (one
solve and one lock over the union) and #278 (per-member `vendor/` and
`composer.lock` projection), starts only if the fourth row shows drift on
members that the first row shows are installed alone.

**Design sketch, if the build starts.** Members are declared in the root
`composer.json` under `extra.viv.workspace.members` as globs, so Composer
ignores the setting and compat mode is untouched. A member that requires
another member by name gets a symlink into the tree, the way `workspace:*`
works in pnpm. Each member gets a `vendor/` linked from the store and its
own generated autoloader against the shared lock, so `php members/x/bin/tool`
works without the root. `composer.lock` is still exportable per member,
projected from the shared lock, so a member can leave the workspace.

**Result (2026-09-24, `bench/results/workspaces.md`).** Ten public
repositories with 4 to 37 members each. Row 1: members are installed
alone, by a committed lock or a CI workflow, on 3 of the 10 (a WordPress
platform, a WordPress plugin monorepo, a Laravel shop). Row 4 on those
three: 0 external packages differ between a member's standalone solve
and the aggregate solve, over 22 comparisons. The repository with the
largest drift, laravel/framework at 54 of 58, is a read-only split
source whose components are never installed alone from the monorepo.
Row 5 confirms what the control already gives: warm install cost drops
19 to 118 times under one aggregate root, and the store shrinks where
members share dependencies. Coverage gap: two viv bugs kept the two
repositories shaped most like a workspace out of rows 4 and 5, Sylius
(every component declares a `path` repository to its siblings, #305)
and Neos (24 of 29 members use `self.version`, #304).

**Verdict.** Measured, not pursued. The rerun after #304 and #305
(same results file, second section) covered the two repositories the
first pass missed: all 17 Sylius components solve alone and agree with
the aggregate on every one of 73 shared requirements; the Neos members
that require siblings at `self.version` cannot be solved alone by
Composer either, since the sibling version a standalone member asks
for is not published. On every repository whose members are installed
alone, a workspace would remove no drift. #277 and #278 are closed as
not planned; `viv workspace list` (#276) stays as the discovery tool.

**Work.** Done: #276 (member discovery and `viv workspace list`), #279
(corpus and measurement, plus the rerun). Closed as not planned: #277,
#278. Side results: #304 and #305 fixed, both compat gaps on real
published `composer.json` files.

**What the chapter produced instead.** The measurement never scored
experience, and the aggregate root has one cost the numbers do not
show: someone writes the root `composer.json` with its `path`
repositories and one `require` per member, and keeps it in step. #315
(`viv workspace init` writes the aggregate root from member globs)
takes chapter 3's discovery code and turns it into that command, with
a proposing dry run instead of autodiscovery that writes. The file it
produces is plain Composer, so the compat contract holds and the
chapter's control becomes the thing viv sets up.

## Candidate chapters

Three chapters in, the pattern is clear. Chapter 1 held because it
asked a question Composer's users feel every week. Chapters 2 and 3
closed because the control already gave the result: Composer's cache
in one case, an aggregate root in the other. So every candidate below
names its control and a measurement that costs days, not weeks, and
none starts a build before that measurement is in. They are in the
order worth running them.

### Candidate A: an append-only ledger lock (measured, hold)

**Question.** If the lock is written as an ordered ledger of record
changes, one line per package change with the change that caused it,
does git's own `merge=union` merge it correctly with no driver at all,
or does the union fold over real conflicts silently?

**Why it might hold.** Chapter 1's driver merges by name-keyed record.
A ledger makes the record the unit of the text itself, so two branches
that change different packages append different lines and union
concatenates them. The cost is a fold at read time and the risk is the
silent case: both branches move the same package and union keeps both
lines, so the fold has to detect that and refuse.

**Control.** Chapter 1's `viv lock merge` on the same inputs.

**Measurement.** The client replay corpus from chapter 1 (355 merges,
`bench/results/lockmerge.md`). Convert both parents and the base to
the ledger, `merge=union` them, fold, and compare with the driver's
result: merges identical, merges where the union folded over a same-
package change without a marker, merges where the fold refused and the
driver did not. The chapter holds only if the second count is zero.

**Build if it holds.** A ledger writer and fold in `viv lock convert`,
then a decision on whether the driver stays for `composer.lock` only.

**Result, 2026-09-25.** The chapter holds: the silent-fold count is
zero. `bench/lockmerge/run.py --ledger` replayed 381 merges, the 355
client merges plus 26 from koel and pixelfed (`bench/results/lockmerge.md`).
The fold never finished a merge in which both sides changed a package to
different results, and on every merge both finished, the two results match.

Safe is not the same as sufficient. The fold finishes 277 of the 381
merges and the driver finishes 329; on the client corpus alone, 255 and
303 of 355. The 104 merges the fold refuses are 90 real conflicts and 14
where both sides moved a package to the same version and source reference
but one record carries a field the other lacks (`notification-url`). The
fold hashes the whole record, so it reads those 14 as a fork. Hashing
name, version and reference instead would finish them. The real conflicts
need what the driver has and the fold does not: a re-solve. The one merge
the fold finishes and the driver refuses has a malformed `composer.json`,
which the fold never reads.

Open decision: build the ledger as the format for merges that need no
re-solve and keep the driver for the rest, or keep the driver alone.

### Candidate B: install from a lock years later

**Question.** How many committed locks still install today, byte for
byte, at the commit that wrote them, and what breaks the rest?

**Why now.** Chapter 1's residue found 7 packages gone from Packagist
and every `dev-*` head overwritten since the lock was written. Those
were merges from the last two years. A lock is meant to reproduce an
install, and the corpus can say how long that holds in practice.

**Control.** Composer 2.10 on the same commit, same failure classes.

**Measurement.** The compat corpus and `compat/hunted.md` projects at
commits one, two and four years back, `viv install` from the committed
lock with no update: installs identical, dist URL gone (registry,
GitHub archive, private host), source reference gone, `dev-*` head
moved, platform requirement no longer met by a current PHP. Counts per
class and per age. Results in `compat/results/lock-age.md`.

**Build if it holds.** The doc's earlier "lock that describes the whole
install": content hashes of the extracted trees and a platform snapshot
in chapter 1's format, so an install can be verified against trees the
store still has when the URLs are gone.

### Candidate C: lock-seeded solving

**Question.** On an `update` that changes one package, how much of the
time goes to fetching and solving packages that end up unchanged?

**Control.** The full solve `viv update` runs today.

**Measurement.** No code. The profile already logs the closure fetch,
pool build and solve phases per run (`bench/results/profile.md`). For
each bench project, `viv update <one package>` against the committed
lock: phase times, packages fetched, packages whose locked version
survives. The chapter is worth a build if the surviving share is high
and the fetch dominates.

**Build if it holds.** Start the pool from the locked versions, widen
to the registry only for names the solver cannot satisfy from the
lock. Composer's partial update already keeps the rest locked; this
changes what is fetched, not what is chosen, so the compat sweep with
`COMPAT_LOCKS=1` is the gate.

### Candidate D: locks solved on one PHP, installed on another

**Question.** How often does a lock solved on a developer's PHP refuse
on the PHP that runs it, and would a lock that carries its platform
snapshot have caught it at `update` time?

**Why now.** #300 made `viv install` refuse when the platform does not
satisfy the lock, and #242 made `update` solve for a platform the
machine lacks. Both are correct and both move the failure around; the
question is how common the failure is.

**Control.** Composer's behaviour is identical, so the control is the
status quo: the count of projects where it bites.

**Measurement.** Across the compat corpus, each committed lock's
platform requirements against the previous and next PHP minor, and
against each PHP version the project's own CI matrix names: locks that
would refuse, and on which requirement (`php`, `ext-*`). All from
`composer.lock` and `.github/workflows`, no installs.

**Build if it holds.** A platform snapshot in chapter 1's format and a
`--platform` target on `update`, so the lock says what it was solved
for and `install` reports a mismatch as a solve problem, not a runtime
one.

### Parked, with reasons

- **Autoload as a store property.** #303's per-archive sidecar already
  stores the shaped classmap beside each archive; the remaining gap on
  Laravel is 14 ms of render, and chapter 2 showed the vendor tree is
  not the cost. A design change would not move the number.
- **A declared extension model.** The native adapters show most plugins
  are path mapping, scaffolding and patching, and a manifest could
  replace emulation. That is product design with the adapter list as
  its migration table, better scheduled when a client hits the next
  plugin than measured as a chapter.
