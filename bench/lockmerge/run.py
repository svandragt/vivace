#!/usr/bin/env python3
"""Chapter 1 merge-replay harness (#273): does a name-sorted, no-aggregate
lock format (docs/research.md, `viv.lock`, #272) remove textual merge
conflicts that `composer.lock` has, and how many of the conflicts it
removes were real (both sides actually touched the same package)?

For every merge commit in a repo's history whose two parents BOTH changed
composer.lock relative to their merge base, this replays the three-way
merge under two formats and classifies the result:

  composer.lock  `git merge-file -p <ours> <base> <theirs>` on the file as
                 committed; the exit code is the textual conflict count.
  viv.lock       the same three blobs, each converted with
                 `viv lock convert --stdout` (run from a directory holding
                 that commit's composer.json + composer.lock), then the
                 same `git merge-file` on the converted trio.

"Real conflicts" are computed once from the composer.lock JSON, independent
of format: packages whose identity (version, source reference, dev flag --
`docs/research.md` chapter 1's own three fields, matching `viv lock
merge`'s `Identity`) differs between base->ours AND base->theirs, with
ours and theirs landing on different results. That is the control -- a
textual conflict a format removes is a win only if it was not a real one.

`viv lock convert` doesn't exist on main yet (#272's sibling work); this
script probes for the subcommand once and prints "n/a (viv lock convert
unavailable)" for the whole native column, rather than fabricate it, until
it lands.

Resolution archaeology (#273/#275): for every merge with a real conflict,
git already holds the human's resolution in the merge commit's own
composer.lock. This classifies it without solving anything -- whether
composer.json itself conflicted (source conflict, no lock format fixes
that) or only the lock diverged (lock-only, a driver's target), which side
each real-conflict package's resolution matches (ours/theirs/neither/
removed) and whether the chosen side was the higher version, and how many
packages outside the real-conflict set the resolution dragged along
(cascade) -- sizing how mechanical a merge driver's job would be. It also
re-runs the composer.json merge with each side's file put through
`viv normalize` first, to size how many of the source conflicts a
canonical key order would have prevented on its own.

Usage:
    bench/lockmerge/run.py [project-name,...] [--ledger] [--hybrid] [--offline-rung]
    bench/lockmerge/run.py --self-test

`--offline-rung` (#314) runs the driver twice per merge, without and with
that flag, and adds a table comparing the two: residue cleared/remaining,
the safety number (rung 4, the offline pin, accepted a pin the registry re-solve would have
picked differently, when it also finished), network avoided, and the
median time both ways.

Env: BENCH_CACHE (persistent clone cache, default matches
bench/storeload.sh's own -- clones live under $BENCH_CACHE/lockmerge/<safe
name>), LOCKMERGE_CAP (most recent qualifying merges per repo, default 200),
LOCKMERGE_CORPUS (corpus.toml path), LOCKMERGE_REPORT (output path), VIV
(binary probed for `lock convert`, default target/release/viv).
"""
from __future__ import annotations

import hashlib
import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
import time
import tomllib
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent.parent


def log(msg: str) -> None:
    print(f"lockmerge: {msg}", file=sys.stderr)


# The user's global ~/.gitconfig sets log.showsignature=true, which injects
# "Good "git" signature..." lines into `git log`/`git show` stdout ahead of
# the actual format string -- fatal for anything parsing %H. Every call
# here disables it explicitly rather than relying on nothing overriding it.
GIT_BASE = ["git", "-c", "log.showsignature=false"]


