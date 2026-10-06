---
title: Merging composer.lock without conflicts
order: 20
summary: The merge driver, .gitattributes wiring, and dev-* branches pinned by commit.
---

# Merging composer.lock without conflicts

Two branches that each ran `update` conflict in `composer.lock` almost
every time, because git merges it line by line and the file is one large
JSON array. viv merges it as a git merge driver, record by record, and
re-solves only the packages the two branches changed differently. `viv
init` writes the `.gitattributes` line and `viv install` wires the driver
into each clone, so after the first install nobody runs anything extra.
Replayed over 355 real merges from four client projects, `composer.lock`
conflicted 228 times under git, 51 times with the driver, and 6 times
once a `dev-*` branch is pinned by commit and the later commit wins; the
6 are packages or commits that no longer exist anywhere, which viv marks
and names.[^19] A project can commit `viv.lock` alone and let `viv
install` generate `composer.lock` from it. The setup is described under
"Everyday commands".

## How the driver is wired in

`viv init` writes `composer.lock merge=viv` (and `viv.lock merge=viv` once
the project has adopted that file) to `.gitattributes`; an existing project
adds the line once by hand instead. `viv install` then sets `git config
merge.viv.driver 'viv lock merge %O %A %B'` in each clone that has the
attribute, so every future `git merge` hands both branches' locks to viv,
which merges them record by record and re-solves what diverged; a
`dev-*` branch both sides moved keeps the later commit, and a pinned
commit the registry no longer describes is fetched from the package's
git source. A commit that is gone from the source too is remembered in the
store, so the next run reports it without fetching again. You only see
conflict markers when the merged `composer.json` cannot be satisfied, or
when both sides moved a `dev-*` branch and viv cannot pick a winner: the
two commits have the same time, one has none, or one side removed the
package. Then viv prints one line on stderr naming the package and both
commits (or `removed`), and leaves conflict markers for that package. A clone that merges before anyone has run `install`
there yet has no driver configured, so git leaves plain conflict markers
instead — `install` notices, resolves the lock straight from git's own
index stages the same way, and wires the clone so the next merge doesn't
need to.

`viv lock merge --offline-rung` (opt-in, off by default) is a last resort
before conflict markers: once re-solving against the registry has already
failed, it tries keeping one side's own pinned version for a package the
merge can't agree on, but only when the two locks' own requirements say
that version still fits — no registry request at all. It can never
override an answer the registry re-solve already found, since it only
runs once that re-solve has given up.

For the design behind the record format and the merge floor these numbers
come from, see [chapter 1](../architecture/research/chapter-1.html) of the
research programme.

[^19]: The replay and its counts are in [`bench/results/lockmerge.md`](bench/results/lockmerge.md); the design and the remaining cases are chapter 1 of [`docs/research.md`](docs/research.md). Client projects are anonymised.
