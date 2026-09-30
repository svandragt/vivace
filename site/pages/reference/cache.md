---
title: viv cache
order: 90
summary: prunes, cleans or reports the size of the shared store
---

# viv cache

Manages viv's shared store: prune stale entries, check its size, or remove
it outright.

## Usage

```
{{help:cache}}
```

### viv cache prune

Removes stale buckets, orphan temp directories, orphan `.ok` markers, and
any archive no dist pointer references any more. `--older-than DAYS` also
removes dist pointers not installed from in that many days, before
sweeping the archives that leaves unreferenced.

```
{{help:cache prune}}
```

### viv cache clean

Removes the whole cache, after confirming it looks like a viv cache (only
known bucket names, or empty); refuses otherwise.

```
{{help:cache clean}}
```

### viv cache size

Prints archive and dist-pointer counts and total size; writes nothing.

```
{{help:cache size}}
```

## Reads and writes

- Reads and writes: the store under `$XDG_CACHE_HOME/vivace` (or
  `--cache-dir`).

## Exit codes

- `0` — completed (including `clean` on an empty or already-clean cache).
- `1` — a filesystem error, or `clean` refusing a directory that holds
  something other than a viv cache.

## See also

[Files viv writes](files.html)
