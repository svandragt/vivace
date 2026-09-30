---
title: Workspaces
order: 60
summary: One resolve and one vendor/ across several composer.json files.
---

# Workspaces

```sh
viv workspace init packages/*   # write a top-level composer.json requiring every match, then resolve/install
viv workspace add plugins/new-one   # add one more member to that file and resolve again
viv workspace list        # list a workspace's members and their inter-requirements
```

A repository with several `composer.json` files can share one resolve and
one `vendor/` through a top-level root. `viv workspace init plugins/*
themes/*` writes that root: one `path` repository per pattern, one
`require` line for each package a pattern matches, and
`minimum-stability`/`prefer-stable` unless the file already sets them. It
then resolves and installs the way `add` does; `--no-install` stops after
writing the file and its lock. With no patterns, it lists every
`composer.json` below the current directory, grouped by parent directory,
and writes nothing, so you can copy the patterns you want.
`viv workspace add plugins/new-one` adds one more package to an existing
root and resolves again. The file is ordinary Composer input, so
`composer install` accepts it too. `viv workspace list` reports which
members in `extra.viv.workspace.members` require each other.

This is chapter 3 of viv's research programme; see [chapter
3](../architecture/research/chapter-3.html) for the question it set out to
answer and the measurement behind it.
