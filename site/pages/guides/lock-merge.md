---
title: Merging composer.lock without conflicts
order: 20
summary: The merge driver, .gitattributes wiring, and dev-* branches pinned by commit.
---

# Merging composer.lock without conflicts

{{include:README.md#Merging composer.lock without conflicts}}

## How the driver is wired in

`viv init` writes `composer.lock merge=viv` (and `viv.lock merge=viv` once
the project has adopted that file) to `.gitattributes`; an existing project
adds the line once by hand instead. `viv install` then sets `git config
merge.viv.driver 'viv lock merge %O %A %B'` in each clone that has the
attribute, so every future `git merge` hands both branches' locks to viv,
which merges them record by record and re-solves what diverged; a
`dev-*` branch both sides moved keeps the later commit, and a pinned
commit the registry no longer describes is fetched from the package's
git source. You only see conflict markers when the merged `composer.json`
cannot be satisfied. A clone that merges before anyone has run `install`
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