def git(cwd: Path, *args: str, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(
        [*GIT_BASE, *args], cwd=cwd, capture_output=True, text=True, check=check
    )


def git_ok(cwd: Path, *args: str) -> bool:
    """True if the git command exits 0, without raising on non-zero."""
    return subprocess.run([*GIT_BASE, *args], cwd=cwd, capture_output=True).returncode == 0


def git_or_none(cwd: Path, *args: str) -> str | None:
    result = subprocess.run([*GIT_BASE, *args], cwd=cwd, capture_output=True, text=True)
    return result.stdout.strip() if result.returncode == 0 else None


def blob(cwd: Path, rev: str, path: str) -> bytes | None:
    result = subprocess.run(
        [*GIT_BASE, "show", f"{rev}:{path}"], cwd=cwd, capture_output=True
    )
    return result.stdout if result.returncode == 0 else None


# --- corpus -----------------------------------------------------------


@dataclass
class Project:
    name: str
    repo: str


def parse_corpus(path: Path) -> list[Project]:
    with open(path, "rb") as f:
        data = tomllib.load(f)
    return [Project(p["name"], p["repo"]) for p in data.get("project", [])]


def update_corpus_note(path: Path, name: str, note: str) -> None:
    """Rewrite the `examined`/`skip` line under [[project]] name = "<name>"
    in place, the way `make compat-refresh` rewrites compat/corpus.toml's
    pins -- so the corpus file always shows the range the last run actually
    swept."""
    text = path.read_text()
    block_re = re.compile(
        r'(\[\[project\]\]\nname = "' + re.escape(name) + r'"\nrepo = "[^"]*"\n)'
        r'((?:(?:examined|skip) = "[^"]*"\n)*)'
    )

    def repl(m: re.Match) -> str:
        return m.group(1) + note + "\n"

    new_text, n = block_re.subn(repl, text, count=1)
    if n:
        path.write_text(new_text)


# --- merge enumeration --------------------------------------------------


@dataclass
class Merge:
    sha: str
    ours: str
    theirs: str
    base: str


def qualifying_merges(repo_dir: Path, cap: int) -> tuple[list[Merge], list[str], bool]:
    """Merge commits (newest first) whose two parents both changed
    composer.lock relative to their merge base, capped at the most recent
    `cap`. Returns (merges, footnotes, exhausted) where `exhausted` is True
    if the whole merge log was walked without hitting the cap."""
    shas = git(repo_dir, "log", "--merges", "--pretty=%H").stdout.split()
    merges: list[Merge] = []
    footnotes: list[str] = []
    exhausted = True
    for sha in shas:
        if len(merges) >= cap:
            exhausted = False
            break
        parents = git(repo_dir, "rev-list", "--parents", "-n", "1", sha).stdout.split()[1:]
        if len(parents) != 2:
            continue  # root commit or octopus merge: outside this measurement's definition
        ours, theirs = parents
        base = git_or_none(repo_dir, "merge-base", ours, theirs)
        if base is None:
            footnotes.append(f"{sha[:12]}: skipped, no merge base (unrelated histories)")
            continue
        if not all(
            git_ok(repo_dir, "cat-file", "-e", f"{rev}:composer.lock")
            for rev in (base, ours, theirs)
        ):
            continue  # lock not present on every side yet, doesn't qualify
        changed_ours = not git_ok(repo_dir, "diff", "--quiet", base, ours, "--", "composer.lock")
        changed_theirs = not git_ok(repo_dir, "diff", "--quiet", base, theirs, "--", "composer.lock")
        if changed_ours and changed_theirs:
            merges.append(Merge(sha, ours, theirs, base))
    return merges, footnotes, exhausted


# --- classification -------------------------------------------------


def lock_index(raw: bytes) -> dict[str, tuple[str | None, str | None, bool]] | None:
    """name -> (version, source-reference, dev): the same three-field
    identity `docs/research.md` chapter 1 and `viv lock merge`'s own
    `Identity` use, not just (version, reference). A two-field index made
    "Merges with real conflicts" undercount relative to the driver: a
    package whose dev classification differs between `ours`/`theirs` while
    its version/reference matches `base` on one side looked *unchanged* on
    that side (same 2-tuple), even though it genuinely changed too, hiding
    a real three-way divergence whenever the other side also changed the
    package (#275 chunk 2's 88-vs-82 finding)."""
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None
    idx: dict[str, tuple[str | None, str | None, bool]] = {}
    for key, dev in (("packages", False), ("packages-dev", True)):
        for pkg in data.get(key) or []:
            name = pkg.get("name")
            if not name:
                continue
            idx[name] = (pkg.get("version"), (pkg.get("source") or {}).get("reference"), dev)
    return idx


def real_conflict_names(base: dict, ours: dict, theirs: dict) -> set[str]:
    names = set()
    for name in set(base) | set(ours) | set(theirs):
        b, o, t = base.get(name), ours.get(name), theirs.get(name)
        if o != b and t != b and o != t:
            names.add(name)
    return names


def real_conflicts(base: dict, ours: dict, theirs: dict) -> int:
    return len(real_conflict_names(base, ours, theirs))


# --- ledger fold (#306) --------------------------------------------------
# Candidate A (docs/research.md): the lock written as an ordered ledger of
# package-record changes, `merge=union`'d and folded, compared against
# `viv lock merge`'s own result on the same merge -- a measurement only, no
# ledger writer or reader lands in viv from this.


def ledger_key(section: str, name: str) -> str:
    return f"{section}/{name}"


def lock_records(raw: bytes) -> dict[str, dict] | None:
    """key ("packages/<name>" or "packages-dev/<name>") -> the package's
    full JSON object, the ledger's record unit. Top-level fields
    (content-hash, plugin-api-version, ...) never become ledger lines --
    content-hash is recomputed from composer.json, so carrying it would
    fork every merge."""
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None
    records: dict[str, dict] = {}
    for section in ("packages", "packages-dev"):
        for pkg in data.get(section) or []:
            name = pkg.get("name")
            if not name:
                continue
            records[ledger_key(section, name)] = pkg
    return records


def canonical_record_text(pkg: dict) -> str:
    return json.dumps(pkg, sort_keys=True, separators=(",", ":"))


def record_hash(pkg: dict) -> str:
    return hashlib.sha1(canonical_record_text(pkg).encode()).hexdigest()


def record_reference(pkg: dict) -> str | None:
    """Source reference, falling back to the dist reference when there is
    no `source` block -- a dist-only entry (no VCS) still has a reference
    worth comparing, just not under `source`."""
    source = pkg.get("source") or {}
    if source.get("reference"):
        return source["reference"]
    return (pkg.get("dist") or {}).get("reference")


def identity_hash(pkg: dict) -> str:
    """The hybrid fold's fast-path hash (#306 follow-up): version +
    reference only, not the whole record -- so a package both sides move
    to the same place agrees even when one side's record carries an extra
    metadata field the other's doesn't (`notification-url`, the 14-merge
    false-fork the full-record hash gave in the first candidate-A
    measurement)."""
    text = json.dumps([pkg.get("version"), record_reference(pkg)], sort_keys=True, separators=(",", ":"))
    return hashlib.sha1(text.encode()).hexdigest()


def base_ledger_lines(records: dict[str, dict], hash_fn=record_hash) -> list[str]:
    """Genesis line per package, sorted by key. No cause token (`-`):
    nothing appended it, it's the shared start both parent ledgers embed
    verbatim, so it must come out byte-identical on both sides. `hash_fn`
    swaps in `identity_hash` for the hybrid fold; default keeps every
    existing caller's full-record behaviour."""
    return [
        f"set {key} - {hash_fn(records[key])} - {canonical_record_text(records[key])}"
        for key in sorted(records)
    ]


def parent_ledger_lines(
    base_records: dict[str, dict],
    side_records: dict[str, dict],
    cause: str,
    hash_fn=record_hash,
    base_lines: list[str] | None = None,
    base_hash: dict[str, str] | None = None,
) -> list[str]:
    """The base ledger unchanged, plus one appended line per package that
    differs from base: `set` with the new record, or `del`. `prev` chains
    to the base record's own hash (`-` for a package base never had).
    `cause` (the parent commit's short sha) makes two sides' lines differ
    textually even when the change agrees byte-for-byte, so `merge=union`
    can't collapse an agreement into a single line before the fold ever
    gets to reason about it. `base_lines`/`base_hash` let a caller
    precompute the shared base once per merge and pass the same values to
    both sides -- so timing the fold can charge that one-off cost once,
    not twice."""
    if base_lines is None:
        base_lines = base_ledger_lines(base_records, hash_fn)
    if base_hash is None:
        base_hash = {key: hash_fn(pkg) for key, pkg in base_records.items()}
    lines = list(base_lines)
    for key in sorted(set(base_records) | set(side_records)):
        base_pkg, side_pkg = base_records.get(key), side_records.get(key)
        if base_pkg is not None and side_pkg is not None and base_hash[key] == hash_fn(side_pkg):
            continue  # unchanged from base on this side
        prev = base_hash.get(key, "-")
        if side_pkg is None:
            lines.append(f"del {key} {prev} - {cause} -")
        else:
            lines.append(f"set {key} {prev} {hash_fn(side_pkg)} {cause} {canonical_record_text(side_pkg)}")
    return lines


@dataclass
class LedgerLine:
    op: str
    key: str
    prev: str
    result_hash: str
    cause: str
    payload: str


def parse_ledger_line(raw: str) -> LedgerLine | None:
    """Fields are fixed-width up to `cause`; `payload` (JSON, may itself
    contain spaces) is everything after, which is why it's last and the
    split is bounded."""
    parts = raw.split(" ", 5)
    if len(parts) != 6 or parts[0] not in ("set", "del"):
        return None
    op, key, prev, result_hash, cause, payload = parts
    return LedgerLine(op, key, prev, result_hash, cause, payload)


def fold_ledger(lines: list[str]) -> tuple[dict[str, dict] | None, str | None, bool]:
    """Replays a unioned ledger's lines and returns (records, None,
    picked) -- key -> final package dict, a deleted key simply absent --
    or (None, reason, False) when the fold refuses. Per key, lines chain
    by `prev`: two lines with the same `prev` and the same result (op,
    hash) are the same change seen twice (agreement, apply once); the
    same `prev` with different results is a fork; a line whose `prev`
    doesn't match the key's current head (the genesis hash, or `-` when
    there is none) also refuses. Grouping by key rather than replaying
    line order makes the fold order-independent, as required.

    `picked` is True when two agreeing entries (same op, same hash) carry
    different payload text -- only possible under the hybrid fold's
    identity hash (#306 follow-up), where "same version and reference"
    doesn't mean "byte-identical record". The winner is then the entry
    whose canonical JSON payload sorts first, a deterministic pick rather
    than an arbitrary one; under the full-record hash a matching hash
    already implies matching text, so `picked` never fires there."""
    parsed: list[LedgerLine] = []
    for raw in lines:
        if not raw.strip():
            continue
        pl = parse_ledger_line(raw)
        if pl is None:
            return None, f"unparseable ledger line: {raw!r}", False
        parsed.append(pl)

    by_key: dict[str, list[LedgerLine]] = {}
    for pl in parsed:
        by_key.setdefault(pl.key, []).append(pl)

    result: dict[str, dict] = {}
    picked = False
    for key, entries in by_key.items():
        genesis = [e for e in entries if e.cause == "-"]
        appended = [e for e in entries if e.cause != "-"]
        if len(genesis) > 1:
            return None, f"{key}: duplicate genesis line", False
        head_hash = genesis[0].result_hash if genesis else "-"

        for e in appended:
            if e.prev != head_hash:
                return None, f"{key}: prev {e.prev!r} does not match head {head_hash!r}", False

        if not appended:
            if genesis:
                result[key] = json.loads(genesis[0].payload)
            continue

        signatures = {(e.op, e.result_hash) for e in appended}
        if len(signatures) > 1:
            return None, f"{key}: fork ({len(appended)} divergent results for the same prev)", False

        winner = min(appended, key=lambda e: e.payload)
        if len({e.payload for e in appended}) > 1:
            picked = True
        if winner.op != "del":
            result[key] = json.loads(winner.payload)

    return result, None, picked


def record_identity(pkg: dict) -> tuple[str | None, str | None]:
    return pkg.get("version"), (pkg.get("source") or {}).get("reference")


def keyed_identity(records: dict[str, dict]) -> dict[str, tuple[str | None, str | None]]:
    return {key: record_identity(pkg) for key, pkg in records.items()}


def union_merge_ledger(
    work: Path, ours_lines: list[str], base_lines: list[str], theirs_lines: list[str]
) -> tuple[str | None, str | None]:
    """`git merge-file --union` of the three ledgers, in `work` -- pure
    appends on top of an identical shared prefix, so union has nothing to
    pick a side on; this only concatenates the two sides' own appended
    lines onto the shared base."""
    paths = {}
    for label, content_lines in (("ours", ours_lines), ("base", base_lines), ("theirs", theirs_lines)):
        p = work / f"ledger-{label}"
        p.write_text("\n".join(content_lines) + ("\n" if content_lines else ""))
        paths[label] = p
    result = subprocess.run(
        ["git", "merge-file", "--union", "-p", str(paths["ours"]), str(paths["base"]), str(paths["theirs"])],
        capture_output=True,
    )
    if result.returncode < 0:
        return None, f"git merge-file --union crashed (signal {-result.returncode})"
    return result.stdout.decode(), None


def classify_ledger(
    work: Path,
    base_records: dict[str, dict],
    ours_records: dict[str, dict],
    theirs_records: dict[str, dict],
    ours_cause: str,
    theirs_cause: str,
    real_names: set[str],
    driver_conflicted: bool,
    driver_lock: bytes | None,
) -> tuple[str, str | None]:
    """Compares the ledger fold against `viv lock merge`'s own result on
    the same merge. `driver_conflicted` must already be a known bool (the
    caller excludes a crashed driver run before calling this -- it isn't a
    control). Returns (category, detail); category is one of "identical",
    "silent_fold" (the chapter-breaking outcome -- either a real conflict
    folded without refusal, or the fold's result disagrees with the
    driver's), "fold_refused_driver_ok", "both_refused",
    "fold_ok_driver_refused"."""
    ours_lines = parent_ledger_lines(base_records, ours_records, ours_cause)
    theirs_lines = parent_ledger_lines(base_records, theirs_records, theirs_cause)
    base_lines = base_ledger_lines(base_records)
    union_text, err = union_merge_ledger(work, ours_lines, base_lines, theirs_lines)
    fold_records, fold_err, _picked = (None, err, False) if err else fold_ledger(union_text.splitlines())

    if fold_records is None:
        return ("both_refused" if driver_conflicted else "fold_refused_driver_ok"), fold_err

    if real_names:
        return "silent_fold", f"real conflict(s) folded without refusal ({len(real_names)} package(s))"

    if driver_conflicted:
        return "fold_ok_driver_refused", None

    driver_records = lock_records(driver_lock) if driver_lock is not None else None
    if driver_records is None or keyed_identity(fold_records) != keyed_identity(driver_records):
        return "silent_fold", "fold result differs from the driver's merged lock"
    return "identical", None


# --- hybrid fold (#306 follow-up, 2026-09-25) ---------------------------
# Candidate A's open decision: build the ledger for merges that need no
# re-solve and keep the driver for the rest, or keep the driver alone.
# This measures Option 2 (ledger union + the identity-hash fold, falling
# back to the driver when the fold refuses) against Option 1 (the driver
# alone, `classify_ledger`'s own control) on the same merges, including
# per-merge timing -- still a measurement only, no hybrid path lands in
# viv from this.


@dataclass
class HybridOutcome:
    identity_finished: bool  # the identity-hash fold alone produced a result
    silent: bool  # that result folded over a real conflict (real_conflict_names)
    picked: bool  # fold_ledger's deterministic tie-break fired (metadata-only difference)
    finished: bool  # the hybrid pipeline as a whole (fold, else driver) produced a result
    ne_driver: bool  # hybrid and driver both finished, package identity sets differ
    metadata_diff: bool  # identity sets agree, but a shared record's full JSON differs
    time: float  # seconds: the fold alone, or fold + driver on the fallback path


def classify_hybrid(
    base_records: dict[str, dict],
    ours_records: dict[str, dict],
    theirs_records: dict[str, dict],
    ours_cause: str,
    theirs_cause: str,
    real_names: set[str],
    driver_conflicted: bool,
    driver_lock: bytes | None,
    driver_time: float | None,
    work: Path,
) -> HybridOutcome:
    """Timed from here: building each side's ledger lines against an
    already-computed base state, the union, and the fold -- deliberately
    excluding the one-off cost of building that base state itself (paid
    once per merge below, not once per side, the same way a real hybrid
    implementation would only ever convert `base` once)."""
    base_lines = base_ledger_lines(base_records, identity_hash)
    base_hash = {key: identity_hash(pkg) for key, pkg in base_records.items()}

    start = time.perf_counter()
    ours_lines = parent_ledger_lines(base_records, ours_records, ours_cause, identity_hash, base_lines, base_hash)
    theirs_lines = parent_ledger_lines(base_records, theirs_records, theirs_cause, identity_hash, base_lines, base_hash)
    union_text, err = union_merge_ledger(work, ours_lines, base_lines, theirs_lines)
    fold_records, fold_err, picked = (None, err, False) if err else fold_ledger(union_text.splitlines())
    fold_time = time.perf_counter() - start

    identity_finished = fold_records is not None
    silent = identity_finished and bool(real_names)
    driver_finished = driver_conflicted is False
    finished = identity_finished or driver_finished

    if identity_finished:
        hybrid_records, hybrid_time = fold_records, fold_time
    elif driver_finished:
        hybrid_records = lock_records(driver_lock) if driver_lock is not None else None
        hybrid_time = fold_time + (driver_time or 0.0)
    else:
        hybrid_records, hybrid_time = None, fold_time

    ne_driver = metadata_diff = False
    if driver_finished and hybrid_records is not None:
        driver_records = lock_records(driver_lock) if driver_lock is not None else None
        if driver_records is not None:
            if keyed_identity(hybrid_records) != keyed_identity(driver_records):
                ne_driver = True
            elif any(
                canonical_record_text(hybrid_records[k]) != canonical_record_text(driver_records[k])
                for k in hybrid_records
                if k in driver_records
            ):
                metadata_diff = True

    return HybridOutcome(identity_finished, silent, picked, finished, ne_driver, metadata_diff, hybrid_time)


# --- resolution archaeology (#273 addendum) -----------------------------
# For merges with real conflicts, git history already holds the human's
# resolution (the merge commit's own composer.lock). This classifies it
# without solving or touching the network: was composer.json itself
# conflicted (no lock format fixes that), which side's value the merge
# commit kept, and whether the resolution touched packages beyond the
# real-conflict set (cascade).


def _version_segments(v: str) -> list[str] | None:
    """None for a version this bench won't order (dev branches don't carry
    a release ordering worth approximating)."""
    if v.startswith("dev-") or "-dev" in v:
        return None
    s = v[1:] if v.startswith("v") else v
    return re.split(r"[.\-+]", s)


def _cmp_segment(a: str, b: str) -> int:
    if a.isdigit() and b.isdigit():
        ai, bi = int(a), int(b)
        return (ai > bi) - (ai < bi)
    return (a > b) - (a < b)


def compare_versions(v1: str | None, v2: str | None) -> int | None:
    """-1/0/1 for v1 vs v2, or None when either side is incomparable
    (dev branch) -- approximate ordering on release versions, no version
    library, this is a bench script."""
    if v1 is None or v2 is None:
        return None
    s1, s2 = _version_segments(v1), _version_segments(v2)
    if s1 is None or s2 is None:
        return None
    for a, b in zip(s1, s2):
        c = _cmp_segment(a, b)
        if c != 0:
            return c
    return (len(s1) > len(s2)) - (len(s1) < len(s2))


@dataclass
class PackageResolution:
    name: str
    outcome: str  # "ours" | "theirs" | "neither" | "removed"
    higher: str | None  # "higher" | "not_higher" | "na" (only set for ours/theirs)


def classify_resolution(
    m: tuple | None, o: tuple | None, t: tuple | None
) -> tuple[str, str | None]:
    """m/o/t are (version, source-reference, dev) tuples from the merge
    commit's, ours', and theirs' composer.lock (or None when the package is
    absent)."""
    if m is None:
        return "removed", None
    if m == o:
        outcome = "ours"
    elif m == t:
        outcome = "theirs"
    else:
        return "neither", None
    o_ver = o[0] if o else None
    t_ver = t[0] if t else None
    cmp = compare_versions(o_ver, t_ver)
    if cmp is None or cmp == 0:
        higher = "na" if cmp is None else "not_higher"
    else:
        ours_is_higher = cmp > 0
        higher = "higher" if (outcome == "ours") == ours_is_higher else "not_higher"
    return outcome, higher


# The offline pin's own rung number, `src/lock_merge.rs`'s
# `OFFLINE_PIN_RUNG` -- one further rung past the highest of the three
# escalation rungs (1-3), not a fourth choice among them, so `viv lock
# merge`'s own "resolved via rung N" line prints 4, never 0. An earlier
# pass of this harness assumed 0 (this file's own prior `parse_resolution`
# self-test example used it too); every check gating on the offline pin
# having been the one that resolved a merge reads this constant, not a
# literal, so there is exactly one place left to get it wrong.
OFFLINE_PIN_RUNG = 4


def classify_safety(
    offline_idx: dict, baseline_idx: dict, ours_idx: dict, theirs_idx: dict,
) -> str | None:
    """#314's safety number: `None` when every package the offline pin
    (rung 4) and the registry re-solve (baseline, no flag) both resolved
    agrees; otherwise one kind string per differing package, joined,
    naming which parent the pin came from and whether the registry chose
    older or newer. Compares every package, not just the divergent names
    -- rung 3's own `moved` list (#296) means a *non*-divergent package
    can differ too."""
    kinds: list[str] = []
    for name in sorted(set(offline_idx) | set(baseline_idx)):
        off, base = offline_idx.get(name), baseline_idx.get(name)
        if off == base:
            continue
        parent = "neither parent"
        if off == ours_idx.get(name):
            parent = "ours"
        elif off == theirs_idx.get(name):
            parent = "theirs"
        off_ver = off[0] if off else None
        base_ver = base[0] if base else None
        cmp = compare_versions(off_ver, base_ver)
        if cmp is None:
            direction = "unversioned (dev branch)"
        elif cmp < 0:
            direction = "older pin kept"
        elif cmp > 0:
            direction = "newer chosen"
        else:
            direction = "same version, different reference"
        kinds.append(f"{parent}, {direction}")
    return "; ".join(kinds) if kinds else None


def normalize_composer_json(viv_bin: str, content: bytes) -> tuple[bytes | None, str | None]:
    """`viv normalize -d <tmpdir>` rewrites composer.json in place; no
    --stdout, so write, run, read back."""
    with tempfile.TemporaryDirectory(prefix="lockmerge-normalize-") as td:
        path = Path(td) / "composer.json"
        path.write_bytes(content)
        try:
            result = subprocess.run([viv_bin, "normalize", "-d", td], capture_output=True)
        except FileNotFoundError:
            return None, f"viv binary not found at {viv_bin}"
        if result.returncode != 0:
            stderr = result.stderr.decode(errors="replace").strip()
            return None, (stderr.splitlines()[-1] if stderr else "viv normalize failed")
        return path.read_bytes(), None


@dataclass
class ArchOutcome:
    sha: str
    source_conflict: bool
    packages: list[PackageResolution]
    cascade: int
    normalized_conflict: bool | None  # None: normalize failed or errored, see normalize_error
    normalize_error: str | None


def compute_archaeology(
    repo_dir: Path,
    work: Path,
    m: Merge,
    real_names: set[str],
    base_idx: dict,
    ours_idx: dict,
    theirs_idx: dict,
    viv_bin: str,
    footnotes: list[str],
) -> ArchOutcome | None:
    base_json = blob(repo_dir, m.base, "composer.json")
    ours_json = blob(repo_dir, m.ours, "composer.json")
    theirs_json = blob(repo_dir, m.theirs, "composer.json")
    if base_json is None or ours_json is None or theirs_json is None:
        footnotes.append(f"{m.sha[:12]}: composer.json missing on one side, archaeology skipped")
        return None
    json_conflicts, err = merge_file_conflicts(work, ours_json, base_json, theirs_json)
    if err:
        footnotes.append(f"{m.sha[:12]}: composer.json merge: {err}, archaeology skipped")
        return None

    m_lock = blob(repo_dir, m.sha, "composer.lock")
    m_idx = lock_index(m_lock) if m_lock is not None else None
    if m_idx is None:
        footnotes.append(f"{m.sha[:12]}: merge's own composer.lock missing/unparseable, archaeology skipped")
        return None

    packages = []
    for name in sorted(real_names):
        outcome, higher = classify_resolution(m_idx.get(name), ours_idx.get(name), theirs_idx.get(name))
        packages.append(PackageResolution(name, outcome, higher))

    dragged = (set(m_idx) | set(ours_idx) | set(theirs_idx)) - real_names
    cascade = sum(
        1
        for name in dragged
        if m_idx.get(name) != ours_idx.get(name) and m_idx.get(name) != theirs_idx.get(name)
    )

    norm_ours, e1 = normalize_composer_json(viv_bin, ours_json)
    norm_base, e2 = normalize_composer_json(viv_bin, base_json)
    norm_theirs, e3 = normalize_composer_json(viv_bin, theirs_json)
    norm_err = e1 or e2 or e3
    if norm_err:
        footnotes.append(f"{m.sha[:12]}: viv normalize failed, archaeology's normalized column is n/a: {norm_err}")
        normalized_conflict, normalize_error = None, norm_err
    else:
        norm_conflicts, err = merge_file_conflicts(work, norm_ours, norm_base, norm_theirs)
        if err:
            footnotes.append(f"{m.sha[:12]}: normalized composer.json merge: {err}, archaeology's normalized column is n/a")
            normalized_conflict, normalize_error = None, err
        else:
            normalized_conflict, normalize_error = norm_conflicts > 0, None

    return ArchOutcome(m.sha, json_conflicts > 0, packages, cascade, normalized_conflict, normalize_error)


def merge_file_conflicts(work: Path, ours: bytes, base: bytes, theirs: bytes) -> tuple[int | None, str | None]:
    """`git merge-file -p <ours> <base> <theirs>`; exit code is the
    conflict count (0 = clean), or negative on a real error."""
    paths = []
    for label, content in (("ours", ours), ("base", base), ("theirs", theirs)):
        p = work / label
        p.write_bytes(content)
        paths.append(str(p))
    result = subprocess.run(
        ["git", "merge-file", "-p", *paths], capture_output=True
    )
    if result.returncode < 0:
        return None, f"git merge-file crashed (signal {-result.returncode})"
    return result.returncode, None


def viv_bin_commit(viv_bin: str) -> str | None:
    """The git commit that produced `viv_bin`, when it sits under a
    `target/{release,debug}/viv` of a checked-out git repo -- so a run whose
    native column comes from a branch binary (#273: `viv lock convert`
    landed on origin/273-lock-convert ahead of main) records exactly which
    commit, not just the version string every build of 0.14.0 shares."""
    try:
        repo_root = Path(viv_bin).resolve().parents[2]
    except IndexError:
        return None
    if not (repo_root / ".git").exists():
        return None
    return git_or_none(repo_root, "rev-parse", "HEAD")


def probe_native(viv_bin: str) -> bool:
    try:
        result = subprocess.run([viv_bin, "lock", "convert", "--help"], capture_output=True)
    except FileNotFoundError:
        return False
    return result.returncode == 0


def probe_driver(viv_bin: str) -> bool:
    try:
        result = subprocess.run([viv_bin, "lock", "merge", "--help"], capture_output=True)
    except FileNotFoundError:
        return False
    return result.returncode == 0


_RUNG_RE = re.compile(r"resolved via rung (\d+) \((\w+)\):")
_MOVED_RE = re.compile(r"^viv lock merge: \S+ moved outside the divergent set: ")


def parse_resolution(stderr: str) -> tuple[int | None, int]:
    """`viv lock merge`'s own "say what moved" lines (#296): the rung
    reached (`None` when nothing printed one -- a merge with no divergence
    at all never calls the escalation path) and how many packages it named
    as moved outside the divergent set."""
    rung: int | None = None
    moved = 0
    for line in stderr.splitlines():
        match = _RUNG_RE.search(line)
        if match:
            rung = int(match.group(1))
        elif _MOVED_RE.match(line):
            moved += 1
    return rung, moved


def summarize_resolve_failure(stderr: str) -> str:
    """A one-line reason for a failed re-solve. The solver's own
    `SolverError` (`src/solver/problem.rs`'s `Display`) is a `  Problem N`
    report, each with a `    - ` bullet list, followed by a fixed
    "Potential causes ... Read <troubleshooting>" tail when any problem
    named a package that doesn't exist at all -- so neither the first nor
    the last line of the whole message is the reason. Categorising by the
    *head* bullet ("Root composer.json requires X ^N -> satisfiable by
    X[v]", always the request that started the chain, never the cause) is
    what misled an earlier pass of this harness into calling 61 client
    footnotes "platform anachronism": one, checked by hand, actually said
    "Y dev-latest conflicts with X 9.6.31" two lines further down. The
    *leaf* -- Problem 1's last `- ` bullet -- is the one that names the
    actual failure ("conflicts with", "is missing from your platform",
    "does not satisfy", "no matching package"). "Could not be found in any
    version" (a package that doesn't exist) is checked first regardless of
    position: it's already a leaf, a dead end with nothing to walk further,
    so there's no head/leaf distinction to get wrong for it. Falls back to
    the previous head-based heuristic, then the first non-empty line, for
    stderr with no `Problem` block at all (a network error, a repository
    type viv doesn't support, a JSON parse error)."""
    lines = [line.strip() for line in stderr.splitlines()]
    stripped = [line for line in lines if line]
    if not stripped:
        return "no stderr"
    for line in stripped:
        if "could not be found in any version" in line:
            return line

    bullets: list[str] = []
    in_problem_1 = False
    for line in lines:
        if line == "Problem 1":
            in_problem_1 = True
            continue
        if not in_problem_1:
            continue
        if not line.startswith("-"):
            break
        bullets.append(line)
    if bullets:
        return bullets[-1]

    for line in stripped:
        if line.startswith("- ") and "Root composer.json requires" in line:
            return line
    return stripped[0]


def _php_floor(constraint: str) -> str | None:
    """The highest `major.minor` mentioned in a version constraint string,
    as `<major>.<minor>.99`: a bare major (`^7`) is read as `<major>.0`, so
    `^5.6|^7` picks `7` over `5.6` (major wins), giving `7.0.99`. No
    constraint parser -- this is a floor for `declare_contemporaneous_platform`,
    not a real evaluator, and it is deliberately wrong for an
    upper-bound-exclusive constraint like `>=7.4 <8.3` (picks `8.3.99`,
    which then fails `<8.3`); that merge stays footnoted, honestly."""
    tokens = re.findall(r"\d+(?:\.\d+)*", constraint)
    if not tokens:
        return None

    def key(token: str) -> tuple[int, int]:
        parts = token.split(".")
        return int(parts[0]), int(parts[1]) if len(parts) > 1 else 0

    major, minor = max((key(t) for t in tokens))
    return f"{major}.{minor}.99"


def _platform_constraints_in_lock(lock_bytes: bytes) -> dict[str, list[str]]:
    """Every `ext-*`/`lib-*` name in any package's own `require`, across
    `packages`+`packages-dev`, mapped to every constraint string seen for
    it: a lock entry's `require` is that package's real transitive
    requirement as of that commit, so this is the platform a
    contemporaneous resolve actually walked -- not just what the root
    manifest names directly (`ext-ffi`, needed by `jcupitt/vips` three
    levels under a root require, is invisible to the manifest alone but
    present in every lock that ever resolved it)."""
    try:
        data = json.loads(lock_bytes)
    except json.JSONDecodeError:
        return {}
    constraints: dict[str, list[str]] = {}
    for key in ("packages", "packages-dev"):
        for pkg in data.get(key) or []:
            for name, constraint in (pkg.get("require") or {}).items():
                if name.startswith(("ext-", "lib-")):
                    constraints.setdefault(name, []).append(str(constraint))
    return constraints


def declare_contemporaneous_platform(composer_json: bytes, ours_lock: bytes, theirs_lock: bytes) -> bytes:
    """The replay re-solves a historical manifest against today's
    Packagist on today's platform; a contemporaneous developer's PHP
    satisfied their own manifest by definition, so this declares a
    platform that does too, the same way a project pins its own target --
    via `config.platform`, the mechanism `pool_builder` already reads
    (`src/lock_merge.rs`'s own investigation), not a `--ignore-platform-reqs`
    viv's solver does not implement yet (#242). Derives a `php` floor from
    the manifest's own `require.php` (`_php_floor`'s heuristic and known
    failure case above); every `ext-*`/`lib-*` name in `require`/
    `require-dev`, plus every such name `_platform_constraints_in_lock`
    finds in `ours_lock`/`theirs_lock` (the transitive closure as of that
    commit, `ours`/`theirs` rather than `base` since either side's own
    resolve is a real historical platform, closer to the merge than the
    common ancestor), gets its *own* floor from every constraint string
    ever seen for that name, not the php one: PECL extensions version
    independently of PHP (`ext-zip`'s `^1.14.0` has nothing to do with PHP
    8.4), so reusing the php floor for it fails its own constraint outright.
    Falls back to the php floor only when none of a name's constraints
    contain a digit (`*`, or an implicit `ext-foo` with no version at all).
    `setdefault` throughout, so a name the manifest's own `config.platform`
    already sets keeps its override. Leaves `php` (and so everything else)
    untouched when there is no `require.php` to derive a floor from.
    Malformed JSON is left as-is; that merge's footnote is `viv lock
    merge`'s own parse error, not this rewrite's."""
    try:
        data = json.loads(composer_json)
    except json.JSONDecodeError:
        return composer_json

    require = data.get("require") or {}
    php_floor = _php_floor(require["php"]) if "php" in require else None
    if php_floor is None:
        return composer_json

    config = data.setdefault("config", {})
    platform = config.setdefault("platform", {})
    platform.setdefault("php", php_floor)

    constraints: dict[str, list[str]] = {}
    for links in (require, data.get("require-dev") or {}):
        for name, constraint in links.items():
            if name.startswith(("ext-", "lib-")):
                constraints.setdefault(name, []).append(str(constraint))
    for lock_bytes in (ours_lock, theirs_lock):
        for name, values in _platform_constraints_in_lock(lock_bytes).items():
            constraints.setdefault(name, []).extend(values)

    for name, values in constraints.items():
        floor = _php_floor(" ".join(values)) or php_floor
        platform.setdefault(name, floor)

    return json.dumps(data).encode()


# --- hang watchdog (#314 step 4) ----------------------------------------
# A real client-corpus sweep does live network I/O per merge (registry
# escalation, now also `viv install`'s dist/source fetches); the previous
# replay saw two `viv lock merge` calls run past a 90-second harness
# watchdog under real Packagist load. Rather than requiring someone to
# watch the whole multi-hour sweep and intervene by hand, every long-lived
# subprocess here runs under this watchdog: past `LOCKMERGE_HANG_TIMEOUT`
# seconds (default 300, the runbook's own threshold), it captures a few
# `/proc` diagnostics before killing the process, so a hang leaves a trace
# instead of an unattended stall -- and the caller still gets back
# something that looks exactly like a crashed subprocess (negative
# returncode), no special-casing needed downstream.

HANG_TIMEOUT = int(os.environ.get("LOCKMERGE_HANG_TIMEOUT", "300"))


def capture_hang_diagnostics(pid: int, hang_dir: Path, label: str) -> Path:
    hang_dir.mkdir(parents=True, exist_ok=True)
    out = hang_dir / f"hang-{label}-{pid}.txt"
    sections = []
    for name, cmd in (
        ("stack", ["cat", f"/proc/{pid}/stack"]),
        ("fd", ["ls", "-l", f"/proc/{pid}/fd"]),
        ("cmdline", ["sh", "-c", f"tr '\\0' ' ' < /proc/{pid}/cmdline"]),
        ("pstree", ["pstree", "-p", str(pid)]),
    ):
        result = subprocess.run(cmd, capture_output=True, text=True)
        sections.append(f"--- {name} ---\n{result.stdout}{result.stderr}")
    out.write_text("\n\n".join(sections) + "\n")
    return out


def run_with_watchdog(
    command: list[str], hang_dir: Path | None, label: str, timeout: int = HANG_TIMEOUT
) -> subprocess.CompletedProcess:
    """Like `subprocess.run(command, capture_output=True)`, but past
    `timeout` seconds it captures `/proc/<pid>` diagnostics into `hang_dir`
    (when given -- `None` skips capture, e.g. under `--self-test`, which
    never runs anything this slow) before killing the process, returning a
    result with `returncode=-9` as if the process had been sent SIGKILL
    directly -- indistinguishable downstream from any other crash."""
    proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        stdout, stderr = proc.communicate(timeout=timeout)
        return subprocess.CompletedProcess(command, proc.returncode, stdout, stderr)
    except subprocess.TimeoutExpired:
        if hang_dir is not None:
            path = capture_hang_diagnostics(proc.pid, hang_dir, label)
            log(f"{label}: hung past {timeout}s (pid {proc.pid}), diagnostics: {path}")
        proc.kill()
        stdout, stderr = proc.communicate()
        return subprocess.CompletedProcess(command, -9, stdout, stderr)


def driver_conflict(
    viv_bin: str, work: Path, cache_dir: Path, repo_dir: Path, sha: str,
    composer_json: bytes | None, base: bytes, ours: bytes, theirs: bytes,
    offline_rung: bool = False, hang_dir: Path | None = None,
) -> tuple[bool | None, str | None, int | None, int, bytes | None]:
    """`viv lock merge` on the composer.lock trio, run from a directory
    holding the merge commit's own composer.json with its platform
    declared contemporaneous (`declare_contemporaneous_platform`) and
    `--as-of` set to the merge commit's own committer date: the replay
    re-solves a historical manifest against today's Packagist, but a
    contemporaneous developer's `composer update` could only ever pick a
    version that existed by the time they ran it, so this drops anything
    Packagist dates later than the merge itself, the same way the platform
    declaration accounts for the machine instead of guessing at it with a
    flag viv's solver doesn't have. `dev-*` branches remain today's heads
    regardless (`--as-of`'s own doc comment) and are footnoted like any
    other failure when that's what a re-solve actually hits.

    Exit 0 is clean (a divergent name's own re-solve finished, chunk 2),
    exit 1 is a name that stayed divergent -- either no re-solve was
    attempted (no divergence at all) or it was and didn't finish, in which
    case `summarize_resolve_failure` pulls the reason (dead package,
    abandoned repo URL, constraint conflict, no network) out of stderr for
    the caller to footnote as an honest result, not a harness gap.
    `cache_dir` is reused across every merge in a repo so a warm re-solve
    isn't repaying the same Packagist metadata fetch each time;
    `--cache-dir` (not the real `~/.cache/vivace`), same isolation rule as
    `bench/run.sh`. Declaring the platform (and now the registry's own
    moment) changes the `content-hash` `viv lock merge` would write, so
    this only ever looks at the exit code and stderr, never the lock it
    produced -- a byte comparison against the merge commit's own
    composer.lock would be comparing apples to a platform, and now a
    registry state, that was never real.
    Returns (None, error, None, 0, None) on anything else (a crash, or
    composer.json missing at that revision). The third and fourth values
    are `parse_resolution`'s own (#296): the rung a successful resolve
    reached, and how many packages it named as moved outside the divergent
    set -- both `None`/0 on a conflict or a crash, since neither prints
    that pair. The fifth is the merged `composer.lock` bytes on a clean
    (exit 0) run, read back off `ours` -- `merge_composer_lock` writes its
    result there unconditionally (#306: the ledger fold's own control
    needs the driver's actual merged content, not just whether it
    conflicted). `offline_rung` (#314) appends `--offline-rung`; a
    successful offline-pin resolve reports rung 4 through the same
    `parse_resolution` line, no format change needed here."""
    if composer_json is None:
        return None, "composer.json missing at the merge commit", None, 0, None
    composer_json = declare_contemporaneous_platform(composer_json, ours, theirs)
    tmpdir = work / "driver-src"
    tmpdir.mkdir(exist_ok=True)
    (tmpdir / "composer.json").write_bytes(composer_json)
    paths = {}
    for label, content in (("base", base), ("ours", ours), ("theirs", theirs)):
        p = tmpdir / f"{label}.lock"
        p.write_bytes(content)
        paths[label] = p
    command = [
        viv_bin, "--cache-dir", str(cache_dir), "lock", "merge",
        str(paths["base"]), str(paths["ours"]), str(paths["theirs"]), "-d", str(tmpdir),
    ]
    committer_date = git_or_none(repo_dir, "show", "-s", "--format=%cI", sha)
    if committer_date:
        command += ["--as-of", committer_date]
    if offline_rung:
        command.append("--offline-rung")
    result = run_with_watchdog(command, hang_dir, f"{sha[:12]}-lock-merge")
    if result.returncode not in (0, 1):
        stderr = result.stderr.decode(errors="replace").strip()
        return None, (summarize_resolve_failure(stderr) if stderr else f"viv lock merge crashed ({result.returncode})"), None, 0, None
    if result.returncode == 1:
        stderr = result.stderr.decode(errors="replace").strip()
        return True, (summarize_resolve_failure(stderr) if stderr else None), None, 0, None
    stderr = result.stderr.decode(errors="replace").strip()
    rung, moved = parse_resolution(stderr)
    return False, None, rung, moved, paths["ours"].read_bytes()


def native_lock_text(viv_bin: str, work: Path, composer_json: bytes | None, composer_lock: bytes) -> tuple[bytes | None, str | None]:
    if composer_json is None:
        return None, "composer.json missing at this revision"
    tmpdir = work / "convert-src"
    tmpdir.mkdir(exist_ok=True)
    (tmpdir / "composer.json").write_bytes(composer_json)
    (tmpdir / "composer.lock").write_bytes(composer_lock)
    result = subprocess.run(
        [viv_bin, "lock", "convert", "--stdout", "-d", str(tmpdir)], capture_output=True
    )
    if result.returncode != 0:
        stderr = result.stderr.decode(errors="replace").strip()
        return None, (stderr.splitlines()[-1] if stderr else "viv lock convert failed")
    return result.stdout, None


# --- per-repo run --------------------------------------------------


@dataclass
class MergeOutcome:
    sha: str
    composer_conflicts: int | None
    native_conflicts: int | None
    native_error: str | None
    real: int
    driver_conflict: bool | None = None
    driver_error: str | None = None
    driver_rung: int | None = None
    driver_moved: int = 0
    driver_time: float | None = None  # seconds, the driver_conflict() call itself
    ledger_category: str | None = None  # #306, only set with --ledger
    hybrid: "HybridOutcome | None" = None  # #306 follow-up, only set with --hybrid
    offline: "OfflineOutcome | None" = None  # #314, only set with --offline-rung


LEAF_CAUSE_DEV_HEAD = "dev-* head"
LEAF_CAUSE_PACKAGE_GONE = "package gone"
LEAF_CAUSE_MALFORMED_MANIFEST = "malformed manifest"
LEAF_CAUSE_OTHER = "other"


def leaf_cause_class(reason: str | None) -> str:
    """Buckets a `driver_error`/`summarize_resolve_failure` reason into the
    same four leaf-cause classes `bench/results/lockmerge.md`'s existing
    table names (2026-09-23 section): a `dev-*` branch's current head
    conflicting with a pinned release, a package Packagist no longer
    lists, a manifest that doesn't parse, or (#314's own residue, not seen
    in that table) anything else."""
    if not reason:
        return LEAF_CAUSE_OTHER
    if "could not be found in any version" in reason:
        return LEAF_CAUSE_PACKAGE_GONE
    if "dev-" in reason and "conflicts with" in reason:
        return LEAF_CAUSE_DEV_HEAD
    if "parsing composer.json" in reason or "composer.json" in reason and "comma" in reason:
        return LEAF_CAUSE_MALFORMED_MANIFEST
    return LEAF_CAUSE_OTHER


# --- dev-as-commits mode (#331 candidate 3.3) ----------------------------
# Chapter 1's driver leaves 51 of 355 client merges in conflict
# (`docs/research.md`, "The 51 are the replay's floor"), 43 of them because a
# `dev-*` requirement's registry entry only ever shows today's branch head.
# This mode never sends a `dev-*` record to that re-solve at all: its three-
# way identity is decided directly off `source.reference` (the commit each
# side locked), and only the remaining, non-`dev-*` residue goes to the
# unmodified driver -- offline, since nothing here may fetch (the brief).

DEV_FINISHED = "finished"
DEV_REAL_CONFLICT = "real conflict"
DEV_OTHER_CONFLICT = "other conflict"
DEV_NEEDS_FETCH = "needs fetch"
DEV_TIMED_OUT = "timed out"


def is_dev_version(pkg: dict | None) -> bool:
    return pkg is not None and str(pkg.get("version") or "").startswith("dev-")


def records_by_name(raw: bytes) -> dict[str, dict] | None:
    """Like `lock_records`, but keyed by plain package name (no section
    prefix) -- this mode reasons about a package's identity across the
    merge, not which of `packages`/`packages-dev` it happened to sit in on
    a given side."""
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None
    records: dict[str, dict] = {}
    for section in ("packages", "packages-dev"):
        for pkg in data.get(section) or []:
            name = pkg.get("name")
            if name:
                records[name] = pkg
    return records


def dev_commit_pick(
    base_pkg: dict | None, ours_pkg: dict | None, theirs_pkg: dict | None
) -> tuple[bool, dict | None]:
    """The three-way pick the brief specifies for a `dev-*` record: same
    commit both sides or changed on one side only -> take it (`conflict`
    False, with the winning record or `None` when both sides removed it);
    changed to different commits on both sides -> a real conflict for a
    person (`conflict` True). Mirrors `real_conflict_names`'s own o!=b and
    t!=b and o!=t rule, keyed on `record_reference` instead of the
    (version, source-ref, dev) triple, since a branch's `version` string
    never changes even when its head does."""
    b_ref = record_reference(base_pkg) if base_pkg else None
    o_ref = record_reference(ours_pkg) if ours_pkg else None
    t_ref = record_reference(theirs_pkg) if theirs_pkg else None
    if o_ref == t_ref:
        return False, ours_pkg if ours_pkg is not None else theirs_pkg
    if o_ref == b_ref:
        return False, theirs_pkg
    if t_ref == b_ref:
        return False, ours_pkg
    return True, None


def strip_dev_records(raw: bytes, dev_names: set[str]) -> bytes | None:
    """Drops every `dev_names` entry from `packages`/`packages-dev` -- this
    mode has already decided their outcome itself, so the existing driver
    must never see them as divergent (or at all)."""
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None
    for section in ("packages", "packages-dev"):
        pkgs = data.get(section)
        if pkgs is not None:
            data[section] = [p for p in pkgs if p.get("name") not in dev_names]
    return json.dumps(data).encode()


def strip_dev_requires(composer_json: bytes, dev_names: set[str]) -> bytes:
    """Drops `dev_names` from root `require`/`require-dev` too -- rung 3
    (`solver::solve_update_seeded`) solves the root manifest's own requires
    directly, not just the lock's divergent set, so a `dev-*` name would
    still reach the registry through it if only the lock were stripped."""
    try:
        data = json.loads(composer_json)
    except json.JSONDecodeError:
        return composer_json
    for section in ("require", "require-dev"):
        reqs = data.get(section)
        if isinstance(reqs, dict):
            for name in dev_names:
                reqs.pop(name, None)
    return json.dumps(data).encode()


def dev_ref_cached(cache_root: Path, name: str, ref: str | None) -> bool:
    """Whether `name`'s cached `~dev` provider file -- under any host
    directory `--cache-dir` has fetched one for -- currently lists `ref` on
    one of its own `dev-*` version entries. Packagist's provider only ever
    serves today's branch head (`docs/research.md`'s own finding), so this
    is a proxy for "already resident, no source fetch needed" rather than
    a guarantee: a historical `ref` this cache never happened to see as
    today's head reads as needing a fetch, correctly."""
    if not ref:
        return False
    encoded = name.replace("/", "$")
    repo_v0 = cache_root / "repo-v0"
    if not repo_v0.is_dir():
        return False
    for host_dir in repo_v0.iterdir():
        f = host_dir / f"provider-{encoded}~dev.json"
        if not f.is_file():
            continue
        try:
            data = json.loads(f.read_text())
        except (OSError, json.JSONDecodeError):
            continue
        for entry in (data.get("packages") or {}).get(name, []):
            if not str(entry.get("version") or "").startswith("dev-"):
                continue
            entry_ref = (entry.get("source") or {}).get("reference") or (
                entry.get("dist") or {}
            ).get("reference")
            if entry_ref == ref:
                return True
    return False


def resolve_offline_stripped(
    viv_bin: str, work: Path, repo_dir: Path, sha: str,
    composer_json: bytes, base: bytes, ours: bytes, theirs: bytes, hang_dir: Path | None,
) -> tuple[str, str | None]:
    """`viv lock merge --offline` on the dev-stripped trio: the existing
    driver, unmodified, deciding only the non-`dev-*` residue. `--offline`
    means a cache miss fails fast with "Network disabled" (`src/fetch.rs`)
    instead of fetching, which this classifies as `DEV_NEEDS_FETCH` rather
    than a real conflict; a `run_with_watchdog` kill (past `HANG_TIMEOUT`,
    300s by default -- the brief's own cap) is `DEV_TIMED_OUT`. No
    `--cache-dir`: this reads viv's own default store, whatever earlier,
    non-bench viv activity on this machine already populated -- the brief's
    "metadata cache from earlier replays", never fetched fresh here."""
    composer_json = declare_contemporaneous_platform(composer_json, ours, theirs)
    tmpdir = work / sha[:12]
    tmpdir.mkdir(parents=True, exist_ok=True)
    (tmpdir / "composer.json").write_bytes(composer_json)
    paths = {}
    for label, content in (("base", base), ("ours", ours), ("theirs", theirs)):
        p = tmpdir / f"{label}.lock"
        p.write_bytes(content)
        paths[label] = p
    command = [
        viv_bin, "--offline", "lock", "merge",
        str(paths["base"]), str(paths["ours"]), str(paths["theirs"]), "-d", str(tmpdir),
    ]
    committer_date = git_or_none(repo_dir, "show", "-s", "--format=%cI", sha)
    if committer_date:
        command += ["--as-of", committer_date]
    result = run_with_watchdog(command, hang_dir, f"{sha[:12]}-dev-commits")
    if result.returncode == -9:
        return DEV_TIMED_OUT, None
    stderr = result.stderr.decode(errors="replace").strip()
    if result.returncode == 0:
        return DEV_FINISHED, None
    if "Network disabled" in stderr:
        return DEV_NEEDS_FETCH, (stderr.splitlines()[0] if stderr else None)
    return DEV_OTHER_CONFLICT, (summarize_resolve_failure(stderr) if stderr else f"exit {result.returncode}")


@dataclass
class DevMergeOutcome:
    project: str
    sha: str
    leaf_cause: str  # this merge's original (chapter 1) leaf cause, from the filter file
    category: str
    reason: str | None = None
    fetch_cost: int = 0  # only set when category is DEV_FINISHED


def dev_as_commits_merge(
    repo_dir: Path, work: Path, viv_bin: str, project_name: str, m: Merge, leaf_cause: str,
    hang_dir: Path | None, cache_root: Path,
) -> DevMergeOutcome:
    base_lock = blob(repo_dir, m.base, "composer.lock")
    ours_lock = blob(repo_dir, m.ours, "composer.lock")
    theirs_lock = blob(repo_dir, m.theirs, "composer.lock")
    if base_lock is None or ours_lock is None or theirs_lock is None:
        return DevMergeOutcome(project_name, m.sha, leaf_cause, DEV_OTHER_CONFLICT, "composer.lock missing on one side")

    base_by_name = records_by_name(base_lock)
    ours_by_name = records_by_name(ours_lock)
    theirs_by_name = records_by_name(theirs_lock)
    if base_by_name is None or ours_by_name is None or theirs_by_name is None:
        return DevMergeOutcome(project_name, m.sha, leaf_cause, DEV_OTHER_CONFLICT, "composer.lock did not parse as JSON")

    dev_names = {
        name
        for side in (base_by_name, ours_by_name, theirs_by_name)
        for name, pkg in side.items()
        if is_dev_version(pkg)
    }

    picks = {
        name: dev_commit_pick(base_by_name.get(name), ours_by_name.get(name), theirs_by_name.get(name))
        for name in dev_names
    }
    dev_conflicts = sorted(name for name, (conflict, _) in picks.items() if conflict)
    if dev_conflicts:
        return DevMergeOutcome(project_name, m.sha, leaf_cause, DEV_REAL_CONFLICT, ", ".join(dev_conflicts))

    base2 = strip_dev_records(base_lock, dev_names)
    ours2 = strip_dev_records(ours_lock, dev_names)
    theirs2 = strip_dev_records(theirs_lock, dev_names)
    if base2 is None or ours2 is None or theirs2 is None:
        return DevMergeOutcome(project_name, m.sha, leaf_cause, DEV_OTHER_CONFLICT, "composer.lock did not parse as JSON")

    merge_json = blob(repo_dir, m.sha, "composer.json")
    if merge_json is None:
        return DevMergeOutcome(project_name, m.sha, leaf_cause, DEV_OTHER_CONFLICT, "composer.json missing at the merge commit")
    merge_json = strip_dev_requires(merge_json, dev_names)

    category, reason = resolve_offline_stripped(
        viv_bin, work, repo_dir, m.sha, merge_json, base2, ours2, theirs2, hang_dir
    )

    fetch_cost = 0
    if category == DEV_FINISHED:
        for name, (_conflict, resolved_pkg) in picks.items():
            if resolved_pkg is None:
                continue  # removed on both sides, nothing to fetch
            ref = record_reference(resolved_pkg)
            if not dev_ref_cached(cache_root, name, ref):
                fetch_cost += 1

    return DevMergeOutcome(project_name, m.sha, leaf_cause, category, reason, fetch_cost)


def parse_only_conflicting(path: Path) -> dict[str, dict[str, str]]:
    """`--only-conflicting`'s file: `<project>\\t<sha12>\\t<leaf cause>` per
    line, one merge chapter 1's driver left in conflict -- this replay's
    own filter, not regenerated here (a fresh, non-`--offline` pass over
    all 355 merges would re-fetch every registry entry this mode exists to
    avoid touching)."""
    result: dict[str, dict[str, str]] = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        project, sha, cause = line.split("\t")
        result.setdefault(project, {})[sha] = cause
    return result


def run_dev_as_commits(
    projects: list[Project], cache_root: Path, viv_bin: str, only: dict[str, dict[str, str]],
    cap: int, hang_dir: Path | None,
) -> list[DevMergeOutcome]:
    tasks: list[tuple[Path, str, Merge, str]] = []
    for project in projects:
        wanted = only.get(project.name)
        if not wanted:
            continue
        repo_dir = clone_or_reuse(cache_root, project)
        merges, _footnotes, _exhausted = qualifying_merges(repo_dir, cap)
        by_prefix = {m.sha[:12]: m for m in merges}
        for sha12, cause in wanted.items():
            m = by_prefix.get(sha12)
            if m is None:
                log(f"{project.name}: {sha12} not found among qualifying merges (cap {cap}), skipped")
                continue
            tasks.append((repo_dir, project.name, m, cause))

    outcomes: list[DevMergeOutcome] = []
    with tempfile.TemporaryDirectory(prefix="lockmerge-dev-commits-") as tmp:
        work = Path(tmp)
        with ThreadPoolExecutor(max_workers=4) as pool:
            futures = [
                pool.submit(dev_as_commits_merge, repo_dir, work, viv_bin, name, m, cause, hang_dir, cache_root)
                for repo_dir, name, m, cause in tasks
            ]
            for future in futures:
                outcomes.append(future.result())
    return outcomes


def render_dev_as_commits(
    outcomes: list[DevMergeOutcome], cap: int, viv_bin: str, viv_version: str, viv_commit: str | None,
    only_path: Path, wall_time: float,
) -> str:
    lines = [
        f"\n## Generation 3: dev-* as commits\n",
        f"{datetime.now(timezone.utc).strftime('%Y-%m-%dT%H:%M:%SZ')}\n",
        f"\nCandidate 3.3 (`docs/research.md`, issue #331): replays only the "
        f"{len(outcomes)} merges chapter 1's driver (`viv lock merge`) left in "
        f"conflict (`--only-conflicting`, filtered from a prior full client-"
        f"corpus replay's own footnotes), not all 355. A `dev-*` record's "
        f"three-way identity is decided directly off its commit "
        f"(`source.reference`), never sent to the driver's re-solve; every "
        f"other record follows the unmodified driver, `--offline` (a cache "
        f"miss is `needs fetch`, never a live fetch). Cap: {cap} most recent "
        f"qualifying merges per repository, same corpus as chapter 1's. viv "
        f"binary: `{viv_bin}` ({viv_version}"
        + (f", commit `{viv_commit}`" if viv_commit else "")
        + f"). Wall time: {wall_time:.1f}s.\n",
    ]

    def counts(rows: list[DevMergeOutcome]) -> dict[str, int]:
        c = {DEV_FINISHED: 0, DEV_REAL_CONFLICT: 0, DEV_OTHER_CONFLICT: 0, DEV_NEEDS_FETCH: 0, DEV_TIMED_OUT: 0}
        for o in rows:
            c[o.category] = c.get(o.category, 0) + 1
        return c

    dev_rows = [o for o in outcomes if o.leaf_cause == LEAF_CAUSE_DEV_HEAD]
    other_rows = [o for o in outcomes if o.leaf_cause != LEAF_CAUSE_DEV_HEAD]

    lines.append(f"\n| Group | Merges | Finished | Real conflict | Other conflict | Needs fetch | Timed out |")
    lines.append(f"|---|---|---|---|---|---|---|")
    for label, rows in (("All", outcomes), ("dev-* leaf (chapter 1)", dev_rows), ("Other leaf (chapter 1)", other_rows)):
        c = counts(rows)
        lines.append(
            f"| {label} | {len(rows)} | {c[DEV_FINISHED]} | {c[DEV_REAL_CONFLICT]} | "
            f"{c[DEV_OTHER_CONFLICT]} | {c[DEV_NEEDS_FETCH]} | {c[DEV_TIMED_OUT]} |"
        )

    finished = [o for o in outcomes if o.category == DEV_FINISHED]
    fetch_total = sum(o.fetch_cost for o in finished)
    if finished:
        lines.append(
            f"\nOf the {len(finished)} finished merges, {fetch_total} `dev-*` "
            f"record(s) across them have a commit not in the cached provider "
            f"data -- the cost an install would pay to fetch it. The install "
            f"itself was not run (the brief: Packagist's own metadata for an "
            f"old branch head is gone, so confirming an install would need a "
            f"real fetch)."
        )
    else:
        lines.append(
            "\nNo merge finished, so there is no fetch-cost count: every "
            "finished-merge install check this candidate's third measurement "
            "asks for is moot on this replay. The install check was not run."
        )

    reasoned = [o for o in outcomes if o.reason]
    if reasoned:
        lines.append("\nReasons, per merge:\n")
        for o in reasoned:
            lines.append(f"- {o.project} {o.sha[:12]} ({o.category}): {o.reason}")

    lines.append(
        f"\nCorpus, cache and the `--only-conflicting {only_path.name}` filter "
        "(one `<project>\\t<sha12>\\t<leaf cause>` line per merge, built from "
        "a prior full client-corpus replay's own \"re-solve did not finish\" "
        "footnotes) are held outside the repository, same as the client "
        "corpus above; reproducible by the maintainer from that clone cache "
        "and by nobody else. Reproduce: `LOCKMERGE_CORPUS=<client corpus.toml> "
        "BENCH_CACHE=<client clone cache> bench/lockmerge/run.py "
        f"--dev-as-commits --only-conflicting <path to the filter file>`.\n"
    )
    return "\n".join(lines) + "\n"


# --- install check (#314 follow-up) -------------------------------------
# For every merge the offline pin actually finished (rung 4), does the
# merged lock it produced still install against today's Packagist, and
# what did keeping one parent's pin cost -- a security advisory the other
# side didn't carry, or reverting a divergent package the discarded side
# had already moved past. Only ever reads counts and kind labels out to
# the committed report; `reason`/`InstallCheckOutcome.reason` is a
# footnote/log detail, never client text landing in the repo.

INSTALL_INSTALLS = "installs"
INSTALL_DIST_GONE = "dist_gone"
INSTALL_SOURCE_REF_GONE = "source_ref_gone"
INSTALL_SHASUM_MISMATCH = "shasum_mismatch"
INSTALL_OTHER = "other"
INSTALL_CRASHED = "crashed"


def classify_install_failure(stderr: str) -> tuple[str, str]:
    """`viv install`'s stderr, bucketed by the message shape each cause
    actually produces: `src/fetch.rs`'s `get` wraps a non-2xx dist
    response in reqwest's own "HTTP status client error (404 ...)" text
    (dist gone); `src/source.rs`'s git checkout is wrapped in a "checking
    out {reference}" context (source ref gone); `src/fetch.rs`'s
    `verify_shasum` says "sha1 mismatch" (shasum mismatch); anything else
    is quoted verbatim as `other`, a bench-script heuristic same as
    `summarize_resolve_failure`'s, not an exhaustive parser."""
    lines = [line.strip() for line in stderr.splitlines() if line.strip()]
    if not lines:
        return INSTALL_OTHER, "no stderr"
    text = " ".join(lines)
    if "sha1 mismatch" in text:
        return INSTALL_SHASUM_MISMATCH, lines[0]
    if "checking out" in text:
        return INSTALL_SOURCE_REF_GONE, lines[0]
    if re.search(r"\b404\b", text) or "Not Found" in text:
        return INSTALL_DIST_GONE, lines[0]
    return INSTALL_OTHER, lines[0]


def strip_autoload(composer_json: bytes) -> bytes:
    """Drops `autoload`/`autoload-dev`: neither is part of `content-hash`
    (`src/lock.rs`'s own `RELEVANT` key list), so removing them can't stale
    the lock, and the install-check scratch dir never holds the historical
    project's own source tree -- a root `classmap`/`files` entry pointing
    at a directory that isn't there (`install`'s autoload dump scans it
    eagerly) would fail the install on the *root* package's own layout,
    nothing to do with whether the merged lock's dependencies still
    fetch, which is the only thing #314's install check measures."""
    try:
        data = json.loads(composer_json)
    except json.JSONDecodeError:
        return composer_json
    data.pop("autoload", None)
    data.pop("autoload-dev", None)
    return json.dumps(data).encode()


def install_check(
    viv_bin: str, scratch: Path, hang_dir: Path | None, sha: str,
    composer_json: bytes, composer_lock: bytes,
) -> tuple[str, str | None]:
    """Writes the merged lock+manifest into its own per-merge scratch dir
    (never the shared per-repo `work` -- it has to survive past the next
    merge's own driver call reusing that directory) and installs from it
    with a fresh, empty store: whether the kept pins still resolve to
    something fetchable today, not whether a warm cache already has it."""
    scratch.mkdir(parents=True, exist_ok=True)
    (scratch / "composer.json").write_bytes(strip_autoload(composer_json))
    (scratch / "composer.lock").write_bytes(composer_lock)
    result = run_with_watchdog(
        [
            viv_bin, "install", "--no-scripts", "--no-plugins", "--ignore-platform-reqs",
            "--cache-dir", str(scratch / "cache"), "-d", str(scratch),
        ],
        hang_dir, f"{sha[:12]}-install",
    )
    if result.returncode == 0:
        return INSTALL_INSTALLS, None
    if result.returncode < 0:
        # A negative returncode is Python's own convention for "killed by
        # signal N" (`run_with_watchdog`'s timeout kill included) -- its
        # own outcome (#320's own rule, applied here too), not folded into
        # "other" where an empty/truncated stderr would otherwise land it.
        return INSTALL_CRASHED, f"install crashed (signal {-result.returncode})"
    return classify_install_failure(result.stderr.decode(errors="replace").strip())


def run_audit_locked(
    viv_bin: str, cache_dir: Path, scratch: Path, hang_dir: Path | None, sha: str, label: str,
    composer_json: bytes | None, composer_lock: bytes,
) -> set[str] | None:
    """Package names `viv audit --locked --format json` flags with at
    least one advisory, or `None` when the audit produced no parseable
    JSON (a network failure, say) -- treated as "unknown", not "clean".
    `cache_dir` is the repo's shared driver cache (unlike `install_check`'s
    fresh one): the advisories database, not per-package content, so
    there's nothing wrong with a warm cache here and every reason to reuse
    the same fetch across a repo's merges."""
    scratch.mkdir(parents=True, exist_ok=True)
    (scratch / "composer.json").write_bytes(composer_json or b"{}")
    (scratch / "composer.lock").write_bytes(composer_lock)
    result = run_with_watchdog(
        [viv_bin, "audit", "--locked", "--cache-dir", str(cache_dir), "--format", "json", "-d", str(scratch)],
        hang_dir, f"{sha[:12]}-audit-{label}",
    )
    try:
        data = json.loads(result.stdout)
    except (json.JSONDecodeError, UnicodeDecodeError):
        return None
    advisories = data.get("advisories")
    return set(advisories.keys()) if isinstance(advisories, dict) else set()


@dataclass
class PinKept:
    name: str
    side: str  # "ours" | "theirs" | "neither" | "removed"
    reverted: bool  # the side NOT kept had already moved to a strictly higher version


def classify_kept_pins(
    merged_idx: dict, ours_idx: dict, theirs_idx: dict, divergent: set[str]
) -> list[PinKept]:
    """Per divergent package, which parent's pin the merged lock actually
    carries (`try_offline_rung`, `src/lock_merge.rs`: one attempt tries
    every divergent name against one side's own pin, falling back to the
    other side only for a name that side itself lacks -- so this reads
    the answer off the merged lock the same way `classify_resolution`
    reads a human's merge commit, rather than re-deriving which attempt
    the solver took), and whether the side NOT kept had already moved to
    a higher version -- the pin reverting a deliberate upgrade."""
    kept = []
    for name in sorted(divergent):
        m, o, t = merged_idx.get(name), ours_idx.get(name), theirs_idx.get(name)
        if m is None:
            kept.append(PinKept(name, "removed", False))
            continue
        if m == o:
            side, kept_ver, discarded_ver = "ours", o[0] if o else None, t[0] if t else None
        elif m == t:
            side, kept_ver, discarded_ver = "theirs", t[0] if t else None, o[0] if o else None
        else:
            side, kept_ver, discarded_ver = "neither", None, None
        cmp = compare_versions(discarded_ver, kept_ver)
        kept.append(PinKept(name, side, cmp is not None and cmp > 0))
    return kept


def merge_kept_side(pins: list[PinKept]) -> str:
    sides = {p.side for p in pins if p.side in ("ours", "theirs")}
    if len(sides) == 1:
        return sides.pop()
    return "mixed" if sides else "n/a"


@dataclass
class InstallCheckOutcome:
    """#314 follow-up, only set with `--install-check`, and only for a
    merge the offline pin (rung 4) finished. Counts, not text: `reason` is
    a footnote/log detail only, never quoted into the committed report."""
    kind: str
    reason: str | None
    kept_side: str  # "ours" | "theirs" | "mixed" | "n/a"
    divergent: int
    reverts: int  # divergent packages where the side NOT kept was the higher version
    kept_advisory_count: int  # kept-pin packages `viv audit --locked` flags
    kept_advisory_other_side_clean: int  # ... of those, the side NOT kept had none for that package


@dataclass
class OfflineOutcome:
    """`--offline-rung` (#314), compared against the same merge's plain
    `driver_conflict` (no flag) call. `safety_kind` is only set when both
    finished, rung 4 (the offline pin) is the one that finished the flagged run, and some
    package's identity differs between the two results -- the safety
    number's per-case classification (which parent's pin the offline rung kept, and
    whether the registry re-solve moved to an older or newer version).
    `install` is only set with `--install-check`."""
    conflicted: bool | None
    error: str | None
    rung: int | None
    time: float | None
    safety_kind: str | None = None
    install: "InstallCheckOutcome | None" = None


@dataclass
class RepoReport:
    project: Project
    skipped_reason: str | None = None
    merges: list[MergeOutcome] = field(default_factory=list)
    archaeology: list[ArchOutcome] = field(default_factory=list)
    footnotes: list[str] = field(default_factory=list)
    range_note: str = ""
    capped: bool = False


def clone_or_reuse(cache_root: Path, project: Project) -> Path:
    safe = project.name.replace("/", "_")
    dest = cache_root / safe
    if dest.exists():
        log(f"reusing clone for {project.name}")
    else:
        log(f"cloning {project.name} (blob:none, commit graph only)")
        dest.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "clone", "--quiet", "--filter=blob:none", "--no-checkout", project.repo, str(dest)],
            check=True,
        )
    return dest


