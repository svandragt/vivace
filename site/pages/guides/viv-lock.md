---
title: viv.lock
order: 30
summary: The conflict-less lock format on its own, converting, exporting, and installing from it alone.
---

# viv.lock

`viv.lock` is the record-per-package lock format behind [merging
composer.lock without conflicts](lock-merge.html); this page covers using
it on its own, with or without a committed `composer.lock` beside it.

## Converting and exporting

```sh
viv lock convert          # translate an existing composer.lock into viv.lock
viv lock export --check   # composer.lock from viv.lock; --check only compares
viv update --lock native  # resolves normally, also writes viv.lock beside composer.lock (implied once viv.lock exists)
```

`viv lock export` writes `composer.lock` from `viv.lock` through the same
writer a solve feeds, byte-equal on every pinned compatibility project.
`--check` reports whether the committed `composer.lock` matches without
writing anything.

## Installing from viv.lock alone

`viv install` in a project with `viv.lock` and no `composer.lock` generates
`composer.lock` first and says so, one line, before installing proceeds.
With both files present, `install` writes nothing and refuses a divergence
between them with a hint naming `viv lock export` and `viv lock convert` as
the fix — it never rewrites a `composer.lock` that's already on disk, on
any evidence, since `git checkout` doesn't preserve mtimes and a
present-but-differing file might be one a developer, or Composer itself,
legitimately changed.

A project can therefore commit `viv.lock` alone and let `viv install`
generate `composer.lock` from it, or keep committing both and let `install`
catch the two files disagreeing.

## Marking composer.lock as generated

`viv init --lock native` and `viv lock convert` mark `composer.lock`
`linguist-generated` in `.gitattributes`, so GitHub's diff view and
language statistics treat it the way a build artefact deserves: collapsed
in pull requests and excluded from the repository's language breakdown.

## Where the format comes from

Each `viv.lock` record holds a package's name, version, dist and source
references, its `dev`/root-requirement/time fields, and (since the export
above needed it) its own `composer.lock` block verbatim. The reasoning
behind that shape, and the merge behaviour it earns, is [chapter
1](../architecture/research/chapter-1.html) of the research programme.
