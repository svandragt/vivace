---
title: viv lock
order: 210
summary: converts, merges or exports a viv.lock/composer.lock pair
---

# viv lock

Lock file maintenance: translate an existing `composer.lock` into
`viv.lock` without re-solving, write `composer.lock` back from `viv.lock`,
or merge a lock as a git merge driver.

## Usage

```
{{help:lock}}
```

### viv lock convert

Reads `DIR/composer.lock` and `DIR/composer.json`, and writes
`DIR/viv.lock`, without re-solving anything. `--stdout` prints the result
instead of writing it.

```
{{help:lock convert}}
```

### viv lock export

Writes `DIR/composer.lock` from `DIR/viv.lock` and `DIR/composer.json`,
through the same writer a solve feeds. When `composer.json` names
isolated plugins in `extra.viv.isolate`, the export adds that map to
`composer.lock` as a top-level `extra.viv.isolate` key, which Composer
ignores. `--check` writes nothing and reports whether the existing
`composer.lock` already matches.

```
{{help:lock export}}
```

### viv lock merge

A git merge driver for `composer.lock`/`viv.lock`: merges the three inputs
(`%O %A %B`, see `git help gitattributes`) by name-keyed package record
instead of by text line, and writes the result over `ours`. Configure it as
a `merge=` driver in `.gitattributes` and git config, not run by hand
day-to-day.

```
{{help:lock merge}}
```

## Reads and writes

- Reads: `composer.json`, `composer.lock`, `viv.lock`; `merge` also reads
  the three merge inputs and, unless `--no-resolve`, fetches repository
  metadata over the network for a divergent package's re-solve.
- Writes: `viv.lock` (`convert`), `composer.lock` (`export`), the `ours`
  file in place (`merge`).

## Exit codes

- `0` — converted, exported, or merged cleanly (including `export --check`
  finding the file already up to date).
- `1` — `export --check` finding a difference, or a divergent name's
  re-solve in `merge` not finishing (conflict markers were written
  instead), or a filesystem error.
- `2` — as update, when `merge`'s re-solve is a genuine dependency-resolution
  failure rather than a divergent-name conflict.

## See also

[viv update-lock](update-lock.html), [Files viv writes](files.html)