def run_repo(
    project: Project, cache_root: Path, cap: int, viv_bin: str, native_available: bool,
    driver_available: bool, ledger: bool = False, hybrid: bool = False, offline_rung: bool = False,
    install_check_flag: bool = False, hang_dir: Path | None = None,
) -> RepoReport:
    report = RepoReport(project=project)
    repo_dir = clone_or_reuse(cache_root, project)

    if not git_ok(repo_dir, "cat-file", "-e", "HEAD:composer.lock"):
        report.skipped_reason = "no committed composer.lock at HEAD"
        return report

    merges, footnotes, exhausted = qualifying_merges(repo_dir, cap)
    report.footnotes.extend(footnotes)
    report.capped = not exhausted

    if not merges:
        report.range_note = "no qualifying merge found (no merge had both parents touch composer.lock)"
        return report

    report.range_note = f"{merges[-1].sha[:12]}..{merges[0].sha[:12]} ({len(merges)} qualifying merges)"

    with tempfile.TemporaryDirectory(prefix="lockmerge-") as tmp:
        work = Path(tmp)
        driver_cache = work / "driver-cache"
        for m in merges:
            base_lock = blob(repo_dir, m.base, "composer.lock")
            ours_lock = blob(repo_dir, m.ours, "composer.lock")
            theirs_lock = blob(repo_dir, m.theirs, "composer.lock")
            if base_lock is None or ours_lock is None or theirs_lock is None:
                report.footnotes.append(f"{m.sha[:12]}: composer.lock missing on one side despite earlier check")
                continue

            base_idx, ours_idx, theirs_idx = (
                lock_index(base_lock), lock_index(ours_lock), lock_index(theirs_lock)
            )
            if base_idx is None or ours_idx is None or theirs_idx is None:
                report.footnotes.append(f"{m.sha[:12]}: composer.lock did not parse as JSON on one side, skipped")
                continue
            real_names = real_conflict_names(base_idx, ours_idx, theirs_idx)
            real = len(real_names)

            if real_names:
                arch = compute_archaeology(
                    repo_dir, work, m, real_names, base_idx, ours_idx, theirs_idx, viv_bin, report.footnotes
                )
                if arch is not None:
                    report.archaeology.append(arch)

            composer_conflicts, err = merge_file_conflicts(work, ours_lock, base_lock, theirs_lock)
            if err:
                report.footnotes.append(f"{m.sha[:12]}: composer.lock merge: {err}")
                continue

            native_conflicts: int | None = None
            native_error: str | None = None
            if native_available:
                base_json = blob(repo_dir, m.base, "composer.json")
                ours_json = blob(repo_dir, m.ours, "composer.json")
                theirs_json = blob(repo_dir, m.theirs, "composer.json")
                base_native, e1 = native_lock_text(viv_bin, work, base_json, base_lock)
                ours_native, e2 = native_lock_text(viv_bin, work, ours_json, ours_lock)
                theirs_native, e3 = native_lock_text(viv_bin, work, theirs_json, theirs_lock)
                err3 = e1 or e2 or e3
                if err3:
                    native_error = err3
                else:
                    native_conflicts, err = merge_file_conflicts(work, ours_native, base_native, theirs_native)
                    if err:
                        native_error = err

            driver_conflicted: bool | None = None
            driver_err: str | None = None
            driver_rung: int | None = None
            driver_moved = 0
            driver_lock: bytes | None = None
            driver_time: float | None = None
            if driver_available:
                merge_json = blob(repo_dir, m.sha, "composer.json")
                driver_start = time.perf_counter()
                driver_conflicted, driver_err, driver_rung, driver_moved, driver_lock = driver_conflict(
                    viv_bin, work, driver_cache, repo_dir, m.sha,
                    merge_json, base_lock, ours_lock, theirs_lock, hang_dir=hang_dir,
                )
                driver_time = time.perf_counter() - driver_start
                if driver_conflicted is None:
                    report.footnotes.append(
                        f"{m.sha[:12]}: viv lock merge crashed: {driver_err or 'no stderr'}"
                    )
                elif driver_conflicted and driver_err:
                    report.footnotes.append(
                        f"{m.sha[:12]}: viv lock merge re-solve did not finish: {driver_err}"
                    )

            ledger_category: str | None = None
            hybrid_outcome: HybridOutcome | None = None
            if (ledger or hybrid) and driver_conflicted is not None:
                base_records, ours_records, theirs_records = (
                    lock_records(base_lock), lock_records(ours_lock), lock_records(theirs_lock)
                )
                ledger_category, ledger_detail = classify_ledger(
                    work, base_records, ours_records, theirs_records,
                    m.ours[:12], m.theirs[:12], real_names, driver_conflicted, driver_lock,
                )
                if ledger_category == "silent_fold":
                    report.footnotes.append(f"{m.sha[:12]}: ledger silent fold: {ledger_detail}")

                if hybrid:
                    hybrid_outcome = classify_hybrid(
                        base_records, ours_records, theirs_records,
                        m.ours[:12], m.theirs[:12], real_names, driver_conflicted, driver_lock,
                        driver_time, work,
                    )
                    if hybrid_outcome.silent:
                        report.footnotes.append(
                            f"{m.sha[:12]}: hybrid silent fold (identity hash): "
                            f"real conflict(s) folded without refusal ({len(real_names)} package(s))"
                        )

            offline_outcome: OfflineOutcome | None = None
            if offline_rung and driver_available:
                offline_start = time.perf_counter()
                off_conflicted, off_err, off_rung, _off_moved, off_lock = driver_conflict(
                    viv_bin, work, driver_cache, repo_dir, m.sha,
                    merge_json, base_lock, ours_lock, theirs_lock, offline_rung=True, hang_dir=hang_dir,
                )
                off_time = time.perf_counter() - offline_start
                if off_conflicted is None:
                    report.footnotes.append(
                        f"{m.sha[:12]}: --offline-rung crashed: {off_err or 'no stderr'}"
                    )
                elif off_conflicted and off_err:
                    report.footnotes.append(
                        f"{m.sha[:12]}: --offline-rung re-solve did not finish: {off_err}"
                    )
                safety_kind = None
                if off_rung == OFFLINE_PIN_RUNG and off_conflicted is False and driver_conflicted is False:
                    offline_idx, baseline_idx = lock_index(off_lock), lock_index(driver_lock)
                    if offline_idx is not None and baseline_idx is not None:
                        safety_kind = classify_safety(offline_idx, baseline_idx, ours_idx, theirs_idx)
                        if safety_kind:
                            report.footnotes.append(
                                f"{m.sha[:12]}: offline rung safety difference: {safety_kind}"
                            )

                install_outcome: InstallCheckOutcome | None = None
                if install_check_flag and off_rung == OFFLINE_PIN_RUNG and off_conflicted is False:
                    merged_idx = lock_index(off_lock)
                    merged_json = (
                        declare_contemporaneous_platform(merge_json, ours_lock, theirs_lock)
                        if merge_json is not None else None
                    )
                    if merged_idx is None or merged_json is None:
                        report.footnotes.append(
                            f"{m.sha[:12]}: install check skipped, merged lock/manifest unavailable"
                        )
                    else:
                        pins = classify_kept_pins(merged_idx, ours_idx, theirs_idx, real_names)
                        kind, reason = install_check(
                            viv_bin, work / "install-check" / m.sha[:12], hang_dir, m.sha,
                            merged_json, off_lock,
                        )
                        if kind != INSTALL_INSTALLS:
                            report.footnotes.append(f"{m.sha[:12]}: install check: {kind}: {reason}")

                        kept_advisory_count = kept_advisory_other_side_clean = 0
                        merged_advisories = run_audit_locked(
                            viv_bin, driver_cache, work / "audit-merged" / m.sha[:12], hang_dir,
                            m.sha, "merged", merged_json, off_lock,
                        )
                        if merged_advisories is None:
                            report.footnotes.append(f"{m.sha[:12]}: audit of the merged lock failed")
                        else:
                            kept_names = {p.name for p in pins if p.side in ("ours", "theirs")}
                            flagged = kept_names & merged_advisories
                            if flagged:
                                ours_advisories = run_audit_locked(
                                    viv_bin, driver_cache, work / "audit-ours" / m.sha[:12], hang_dir,
                                    m.sha, "ours", blob(repo_dir, m.ours, "composer.json"), ours_lock,
                                )
                                theirs_advisories = run_audit_locked(
                                    viv_bin, driver_cache, work / "audit-theirs" / m.sha[:12], hang_dir,
                                    m.sha, "theirs", blob(repo_dir, m.theirs, "composer.json"), theirs_lock,
                                )
                                if ours_advisories is None:
                                    report.footnotes.append(f"{m.sha[:12]}: audit of ours' own lock failed")
                                if theirs_advisories is None:
                                    report.footnotes.append(f"{m.sha[:12]}: audit of theirs' own lock failed")
                                for p in pins:
                                    if p.name not in flagged:
                                        continue
                                    kept_advisory_count += 1
                                    discarded = theirs_advisories if p.side == "ours" else ours_advisories
                                    if discarded is not None and p.name not in discarded:
                                        kept_advisory_other_side_clean += 1

                        install_outcome = InstallCheckOutcome(
                            kind, reason, merge_kept_side(pins), len(pins),
                            sum(1 for p in pins if p.reverted),
                            kept_advisory_count, kept_advisory_other_side_clean,
                        )
                        if install_outcome.reverts:
                            report.footnotes.append(
                                f"{m.sha[:12]}: offline pin reverted {install_outcome.reverts} "
                                "divergent package(s) past a higher discarded version"
                            )

                offline_outcome = OfflineOutcome(off_conflicted, off_err, off_rung, off_time, safety_kind, install_outcome)

            report.merges.append(MergeOutcome(
                m.sha, composer_conflicts, native_conflicts, native_error, real,
                driver_conflicted, driver_err, driver_rung, driver_moved, driver_time,
                ledger_category, hybrid_outcome, offline_outcome,
            ))

    return report


