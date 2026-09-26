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
the safety number (rung 0 accepted a pin the registry re-solve would have
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


def classify_safety(
    offline_idx: dict, baseline_idx: dict, ours_idx: dict, theirs_idx: dict,
) -> str | None:
    """#314's safety number: `None` when every package rung 0 and the
    registry re-solve (baseline, no flag) both resolved agrees; otherwise
    one kind string per differing package, joined, naming which parent
    rung 0's pin came from and whether the registry chose older or newer.
    Compares every package, not just the divergent names -- rung 3's own
    `moved` list (#296) means a *non*-divergent package can differ too."""
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


def driver_conflict(
    viv_bin: str, work: Path, cache_dir: Path, repo_dir: Path, sha: str,
    composer_json: bytes | None, base: bytes, ours: bytes, theirs: bytes,
    offline_rung: bool = False,
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
    successful rung-0 resolve reports rung 0 through the same
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
    result = subprocess.run(command, capture_output=True)
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


@dataclass
class OfflineOutcome:
    """`--offline-rung` (#314), compared against the same merge's plain
    `driver_conflict` (no flag) call. `safety_kind` is only set when both
    finished, rung 0 is the one that finished the flagged run, and some
    package's identity differs between the two results -- the safety
    number's per-case classification (which parent's pin rung 0 kept, and
    whether the registry re-solve moved to an older or newer version)."""
    conflicted: bool | None
    error: str | None
    rung: int | None
    time: float | None
    safety_kind: str | None = None


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
                    merge_json, base_lock, ours_lock, theirs_lock,
                )
                driver_time = time.perf_counter() - driver_start
                if driver_conflicted and driver_err:
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
                    merge_json, base_lock, ours_lock, theirs_lock, offline_rung=True,
                )
                off_time = time.perf_counter() - offline_start
                if off_conflicted and off_err:
                    report.footnotes.append(
                        f"{m.sha[:12]}: --offline-rung re-solve did not finish: {off_err}"
                    )
                safety_kind = None
                if off_rung == 0 and off_conflicted is False and driver_conflicted is False:
                    offline_idx, baseline_idx = lock_index(off_lock), lock_index(driver_lock)
                    if offline_idx is not None and baseline_idx is not None:
                        safety_kind = classify_safety(offline_idx, baseline_idx, ours_idx, theirs_idx)
                        if safety_kind:
                            report.footnotes.append(
                                f"{m.sha[:12]}: offline rung 0 safety difference: {safety_kind}"
                            )
                offline_outcome = OfflineOutcome(off_conflicted, off_err, off_rung, off_time, safety_kind)

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


def render_offline(merges: list[MergeOutcome]) -> list[str]:
    """#314: `--offline-rung` against the same merge's plain (no-flag)
    driver call. Four counts, in the issue's own order -- residue cleared
    (a merge that used to end in markers and now doesn't, by the leaf
    cause of the marker it cleared), residue remaining (still markers,
    same leaf-cause buckets), the safety number (rung 0 accepted a pin the
    registry re-solve, when it also finished, would have picked
    differently), and network avoided (rung 0 finished a merge that used
    to need an escalation rung, no registry contact for it this time) --
    plus the median time both ways."""
    counted = [m for m in merges if m.offline is not None]
    lines = ["### Offline rung vs registry escalation (#314)", ""]
    if not counted:
        lines.append("No merge had both an `--offline-rung` and a plain driver result to compare.")
        lines.append("")
        return lines

    n = len(counted)
    cleared = [m for m in counted if m.driver_conflict is True and m.offline.conflicted is False]
    remaining = [m for m in counted if m.offline.conflicted is True]
    network_avoided = [
        m for m in counted
        if m.offline.rung == 0 and m.driver_conflict is False and m.driver_rung is not None
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
        "| Merges examined | Residue cleared | Residue remaining | Safety number | "
        "Network avoided | Median ms, no flag | Median ms, --offline-rung |"
    )
    lines.append("|---|---|---|---|---|---|---|")
    lines.append(
        f"| {n} | {len(cleared)} | {len(remaining)} | {len(safety_cases)} | "
        f"{len(network_avoided)} | "
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
        lines.append("Safety number: 0. No merge where rung 0's pin and the registry's own "
                      "re-solve, when both finished, chose a different package identity.")
        lines.append("")
    return lines


def render(
    reports: list[RepoReport], cap: int, viv_bin: str, viv_version: str,
    native_available: bool, viv_commit: str | None, driver_available: bool,
    ledger: bool = False, hybrid: bool = False, offline_rung: bool = False,
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
        "Conflict hunks (composer.lock) | Conflict hunks (viv.lock) | Real conflicts | "
        "composer.lock conflicted, viv.lock did not |"
    )
    separator = "|---|---|---|---|---|---|---|---|"

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
        driver_vals = [m.driver_conflict for m in r.merges if m.driver_conflict is not None]
        driver_conflicting = sum(driver_vals) if driver_vals else None

        lines.append(
            f"| {n} | {composer_conflicting} | {fmt_n(native_conflicting)} | "
            f"{fmt_n(driver_conflicting)} | "
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
        if driver_vals:
            driver_seen_anywhere = True
            total_driver_conflicting += driver_conflicting
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
    argv = [a for a in argv if a not in ("--ledger", "--hybrid", "--offline-rung")]
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

    reports = []
    for project in projects:
        log(f"{project.name}: starting")
        r = run_repo(
            project, cache_root, cap, viv_bin, native_available, driver_available,
            ledger, hybrid, offline_rung,
        )
        reports.append(r)
        if r.skipped_reason:
            update_corpus_note(corpus_path, project.name, f'skip = "{r.skipped_reason}"')
        else:
            update_corpus_note(corpus_path, project.name, f'examined = "{r.range_note}"')

    section = render(
        reports, cap, viv_bin, viv_version, native_available, viv_commit, driver_available,
        ledger, hybrid, offline_rung,
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
        # #314: rung 0 (--offline-rung) reads through the same line.
        assert parse_resolution(
            "viv lock merge: resolved via rung 0 (offline): d/dep\n"
        ) == (0, 0), "rung 0 must parse the same as any other rung"

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

    print("lockmerge: self-test OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
