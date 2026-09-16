# Cache and offline use

## One cache for every project

{{readme:One cache for every project}}

## The `cache` command

```
{{help:cache}}
```

`--cache-dir` points the store somewhere other than the default
(`$XDG_CACHE_HOME/vivace`, or `~/.cache/vivace`); pass it to any command
that touches the store. `--offline` fails fast on any request instead of
connecting: `install` errors, naming every package not already in the
store, and `update` solves from cached repository metadata only, erroring
on an uncached package.