# --- report writing --------------------------------------------------


def fmt_n(v: int | None) -> str:
    return "n/a" if v is None else str(v)


def render_archaeology(items: list[ArchOutcome]) -> list[str]:
    """Four compact tables classifying the human resolutions git history
    already holds, for every merge with a real conflict, plus whether
    `viv normalize` would have prevented the source conflicts among them."""
    lines = ["### Resolution archaeology", ""]
    n = len(items)
    if n == 0:
        lines.append("No merges with real conflicts.")
        lines.append("")
        return lines

    source = sum(1 for a in items if a.source_conflict)
    lines.append("| Merges with real conflicts | Source conflict | Lock-only |")
    lines.append("|---|---|---|")
    lines.append(f"| {n} | {source} | {n - source} |")
    lines.append("")

    outcomes = [p for a in items for p in a.packages]
    ours = sum(1 for p in outcomes if p.outcome == "ours")
    theirs = sum(1 for p in outcomes if p.outcome == "theirs")
    neither = sum(1 for p in outcomes if p.outcome == "neither")
    removed = sum(1 for p in outcomes if p.outcome == "removed")
    higher = sum(1 for p in outcomes if p.higher == "higher")
    not_higher = sum(1 for p in outcomes if p.higher == "not_higher")
    na = sum(1 for p in outcomes if p.higher == "na")
    comparable = higher + not_higher
    share = f"{higher}/{comparable} ({100 * higher // comparable}%)" if comparable else "n/a"
    lines.append(
        "| Ours | Theirs | Neither | Removed | Higher version (of ours+theirs) | n/a (dev) |"
    )
    lines.append("|---|---|---|---|---|---|")
    lines.append(f"| {ours} | {theirs} | {neither} | {removed} | {share} | {na} |")
    lines.append("")

    cascades = [a.cascade for a in items]
    lines.append("| Median cascade | Max cascade |")
    lines.append("|---|---|")
    lines.append(f"| {statistics.median(cascades):g} | {max(cascades)} |")
    lines.append("")

    source_items = [a for a in items if a.source_conflict]
    remaining = sum(1 for a in source_items if a.normalized_conflict is True)
    prevented = sum(1 for a in source_items if a.normalized_conflict is False)
    norm_na = sum(1 for a in source_items if a.normalized_conflict is None)
    lines.append(
        "| Source conflicts (as committed) | Remaining after normalisation | "
        "Prevented by normalisation | n/a (normalize failed) |"
    )
    lines.append("|---|---|---|---|")
    lines.append(f"| {len(source_items)} | {remaining} | {prevented} | {norm_na} |")
    lines.append("")

    regressed = sum(1 for a in items if not a.source_conflict and a.normalized_conflict is True)
    lines.append(
        f"{regressed} lock-only merge(s) become a source conflict after normalisation."
        if regressed
        else "No lock-only merge becomes a source conflict after normalisation."
    )
    lines.append("")
    return lines


def render_ledger(merges: list[MergeOutcome]) -> list[str]:
    """#306: the ledger fold's outcome against `viv lock merge`'s own
    result on the same merge, for merges where both are known (a crashed
    driver run isn't a control and is excluded already at collection
    time, so this table's total can be smaller than "Merges examined")."""
    counted = [m for m in merges if m.ledger_category is not None]
    lines = ["### Ledger lock fold (#306)", ""]
    if not counted:
        lines.append("No merge had both a ledger fold and a driver result to compare.")
        lines.append("")
        return lines
    categories = ["identical", "silent_fold", "fold_refused_driver_ok", "both_refused", "fold_ok_driver_refused"]
    counts = {c: sum(1 for m in counted if m.ledger_category == c) for c in categories}
    lines.append(
        "| Merges examined | Identical | Silent fold | Fold refused, driver succeeded | "
        "Both refused | Fold succeeded, driver refused |"
    )
    lines.append("|---|---|---|---|---|---|")
    lines.append(
        f"| {len(counted)} | {counts['identical']} | {counts['silent_fold']} | "
        f"{counts['fold_refused_driver_ok']} | {counts['both_refused']} | "
        f"{counts['fold_ok_driver_refused']} |"
    )
    lines.append("")
    return lines


def percentile(vals: list[float], p: float) -> float:
    """Linear-interpolation percentile -- no `statistics.quantiles` edge
    cases to juggle for a corpus as small as 2 merges (koel)."""
    s = sorted(vals)
    if len(s) <= 1:
        return s[0] if s else 0.0
    k = (len(s) - 1) * p
    lo = int(k)
    hi = min(lo + 1, len(s) - 1)
    return s[lo] if lo == hi else s[lo] + (s[hi] - s[lo]) * (k - lo)


def fmt_ms(seconds: float) -> str:
    return f"{seconds * 1000:.1f}"


def render_hybrid(merges: list[MergeOutcome]) -> list[str]:
    """#306 follow-up: Option 2 (ledger union + the identity-hash fold,
    falling back to the driver when the fold refuses) against Option 1
    (the driver alone, `classify_ledger`'s own control), timed per merge.
    `classify_hybrid`'s own docstring says exactly what each timing
    includes and excludes."""
    counted = [m for m in merges if m.hybrid is not None]
    lines = ["### Hybrid fold vs driver alone (#306 follow-up)", ""]
    if not counted:
        lines.append("No merge had both a hybrid fold and a driver result to compare.")
        lines.append("")
        return lines

    n = len(counted)
    driver_only = sum(1 for m in counted if m.driver_conflict is False)
    hybrid_finished = sum(1 for m in counted if m.hybrid.finished)
    full_hash_finished = sum(
        1 for m in counted if m.ledger_category not in ("fold_refused_driver_ok", "both_refused")
    )
    identity_finished = sum(1 for m in counted if m.hybrid.identity_finished)
    silent = sum(1 for m in counted if m.hybrid.silent)
    picked = sum(1 for m in counted if m.hybrid.picked)
    ne_driver = sum(1 for m in counted if m.hybrid.ne_driver)
    metadata_diff = sum(1 for m in counted if m.hybrid.metadata_diff)

    driver_times = [m.driver_time for m in counted if m.driver_time is not None]
    hybrid_times = [m.hybrid.time for m in counted]

    lines.append(
        "| Merges examined | Finished, driver only | Finished, hybrid (identity fold) | "
        "Finished by the fold alone (full hash) | Finished by the fold alone (identity hash) | "
        "Silent fold (identity hash) | Fold picks on metadata-only difference | "
        "Hybrid result ≠ driver result (both finished) | Median ms, driver only | "
        "p95 ms, driver only | Median ms, hybrid | p95 ms, hybrid |"
    )
    lines.append("|---|---|---|---|---|---|---|---|---|---|---|---|")
    lines.append(
        f"| {n} | {driver_only} | {hybrid_finished} | {full_hash_finished} | {identity_finished} | "
        f"{silent} | {picked} | {ne_driver} | "
        f"{fmt_ms(statistics.median(driver_times)) if driver_times else 'n/a'} | "
        f"{fmt_ms(percentile(driver_times, 0.95)) if driver_times else 'n/a'} | "
        f"{fmt_ms(statistics.median(hybrid_times))} | {fmt_ms(percentile(hybrid_times, 0.95))} |"
    )
    lines.append("")
    if metadata_diff:
        lines.append(
            f"{metadata_diff} merge(s) where the hybrid and driver package identity sets "
            "agree but the full record differs (metadata only)."
        )
        lines.append("")
    return lines


def offline_bucket(m: MergeOutcome) -> str:
    """#320: which of four *mutually exclusive* outcomes a merge lands in,
    once both the plain and `--offline-rung` driver calls are in. A crash
    on either call (`driver_conflict`/`offline.conflicted` is `None`, a
    process killed or exiting by signal, not the driver's own conflict/
    success exit) always wins the classification -- we don't know what a
    crashed call would have resolved to, so it can't also count as
    "cleared", "remaining" or "clean" the way an unfiltered `is True`/
    `is False` check would let it silently do (the crash that motivated
    this: two client merges killed past a watchdog timeout, previously
    absent from every count here)."""
    if m.driver_conflict is None or m.offline.conflicted is None:
        return "crashed"
    if m.driver_conflict is True and m.offline.conflicted is False:
        return "cleared"
    if m.offline.conflicted is True:
        return "remaining"
    return "clean"  # neither side ever conflicted, nothing for the flag to clear


def render_offline(merges: list[MergeOutcome]) -> list[str]:
    """#314: `--offline-rung` against the same merge's plain (no-flag)
    driver call. Residue cleared (a merge that used to end in markers and
    now doesn't, by the leaf cause of the marker it cleared), residue
    remaining (still markers, same leaf-cause buckets), crashed (#320:
    either call was killed or exited by signal, so the merge is its own
    outcome rather than silently missing from every other count), and
    clean (neither call ever saw a conflict) partition every counted merge
    exactly once (`offline_bucket`), so the four sum to merges examined.
    Alongside: the safety number (rung 4, the offline pin, accepted a pin the registry
    re-solve, when it also finished, would have picked differently) and
    network avoided (rung 4 finished a merge that used to need an
    escalation rung, no registry contact for it this time) -- both
    cross-cutting flags, not additional outcomes -- plus the median time
    both ways."""
    counted = [m for m in merges if m.offline is not None]
    lines = ["### Offline rung vs registry escalation (#314)", ""]
    if not counted:
        lines.append("No merge had both an `--offline-rung` and a plain driver result to compare.")
        lines.append("")
        return lines

    n = len(counted)
    cleared = [m for m in counted if offline_bucket(m) == "cleared"]
    remaining = [m for m in counted if offline_bucket(m) == "remaining"]
    crashed = [m for m in counted if offline_bucket(m) == "crashed"]
    clean = [m for m in counted if offline_bucket(m) == "clean"]
    network_avoided = [
        m for m in counted
        if m.offline.rung == OFFLINE_PIN_RUNG and m.driver_conflict is False and m.driver_rung is not None
    ]
    safety_cases = [m for m in counted if m.offline.safety_kind]

    def leaf_table(items: list[MergeOutcome], reason_of) -> list[str]:
        if not items:
            return ["None.", ""]
        counts: dict[str, int] = {}
        for m in items:
            counts[leaf_cause_class(reason_of(m))] = counts.get(leaf_cause_class(reason_of(m)), 0) + 1
        out = ["| Leaf cause | Merges |", "|---|---|"]
        for cause, c in sorted(counts.items(), key=lambda kv: -kv[1]):
            out.append(f"| {cause} | {c} |")
        out.append("")
        return out

    driver_times = [m.driver_time for m in counted if m.driver_time is not None]
    offline_times = [m.offline.time for m in counted if m.offline.time is not None]

    lines.append(
        "| Merges examined | Residue cleared | Residue remaining | Crashed | Clean | "
        "Safety number | Network avoided | Median ms, no flag | Median ms, --offline-rung |"
    )
    lines.append("|---|---|---|---|---|---|---|---|---|")
    lines.append(
        f"| {n} | {len(cleared)} | {len(remaining)} | {len(crashed)} | {len(clean)} | "
        f"{len(safety_cases)} | {len(network_avoided)} | "
        f"{fmt_ms(statistics.median(driver_times)) if driver_times else 'n/a'} | "
        f"{fmt_ms(statistics.median(offline_times)) if offline_times else 'n/a'} |"
    )
    lines.append("")

    lines.append("Residue cleared, by the leaf cause it cleared:")
    lines.append("")
    lines.extend(leaf_table(cleared, lambda m: m.driver_error))
    lines.append("Residue remaining, by leaf cause:")
    lines.append("")
    lines.extend(leaf_table(remaining, lambda m: m.offline.error))
    lines.append("Crashed, by reason (either the plain or the `--offline-rung` call):")
    lines.append("")
    lines.extend(leaf_table(crashed, lambda m: m.driver_error if m.driver_conflict is None else m.offline.error))

    if safety_cases:
        kind_counts: dict[str, int] = {}
        for m in safety_cases:
            for kind in m.offline.safety_kind.split("; "):
                kind_counts[kind] = kind_counts.get(kind, 0) + 1
        lines.append(f"Safety number: {len(safety_cases)} merge(s), by kind:")
        lines.append("")
        lines.append("| Kind | Count |")
        lines.append("|---|---|")
        for kind, c in sorted(kind_counts.items(), key=lambda kv: -kv[1]):
            lines.append(f"| {kind} | {c} |")
        lines.append("")
    else:
        lines.append("Safety number: 0. No merge where the offline pin (rung 4) and the registry's own "
                      "re-solve, when both finished, chose a different package identity.")
        lines.append("")
    return lines


def render_install_check(merges: list[MergeOutcome]) -> list[str]:
    """#314 follow-up, only with `--install-check`: of the merges the
    offline pin finished (rung 4), does the merged lock actually install,
    and what did keeping its pins cost -- a security advisory the
    discarded side didn't carry, or reverting a divergent package past a
    higher version the discarded side had already reached. Counts and
    kind labels only; `InstallCheckOutcome.reason` never appears here (it
    is a footnote/log detail, per this repo's client-privacy rule)."""
    counted = [m for m in merges if m.offline is not None and m.offline.install is not None]
    lines = ["### Offline pin install check (#314 follow-up)", ""]
    if not counted:
        lines.append("No merge had an `--install-check` result (none of the offline-rung "
                      "merges in this run reached rung 4, the offline pin).")
        lines.append("")
        return lines

    n = len(counted)
    kinds = [
        INSTALL_INSTALLS, INSTALL_DIST_GONE, INSTALL_SOURCE_REF_GONE, INSTALL_SHASUM_MISMATCH,
        INSTALL_OTHER, INSTALL_CRASHED,
    ]
    counts = {k: sum(1 for m in counted if m.offline.install.kind == k) for k in kinds}
    lines.append(
        "| Finished by the pin | Installs | Dist gone | Source ref gone | Shasum mismatch | Other | Crashed |"
    )
    lines.append("|---|---|---|---|---|---|---|")
    lines.append(
        f"| {n} | {counts[INSTALL_INSTALLS]} | {counts[INSTALL_DIST_GONE]} | "
        f"{counts[INSTALL_SOURCE_REF_GONE]} | {counts[INSTALL_SHASUM_MISMATCH]} | {counts[INSTALL_OTHER]} | "
        f"{counts[INSTALL_CRASHED]} |"
    )
    lines.append("")

    advisory_merges = sum(1 for m in counted if m.offline.install.kept_advisory_count > 0)
    advisory_packages = sum(m.offline.install.kept_advisory_count for m in counted)
    other_side_clean_packages = sum(m.offline.install.kept_advisory_other_side_clean for m in counted)
    lines.append(
        f"Advisory count: {advisory_merges} merge(s) where a kept pin carries a `viv audit "
        f"--locked` advisory ({advisory_packages} package(s) total; {other_side_clean_packages} "
        "of those had no advisory on the side not kept)."
    )
    lines.append("")

    revert_merges = sum(1 for m in counted if m.offline.install.reverts > 0)
    revert_packages = sum(m.offline.install.reverts for m in counted)
    lines.append(
        f"Revert count: {revert_merges} merge(s) with at least one divergent package where the "
        f"side not kept had already moved to a higher version ({revert_packages} package(s) total)."
    )
    lines.append("")

    sides = ["ours", "theirs", "mixed", "n/a"]
    side_counts = {s: sum(1 for m in counted if m.offline.install.kept_side == s) for s in sides}
    lines.append("| Sides kept | Ours | Theirs | Mixed | n/a (no divergent package resolved) |")
    lines.append("|---|---|---|---|---|")
    lines.append(
        f"| {n} | {side_counts['ours']} | {side_counts['theirs']} | {side_counts['mixed']} | {side_counts['n/a']} |"
    )
    lines.append("")
    return lines


def render(
    reports: list[RepoReport], cap: int, viv_bin: str, viv_version: str,
    native_available: bool, viv_commit: str | None, driver_available: bool,
    ledger: bool = False, hybrid: bool = False, offline_rung: bool = False,
    install_check_flag: bool = False,
) -> str:
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    commit_note = f", commit `{viv_commit}`" if viv_commit else ""
    lines = [
        "",
        f"## {now}",
        "",
        f"Cap: {cap} most recent qualifying merges per repository. "
        f"viv binary: `{viv_bin}` ({viv_version}{commit_note}).",
    ]
    if not native_available:
        lines.append(
            "`viv lock convert` is not available on this binary; the viv.lock "
            "column reads n/a throughout this run (#273's harness runs ahead "
            "of the subcommand landing)."
        )
    if not driver_available:
        lines.append(
            "`viv lock merge` is not available on this binary; the viv lock "
            "merge column reads n/a throughout this run."
        )
    lines.append("")

    total_merges = total_composer = total_native = total_real = total_wins = 0
    total_composer_conflicting = total_native_conflicting = total_driver_conflicting = 0
    total_driver_crashed = 0
    native_seen_anywhere = driver_seen_anywhere = False
    total_rung_counts: dict[int, int] = {}
    total_moved_merges = 0

    def rung_line(merges: list[MergeOutcome]) -> str | None:
        """#296: how many merges each rung resolved, and how many moved a
        package outside the divergent set -- `None` when this repo has no
        driver data at all (skipped, or `viv lock merge` unavailable)."""
        rungs = [m.driver_rung for m in merges if m.driver_rung is not None]
        if not rungs:
            return None
        counts = {n: rungs.count(n) for n in sorted(set(rungs))}
        names = {1: "closure", 2: "dependents", 3: "seeded"}
        rung_text = ", ".join(f"rung {n} ({names.get(n, n)}): {c}" for n, c in counts.items())
        moved_merges = sum(1 for m in merges if m.driver_moved > 0)
        return f"Resolved {rung_text}. {moved_merges} merge(s) moved ≥1 package outside the divergent set."

    header = (
        "| Merges examined | Merges conflicting (composer.lock) | "
        "Merges conflicting (viv.lock) | Merges conflicting (viv lock merge) | "
        "Crashed (viv lock merge) | "
        "Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | "
        "composer.lock conflicted, viv.lock did not |"
    )
    separator = "|---|---|---|---|---|---|---|---|---|"

    for r in reports:
        lines.append(f"### {r.project.name}")
        lines.append("")
        if r.skipped_reason:
            lines.append(f"Skipped: {r.skipped_reason}.")
            lines.append("")
            continue
        lines.append(f"Examined `{r.range_note}`" + (" -- capped." if r.capped else "."))
        lines.append("")
        lines.append(header)
        lines.append(separator)

        n = len(r.merges)
        composer_sum = sum(m.composer_conflicts or 0 for m in r.merges)
        composer_conflicting = sum(1 for m in r.merges if m.composer_conflicts and m.composer_conflicts > 0)
        native_vals = [m.native_conflicts for m in r.merges if m.native_conflicts is not None]
        native_sum = sum(native_vals) if native_vals else None
        native_conflicting = sum(1 for v in native_vals if v > 0) if native_vals else None
        real_sum = sum(m.real for m in r.merges)
        wins = sum(
            1 for m in r.merges
            if m.composer_conflicts and m.composer_conflicts > 0 and m.native_conflicts == 0
        )
        wins_display = wins if native_vals else None
        # `driver_available` (not "any merge happened to return non-None"),
        # so a repo where the driver crashed on every merge still reports 0
        # conflicting/n crashed rather than falling back to n/a as if the
        # driver had never run at all (#320).
        driver_conflicting = sum(1 for m in r.merges if m.driver_conflict is True) if driver_available else None
        driver_crashed = sum(1 for m in r.merges if m.driver_conflict is None) if driver_available else None

        lines.append(
            f"| {n} | {composer_conflicting} | {fmt_n(native_conflicting)} | "
            f"{fmt_n(driver_conflicting)} | {fmt_n(driver_crashed)} | "
            f"{composer_sum} | {fmt_n(native_sum)} | {real_sum} | {fmt_n(wins_display)} |"
        )
        lines.append("")

        repo_rung_line = rung_line(r.merges)
        if repo_rung_line:
            lines.append(repo_rung_line)
            lines.append("")
            for m in r.merges:
                if m.driver_rung is not None:
                    total_rung_counts[m.driver_rung] = total_rung_counts.get(m.driver_rung, 0) + 1
                if m.driver_moved > 0:
                    total_moved_merges += 1

        total_merges += n
        total_composer += composer_sum
        total_composer_conflicting += composer_conflicting
        if native_vals:
            native_seen_anywhere = True
            total_native += native_sum
            total_native_conflicting += native_conflicting
            total_wins += wins
        if driver_available:
            driver_seen_anywhere = True
            total_driver_conflicting += driver_conflicting
            total_driver_crashed += driver_crashed
        total_real += real_sum

        for f in r.footnotes:
            lines.append(f"- {f}")
        if r.footnotes:
            lines.append("")

        lines.extend(render_archaeology(r.archaeology))
        if ledger:
            lines.extend(render_ledger(r.merges))
        if hybrid:
            lines.extend(render_hybrid(r.merges))

    lines.append("### Totals")
    lines.append("")
    lines.append(header)
    lines.append(separator)
    lines.append(
        f"| {total_merges} | {total_composer_conflicting} | "
        f"{fmt_n(total_native_conflicting) if native_seen_anywhere else 'n/a'} | "
        f"{fmt_n(total_driver_conflicting) if driver_seen_anywhere else 'n/a'} | "
        f"{fmt_n(total_driver_crashed) if driver_seen_anywhere else 'n/a'} | "
        f"{total_composer} | "
        f"{fmt_n(total_native) if native_seen_anywhere else 'n/a'} | {total_real} | "
        f"{fmt_n(total_wins) if native_seen_anywhere else 'n/a'} |"
    )
    lines.append("")
    if total_rung_counts:
        names = {1: "closure", 2: "dependents", 3: "seeded"}
        rung_text = ", ".join(
            f"rung {n} ({names.get(n, n)}): {c}" for n, c in sorted(total_rung_counts.items())
        )
        lines.append(
            f"Resolved {rung_text}. {total_moved_merges} merge(s) moved "
            "≥1 package outside the divergent set."
        )
        lines.append("")

    all_archaeology = [a for r in reports for a in r.archaeology]
    lines.extend(render_archaeology(all_archaeology))
    if ledger:
        all_merges = [m for r in reports for m in r.merges]
        lines.extend(render_ledger(all_merges))
    if hybrid:
        all_merges = [m for r in reports for m in r.merges]
        lines.extend(render_hybrid(all_merges))
    if offline_rung:
        all_merges = [m for r in reports for m in r.merges]
        lines.extend(render_offline(all_merges))
    if install_check_flag:
        all_merges = [m for r in reports for m in r.merges]
        lines.extend(render_install_check(all_merges))
    return "\n".join(lines)


HEADER = """# Merge replay: composer.lock vs viv.lock (#273)

Chapter 1's measurement (`docs/research.md`): for every merge commit in a
corpus repo's history whose two parents both changed `composer.lock`
relative to their merge base, replay the three-way merge under
`composer.lock` as committed and under the native `viv.lock` conversion
(#272), and count textual conflicts (`git merge-file`'s exit code) under
each, plus real conflicts (packages both sides actually changed to
different results, computed from the JSON, format-independent -- the
control for whether a format's win was a real one).

Corpus and the range actually swept: `bench/lockmerge/corpus.toml`. Harness:
`bench/lockmerge/run.py`.
"""


def main() -> int:
    argv = sys.argv[1:]
    hybrid = "--hybrid" in argv  # #306 follow-up: adds the hybrid-vs-driver-alone table, implies --ledger
    ledger = "--ledger" in argv or hybrid  # #306: adds the ledger-fold-vs-driver counts to the report
    offline_rung = "--offline-rung" in argv  # #314: runs the driver twice, with and without the flag
    install_check_flag = "--install-check" in argv  # #314 follow-up: install-checks every merge the pin finished
    dev_as_commits = "--dev-as-commits" in argv  # #331 candidate 3.3: dev-* records merge by commit, never re-solved
    only_conflicting_path: Path | None = None
    if "--only-conflicting" in argv:  # #331: restricts the replay to a prior run's own conflicting merges
        idx = argv.index("--only-conflicting")
        only_conflicting_path = Path(argv[idx + 1])
        argv = argv[:idx] + argv[idx + 2 :]
    argv = [
        a for a in argv
        if a not in ("--ledger", "--hybrid", "--offline-rung", "--install-check", "--dev-as-commits")
    ]
    if argv and argv[0] == "--self-test":
        return self_test()
    only = argv[0].split(",") if argv else []

    corpus_path = Path(os.environ.get("LOCKMERGE_CORPUS", ROOT / "bench/lockmerge/corpus.toml"))
    report_path = Path(os.environ.get("LOCKMERGE_REPORT", ROOT / "bench/results/lockmerge.md"))
    cache_root = Path(
        os.environ.get("BENCH_CACHE", Path(os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")) / "vivace-bench")
    ) / "lockmerge"
    cache_root.mkdir(parents=True, exist_ok=True)
    cap = int(os.environ.get("LOCKMERGE_CAP", "200"))
    viv_bin = os.environ.get("VIV", str(ROOT / "target/release/viv"))
    hang_dir = Path(os.environ["LOCKMERGE_HANG_DIR"]) if os.environ.get("LOCKMERGE_HANG_DIR") else None

    native_available = probe_native(viv_bin)
    viv_version = "unknown"
    v = subprocess.run([viv_bin, "--version"], capture_output=True, text=True)
    if v.returncode == 0:
        viv_version = v.stdout.strip()
    viv_commit = viv_bin_commit(viv_bin)
    log(f"native (viv lock convert) available: {native_available}")
    driver_available = probe_driver(viv_bin)
    log(f"driver (viv lock merge) available: {driver_available}")

    projects = parse_corpus(corpus_path)
    if only:
        projects = [p for p in projects if p.name in only]

    if dev_as_commits:
        if only_conflicting_path is None:
            log("--dev-as-commits requires --only-conflicting <path>")
            return 1
        only_map = parse_only_conflicting(only_conflicting_path)
        start = time.perf_counter()
        outcomes = run_dev_as_commits(projects, cache_root, viv_bin, only_map, cap, hang_dir)
        wall_time = time.perf_counter() - start
        section = render_dev_as_commits(
            outcomes, cap, viv_bin, viv_version, viv_commit, only_conflicting_path, wall_time
        )
        if not report_path.exists():
            report_path.write_text(HEADER)
        with open(report_path, "a") as f:
            f.write(section)
        log(f"report written to {report_path}, wall time {wall_time:.1f}s")
        return 0

    reports = []
    for project in projects:
        log(f"{project.name}: starting")
        r = run_repo(
            project, cache_root, cap, viv_bin, native_available, driver_available,
            ledger, hybrid, offline_rung, install_check_flag, hang_dir,
        )
        reports.append(r)
        if r.skipped_reason:
            update_corpus_note(corpus_path, project.name, f'skip = "{r.skipped_reason}"')
        else:
            update_corpus_note(corpus_path, project.name, f'examined = "{r.range_note}"')

    section = render(
        reports, cap, viv_bin, viv_version, native_available, viv_commit, driver_available,
        ledger, hybrid, offline_rung, install_check_flag,
    )
    if not report_path.exists():
        report_path.write_text(HEADER)
    with open(report_path, "a") as f:
        f.write(section)
    log(f"report written to {report_path}")
    return 0


# --- self-test --------------------------------------------------


def self_test() -> int:
    """Exercises the classifier and the merge-file wrapper against a small
    synthetic repo, without network -- no corpus clone needed."""
    with tempfile.TemporaryDirectory(prefix="lockmerge-selftest-") as tmp:
        repo = Path(tmp) / "repo"
        repo.mkdir()
        git(repo, "init", "--quiet", "-b", "main")
        git(repo, "config", "user.email", "test@example.com")
        git(repo, "config", "user.name", "Test")

        def lock(pkgs):
            return json.dumps({"packages": pkgs, "packages-dev": []}, indent=4).encode() + b"\n"

        def commit(msg, pkgs, composer_json=None):
            if composer_json is not None:
                (repo / "composer.json").write_bytes(composer_json)
                git(repo, "add", "composer.json")
            (repo / "composer.lock").write_bytes(lock(pkgs))
            git(repo, "add", "composer.lock")
            git(repo, "commit", "--quiet", "-m", msg)
            return git(repo, "rev-parse", "HEAD").stdout.strip()

        # base's require is unsorted (vvv/v before bbb/b); ours2 and theirs2
        # each add one dependency below, colliding on the same line as
        # committed -- and viv normalize's key sort separates them (table 4).
        base_composer_json = (
            b'{\n    "name": "t/t",\n    "require": {\n'
            b'        "vvv/v": "^1.0",\n        "bbb/b": "^1.0"\n    }\n}\n'
        )
        (repo / "composer.json").write_bytes(base_composer_json)
        git(repo, "add", "composer.json")

        base_pkgs = [
            {"name": "a/a", "version": "1.0.0", "source": {"reference": "aaa"}},
            {"name": "b/b", "version": "1.0.0", "source": {"reference": "bbb"}},
        ]
        commit("base", base_pkgs)
        git(repo, "checkout", "--quiet", "-b", "ours")
        ours_pkgs = [
            {"name": "a/a", "version": "1.1.0", "source": {"reference": "aa1"}},
            {"name": "b/b", "version": "1.0.0", "source": {"reference": "bbb"}},
        ]
        commit("ours changes a", ours_pkgs)
        git(repo, "checkout", "--quiet", "main")
        git(repo, "checkout", "--quiet", "-b", "theirs")
        theirs_pkgs = [
            {"name": "a/a", "version": "1.0.0", "source": {"reference": "aaa"}},
            {"name": "b/b", "version": "2.0.0", "source": {"reference": "bb2"}},
        ]
        commit("theirs changes b", theirs_pkgs)
        git(repo, "checkout", "--quiet", "main")
        git(repo, "merge", "--quiet", "--no-ff", "ours", "theirs", "-m", "octopus")
        # octopus (3 parents) must not qualify; add a real two-parent merge too.
        git(repo, "checkout", "--quiet", "-b", "ours2", "ours")
        # both sides change a/a and b/b to different versions -> two real
        # conflicts, one to resolve each way below.
        conflict_pkgs = [
            {"name": "a/a", "version": "1.2.0", "source": {"reference": "aa2"}},
            {"name": "b/b", "version": "1.1.0", "source": {"reference": "bb1"}},
        ]
        ours_composer_json = (
            b'{\n    "name": "t/t",\n    "require": {\n        "aaa/a": "^1.0",\n'
            b'        "vvv/v": "^1.0",\n        "bbb/b": "^1.0"\n    }\n}\n'
        )
        commit("ours2 bumps a and b", conflict_pkgs, ours_composer_json)
        git(repo, "checkout", "--quiet", "-b", "theirs2", "theirs")
        theirs2_pkgs = [
            {"name": "a/a", "version": "1.3.0", "source": {"reference": "aa3"}},
            {"name": "b/b", "version": "2.0.1", "source": {"reference": "bb3"}},
        ]
        theirs_composer_json = (
            b'{\n    "name": "t/t",\n    "require": {\n        "zzz/z": "^1.0",\n'
            b'        "vvv/v": "^1.0",\n        "bbb/b": "^1.0"\n    }\n}\n'
        )
        commit("theirs2 bumps b further", theirs2_pkgs, theirs_composer_json)
        git(repo, "checkout", "--quiet", "main")
        merge_sha_before = git(repo, "rev-parse", "HEAD").stdout.strip()
        git(repo, "checkout", "--quiet", "-b", "merge-target", "ours2")
        r = subprocess.run(["git", "merge", "--no-ff", "-m", "two-parent merge", "theirs2"], cwd=repo, capture_output=True)
        # composer.json conflicts too (both add a line at the same spot);
        # its resolved content doesn't matter to archaeology, which reads
        # only each side's blob, not the merge's own.
        if (repo / "composer.json").exists():
            git(repo, "checkout", "--quiet", "--ours", "composer.json")
            git(repo, "add", "composer.json")
        # a real merge conflict is expected on composer.lock; resolve it the
        # way a human would -- keep ours for a/a, take theirs (the higher
        # version) for b/b -- so archaeology has one of each to classify.
        if (repo / "composer.lock").exists():
            resolved_pkgs = [
                {"name": "a/a", "version": "1.2.0", "source": {"reference": "aa2"}},  # ours
                {"name": "b/b", "version": "2.0.1", "source": {"reference": "bb3"}},  # theirs, higher
            ]
            (repo / "composer.lock").write_bytes(lock(resolved_pkgs))
            git(repo, "add", "composer.lock")
        subprocess.run(["git", "commit", "--quiet", "--no-edit"], cwd=repo, capture_output=True)

        merges, footnotes, exhausted = qualifying_merges(repo, cap=10)
        two_parent = [m for m in merges if m.sha != merge_sha_before]
        assert len(two_parent) >= 1, f"expected the two-parent merge to qualify, got {merges}"
        m = two_parent[0]

        base_lock = blob(repo, m.base, "composer.lock")
        ours_lock = blob(repo, m.ours, "composer.lock")
        theirs_lock = blob(repo, m.theirs, "composer.lock")
        base_idx, ours_idx, theirs_idx = lock_index(base_lock), lock_index(ours_lock), lock_index(theirs_lock)
        real_names = real_conflict_names(base_idx, ours_idx, theirs_idx)
        real = len(real_names)
        assert real_names == {"a/a", "b/b"}, f"expected a/a and b/b as real conflicts, got {real_names}"

        work = Path(tmp) / "work"
        work.mkdir()

        viv_bin = os.environ.get("VIV", str(ROOT / "target/release/viv"))
        arch = compute_archaeology(repo, work, m, real_names, base_idx, ours_idx, theirs_idx, viv_bin, [])
        assert arch is not None, "expected archaeology to classify the resolved merge"
        assert arch.source_conflict is True, "unsorted require additions should collide as committed"
        by_name = {p.name: p for p in arch.packages}
        assert by_name["a/a"].outcome == "ours", f"expected a/a resolved to ours, got {by_name['a/a']}"
        assert by_name["b/b"].outcome == "theirs" and by_name["b/b"].higher == "higher", (
            f"expected b/b resolved to theirs and higher, got {by_name['b/b']}"
        )
        assert arch.cascade == 0, f"expected no dragged-along packages, got {arch.cascade}"
        assert arch.normalize_error is None, f"expected viv normalize to succeed, got {arch.normalize_error}"
        assert arch.normalized_conflict is False, "sorted require keys should separate the two additions"
        assert compare_versions("1.10.0", "1.9.0") == 1, "1.10.0 should compare above 1.9.0 numerically"
        assert compare_versions("dev-main", "1.0.0") is None, "dev branches are incomparable"
        conflicts, err = merge_file_conflicts(work, ours_lock, base_lock, theirs_lock)
        assert err is None, err
        assert conflicts is not None and conflicts >= 1, f"expected a textual conflict on the shared array, got {conflicts}"
        # the single two-parent merge conflicts, so exactly 1 merge conflicting.
        assert sum(1 for c in [conflicts] if c > 0) == 1, "expected the conflicting merge to count as 1"

        # a clean three-way merge (disjoint changes) should report 0.
        clean_conflicts, err = merge_file_conflicts(
            work, lock(ours_pkgs), lock(base_pkgs), lock(theirs_pkgs)
        )
        assert err is None and clean_conflicts == 0, f"expected a clean merge, got {clean_conflicts}/{err}"
        assert sum(1 for c in [clean_conflicts] if c > 0) == 0, "expected the clean merge to count as 0"

        assert real_conflicts(
            {"x": ("1.0", None)}, {"x": ("1.0", None)}, {"x": ("2.0", None)}
        ) == 0, "one-sided change is not a real conflict"

        # #296: parse_resolution reads viv lock merge's own "say what
        # moved" lines off stderr.
        rung, moved = parse_resolution(
            "viv lock merge: resolved via rung 2 (dependents): d/dep\n"
            "viv lock merge: e/dependent moved outside the divergent set: 1.0.0 -> 2.0.0\n"
        )
        assert (rung, moved) == (2, 1), f"expected rung 2 with 1 moved package, got {(rung, moved)}"
        assert parse_resolution("") == (None, 0), "no stderr means no rung to report"
        # #314: rung 4 (--offline-rung's own pin, `OFFLINE_PIN_RUNG` in
        # `src/lock_merge.rs`) reads through the same line.
        assert parse_resolution(
            "viv lock merge: resolved via rung 4 (offline_pin): d/dep\n"
        ) == (4, 0), "rung 4 must parse the same as any other rung"

        # #314: leaf_cause_class buckets a driver_error the same way
        # bench/results/lockmerge.md's existing leaf-cause table does.
        assert leaf_cause_class(
            "- roave/security-advisories dev-latest conflicts with wp-coding-standards/wpcs 2.3.0."
        ) == LEAF_CAUSE_DEV_HEAD
        assert leaf_cause_class(
            "- Root composer.json requires x/y, it could not be found in any version, there may "
            "be a typo in the package name."
        ) == LEAF_CAUSE_PACKAGE_GONE
        assert leaf_cause_class("parsing composer.json: trailing comma at line 1") == (
            LEAF_CAUSE_MALFORMED_MANIFEST
        )
        assert leaf_cause_class("some other reason entirely") == LEAF_CAUSE_OTHER
        assert leaf_cause_class(None) == LEAF_CAUSE_OTHER

        # #314: classify_safety agrees when both sides resolved the same
        # package identically, and names the parent and direction when a
        # divergent package's pin differs from the registry's own pick.
        ours_idx = {"d/dep": ("1.0.0", "aaa", False)}
        theirs_idx = {"d/dep": ("2.0.0", "bbb", False)}
        assert classify_safety(
            {"d/dep": ("1.0.0", "aaa", False)}, {"d/dep": ("1.0.0", "aaa", False)}, ours_idx, theirs_idx,
        ) is None, "identical resolutions must not count against the safety number"
        kind = classify_safety(
            {"d/dep": ("1.0.0", "aaa", False)}, {"d/dep": ("2.0.0", "bbb", False)}, ours_idx, theirs_idx,
        )
        assert kind == "ours, older pin kept", f"expected ours/older, got {kind}"

        # #320: a driver that exits by signal (killed, not its own
        # conflict/success exit) must report itself as a crash rather than
        # matching neither the `is True` nor `is False` check downstream --
        # a fake `viv` that answers --help/--version but kills itself on
        # the real invocation, so this needs no real binary.
        crash_script = Path(tmp) / "fake-viv-crash.sh"
        crash_script.write_text(
            "#!/bin/sh\n"
            "for a in \"$@\"; do\n"
            "  case \"$a\" in\n"
            "    --help|--version) exit 0 ;;\n"
            "  esac\n"
            "done\n"
            "kill -9 $$\n"
        )
        crash_script.chmod(0o755)
        crash_conflicted, crash_err, crash_rung, crash_moved, crash_lock = driver_conflict(
            str(crash_script), work, work / "cache", repo, "deadbeef",
            b'{"require": {}}', b"{}", b"{}", b"{}",
        )
        assert crash_conflicted is None, f"a killed driver must report conflicted=None, got {crash_conflicted}"
        assert crash_err and "crashed" in crash_err, f"expected a crash reason, got {crash_err!r}"
        assert crash_rung is None and crash_moved == 0 and crash_lock is None

        # #320: render_offline's own table must give a crashed merge (on
        # either the plain or the --offline-rung call) its own bucket
        # rather than drop it from every count -- cleared + remaining +
        # crashed + clean must sum to merges examined.
        def offline_merge(sha: str, driver_conflict_val, off_conflicted, off_rung=None) -> MergeOutcome:
            return MergeOutcome(
                sha=sha, composer_conflicts=0, native_conflicts=None, native_error=None,
                real=0, driver_conflict=driver_conflict_val, driver_error="a driver reason",
                offline=OfflineOutcome(off_conflicted, "an offline reason", off_rung, 0.01),
            )

        synthetic = [
            offline_merge("m1", True, False, off_rung=0),  # cleared
            offline_merge("m2", True, True),  # remaining
            offline_merge("m3", False, False),  # clean, no residue either way
            offline_merge("m4", None, True),  # plain driver call crashed
            offline_merge("m5", False, None),  # --offline-rung call crashed
        ]
        assert [offline_bucket(m) for m in synthetic] == [
            "cleared", "remaining", "clean", "crashed", "crashed",
        ], "offline_bucket must classify each synthetic case as intended"
        offline_report = "\n".join(render_offline(synthetic))
        row = next(l for l in offline_report.splitlines() if l.startswith("| 5 |"))
        cells = [int(c.strip()) for c in row.strip("|").split("|")[:5]]
        assert cells == [5, 1, 1, 2, 1], f"expected 5 examined, 1 cleared, 1 remaining, 2 crashed, 1 clean, got {cells}"
        assert sum(cells[1:]) == cells[0], "offline table's outcome buckets must sum to merges examined"

    # #306: ledger fold unit cases -- disjoint changes, agreement, a real
    # fork, and delete-vs-update, plus order-independence and an
    # unparseable line, all without a repo or a viv binary.
    with tempfile.TemporaryDirectory(prefix="lockmerge-ledger-selftest-") as tmp:
        work = Path(tmp)
        pkg_a = {"name": "a/a", "version": "1.0.0", "source": {"reference": "aaa"}}
        pkg_b = {"name": "b/b", "version": "1.0.0", "source": {"reference": "bbb"}}
        base_records = {"packages/a": pkg_a, "packages/b": pkg_b}
        base_lines = base_ledger_lines(base_records)

        def bump(pkg: dict, version: str, reference: str) -> dict:
            return {**pkg, "version": version, "source": {"reference": reference}}

        # disjoint changes -> identical: each side's own change applies, no fork.
        ours = {"packages/a": bump(pkg_a, "1.1.0", "aa1"), "packages/b": pkg_b}
        theirs = {"packages/a": pkg_a, "packages/b": bump(pkg_b, "2.0.0", "bb2")}
        ours_lines = parent_ledger_lines(base_records, ours, "c0ffee1")
        theirs_lines = parent_ledger_lines(base_records, theirs, "c0ffee2")
        union_text, err = union_merge_ledger(work, ours_lines, base_lines, theirs_lines)
        assert err is None, err
        folded, fold_err, picked = fold_ledger(union_text.splitlines())
        assert fold_err is None, fold_err
        assert not picked, "disjoint changes never need the tie-break"
        assert folded["packages/a"]["version"] == "1.1.0"
        assert folded["packages/b"]["version"] == "2.0.0"
        shuffled, shuffled_err, _ = fold_ledger(list(reversed(union_text.splitlines())))
        assert shuffled_err is None and shuffled == folded, "fold must not depend on line order"

        # same package, same change, both sides -> agreement, not a fork.
        agree = {"packages/a": bump(pkg_a, "1.1.0", "aa1"), "packages/b": pkg_b}
        ours_lines = parent_ledger_lines(base_records, agree, "c0ffee1")
        theirs_lines = parent_ledger_lines(base_records, agree, "c0ffee2")
        union_text, err = union_merge_ledger(work, ours_lines, base_lines, theirs_lines)
        assert err is None, err
        assert len(union_text.splitlines()) == len(base_lines) + 2, (
            "the cause token must keep the two sides' identical change on two distinct lines"
        )
        folded, fold_err, _ = fold_ledger(union_text.splitlines())
        assert fold_err is None and folded["packages/a"]["version"] == "1.1.0"

        # same package, different change, both sides -> fork, fold refuses.
        ours = {"packages/a": bump(pkg_a, "1.1.0", "aa1"), "packages/b": pkg_b}
        theirs = {"packages/a": bump(pkg_a, "1.2.0", "aa2"), "packages/b": pkg_b}
        ours_lines = parent_ledger_lines(base_records, ours, "c0ffee1")
        theirs_lines = parent_ledger_lines(base_records, theirs, "c0ffee2")
        union_text, err = union_merge_ledger(work, ours_lines, base_lines, theirs_lines)
        assert err is None, err
        folded, fold_err, _ = fold_ledger(union_text.splitlines())
        assert folded is None and fold_err is not None, "a genuine fork must refuse"

        # one side deletes, the other updates the same package -> refuse.
        ours = {"packages/b": pkg_b}  # a/a deleted
        theirs = {"packages/a": bump(pkg_a, "1.1.0", "aa1"), "packages/b": pkg_b}
        ours_lines = parent_ledger_lines(base_records, ours, "c0ffee1")
        theirs_lines = parent_ledger_lines(base_records, theirs, "c0ffee2")
        union_text, err = union_merge_ledger(work, ours_lines, base_lines, theirs_lines)
        assert err is None, err
        folded, fold_err, _ = fold_ledger(union_text.splitlines())
        assert folded is None and fold_err is not None, "delete-vs-update must refuse"

        # unparseable text (union having mangled a line) refuses too.
        folded, fold_err, _ = fold_ledger(["not a ledger line"])
        assert folded is None and fold_err is not None, "an unparseable line must refuse"

        # #306 follow-up, identity-hash variant: both sides move a/a to the
        # same version and reference, but one record carries an extra
        # metadata field the other lacks -- the false-fork case the first
        # candidate-A measurement found in 14 client/public merges. The
        # full-record hash above still forks on this; the identity hash
        # must agree, with a deterministic pick recorded.
        base_id_lines = base_ledger_lines(base_records, identity_hash)
        ours = {"packages/a": bump(pkg_a, "1.1.0", "aa1"), "packages/b": pkg_b}
        theirs_pkg_a = {**bump(pkg_a, "1.1.0", "aa1"), "notification-url": "https://packagist.example/"}
        theirs = {"packages/a": theirs_pkg_a, "packages/b": pkg_b}

        full_ours_lines = parent_ledger_lines(base_records, ours, "c0ffee1")
        full_theirs_lines = parent_ledger_lines(base_records, theirs, "c0ffee2")
        full_union, err = union_merge_ledger(work, full_ours_lines, base_lines, full_theirs_lines)
        assert err is None, err
        full_folded, full_err, _ = fold_ledger(full_union.splitlines())
        assert full_folded is None and full_err is not None, (
            "the full-record hash must still treat a metadata-only difference as a fork"
        )

        id_ours_lines = parent_ledger_lines(base_records, ours, "c0ffee1", identity_hash)
        id_theirs_lines = parent_ledger_lines(base_records, theirs, "c0ffee2", identity_hash)
        id_union, err = union_merge_ledger(work, id_ours_lines, base_id_lines, id_theirs_lines)
        assert err is None, err
        id_folded, id_err, id_picked = fold_ledger(id_union.splitlines())
        assert id_err is None, id_err
        assert id_picked, "same version+reference but differing metadata must trigger the deterministic pick"
        expected = min(canonical_record_text(ours["packages/a"]), canonical_record_text(theirs_pkg_a))
        assert canonical_record_text(id_folded["packages/a"]) == expected, (
            "the pick must choose the record whose canonical JSON sorts first"
        )

        # #306 follow-up: a genuine version difference still forks under
        # the identity hash -- it isn't a hash that agrees on everything.
        ours = {"packages/a": bump(pkg_a, "1.1.0", "aa1"), "packages/b": pkg_b}
        theirs = {"packages/a": bump(pkg_a, "1.2.0", "aa2"), "packages/b": pkg_b}
        id_ours_lines = parent_ledger_lines(base_records, ours, "c0ffee1", identity_hash)
        id_theirs_lines = parent_ledger_lines(base_records, theirs, "c0ffee2", identity_hash)
        id_union, err = union_merge_ledger(work, id_ours_lines, base_id_lines, id_theirs_lines)
        assert err is None, err
        id_folded, id_err, id_picked = fold_ledger(id_union.splitlines())
        assert id_folded is None and id_err is not None, "a version fork must refuse under the identity hash too"
        assert not id_picked

        # #331 candidate 3.3: dev-* records merge by commit, never re-solved.
        dev_a = {"name": "vendor/dev-pkg", "version": "dev-master", "source": {"reference": "aaa"}}
        dev_a_bumped = {"name": "vendor/dev-pkg", "version": "dev-master", "source": {"reference": "bbb"}}
        dev_a_other = {"name": "vendor/dev-pkg", "version": "dev-master", "source": {"reference": "ccc"}}
        conflict, resolved = dev_commit_pick(dev_a, dev_a, dev_a)
        assert not conflict and resolved == dev_a, "unchanged on both sides must not conflict"
        conflict, resolved = dev_commit_pick(dev_a, dev_a_bumped, dev_a)
        assert not conflict and resolved == dev_a_bumped, "changed on one side only must take that side"
        conflict, resolved = dev_commit_pick(dev_a, dev_a_bumped, dev_a_other)
        assert conflict, "changed to different commits on both sides must conflict"

        lock_bytes = json.dumps({"packages": [dev_a, pkg_b], "packages-dev": []}).encode()
        stripped = strip_dev_records(lock_bytes, {"vendor/dev-pkg"})
        assert json.loads(stripped)["packages"] == [pkg_b], "strip_dev_records must drop only the named dev entries"

        manifest = json.dumps({"require": {"vendor/dev-pkg": "dev-master", "vendor/b": "^1.0"}}).encode()
        stripped_manifest = strip_dev_requires(manifest, {"vendor/dev-pkg"})
        assert json.loads(stripped_manifest)["require"] == {"vendor/b": "^1.0"}, (
            "strip_dev_requires must drop only the named dev requirement"
        )

        cache_root = Path(tmp) / "dev-cache"
        provider_dir = cache_root / "repo-v0" / "repo.packagist.org"
        provider_dir.mkdir(parents=True)
        (provider_dir / "provider-vendor$dev-pkg~dev.json").write_text(json.dumps({
            "packages": {"vendor/dev-pkg": [{"version": "dev-master", "source": {"reference": "bbb"}}]}
        }))
        assert dev_ref_cached(cache_root, "vendor/dev-pkg", "bbb"), (
            "a cached provider entry with a matching reference must read as cached"
        )
        assert not dev_ref_cached(cache_root, "vendor/dev-pkg", "aaa"), (
            "a historical reference the cache never saw as today's head must read as needing a fetch"
        )
        assert not dev_ref_cached(cache_root, "vendor/other", "bbb"), "an uncached package must read as needing a fetch"

    print("lockmerge: self-test OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
