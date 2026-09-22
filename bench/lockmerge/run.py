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
of format: packages whose version or source reference differ between
base->ours AND base->theirs, with ours and theirs landing on different
results. That is the control -- a textual conflict a format removes is a
win only if it was not a real one.

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
(cascade) -- sizing how mechanical a merge driver's job would be.

Usage:
    bench/lockmerge/run.py [project-name,...]
    bench/lockmerge/run.py --self-test

Env: BENCH_CACHE (persistent clone cache, default matches
bench/storeload.sh's own -- clones live under $BENCH_CACHE/lockmerge/<safe
name>), LOCKMERGE_CAP (most recent qualifying merges per repo, default 200),
LOCKMERGE_CORPUS (corpus.toml path), LOCKMERGE_REPORT (output path), VIV
(binary probed for `lock convert`, default target/release/viv).
"""
from __future__ import annotations

import json
import os
import re
import statistics
import subprocess
import sys
import tempfile
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


def lock_index(raw: bytes) -> dict[str, tuple[str | None, str | None]] | None:
    try:
        data = json.loads(raw)
    except json.JSONDecodeError:
        return None
    idx: dict[str, tuple[str | None, str | None]] = {}
    for key in ("packages", "packages-dev"):
        for pkg in data.get(key) or []:
            name = pkg.get("name")
            if not name:
                continue
            idx[name] = (pkg.get("version"), (pkg.get("source") or {}).get("reference"))
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
    """m/o/t are (version, source-reference) tuples from the merge commit's,
    ours', and theirs' composer.lock (or None when the package is absent)."""
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


@dataclass
class ArchOutcome:
    sha: str
    source_conflict: bool
    packages: list[PackageResolution]
    cascade: int


def compute_archaeology(
    repo_dir: Path,
    work: Path,
    m: Merge,
    real_names: set[str],
    base_idx: dict,
    ours_idx: dict,
    theirs_idx: dict,
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

    return ArchOutcome(m.sha, json_conflicts > 0, packages, cascade)


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


def run_repo(project: Project, cache_root: Path, cap: int, viv_bin: str, native_available: bool) -> RepoReport:
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
                    repo_dir, work, m, real_names, base_idx, ours_idx, theirs_idx, report.footnotes
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

            report.merges.append(MergeOutcome(m.sha, composer_conflicts, native_conflicts, native_error, real))

    return report


# --- report writing --------------------------------------------------


def fmt_n(v: int | None) -> str:
    return "n/a" if v is None else str(v)


def render_archaeology(items: list[ArchOutcome]) -> list[str]:
    """Three compact tables classifying the human resolutions git history
    already holds, for every merge with a real conflict."""
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
    return lines


def render(
    reports: list[RepoReport], cap: int, viv_bin: str, viv_version: str,
    native_available: bool, viv_commit: str | None,
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
    lines.append("")

    total_merges = total_composer = total_native = total_real = total_wins = 0
    total_composer_conflicting = total_native_conflicting = 0
    native_seen_anywhere = False

    for r in reports:
        lines.append(f"### {r.project.name}")
        lines.append("")
        if r.skipped_reason:
            lines.append(f"Skipped: {r.skipped_reason}.")
            lines.append("")
            continue
        lines.append(f"Examined `{r.range_note}`" + (" -- capped." if r.capped else "."))
        lines.append("")
        lines.append(
            "| Merges examined | Merges conflicting (composer.lock) | "
            "Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | "
            "Conflict hunks (viv.lock) | Real conflicts | "
            "composer.lock conflicted, viv.lock did not |"
        )
        lines.append("|---|---|---|---|---|---|---|")

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

        lines.append(
            f"| {n} | {composer_conflicting} | {fmt_n(native_conflicting)} | "
            f"{composer_sum} | {fmt_n(native_sum)} | {real_sum} | {fmt_n(wins_display)} |"
        )
        lines.append("")

        total_merges += n
        total_composer += composer_sum
        total_composer_conflicting += composer_conflicting
        if native_vals:
            native_seen_anywhere = True
            total_native += native_sum
            total_native_conflicting += native_conflicting
            total_wins += wins
        total_real += real_sum

        for f in r.footnotes:
            lines.append(f"- {f}")
        if r.footnotes:
            lines.append("")

        lines.extend(render_archaeology(r.archaeology))

    lines.append("### Totals")
    lines.append("")
    lines.append(
        "| Merges examined | Merges conflicting (composer.lock) | "
        "Merges conflicting (viv.lock) | Conflict hunks (composer.lock) | "
        "Conflict hunks (viv.lock) | Real conflicts | "
        "composer.lock conflicted, viv.lock did not |"
    )
    lines.append("|---|---|---|---|---|---|---|")
    lines.append(
        f"| {total_merges} | {total_composer_conflicting} | "
        f"{fmt_n(total_native_conflicting) if native_seen_anywhere else 'n/a'} | "
        f"{total_composer} | "
        f"{fmt_n(total_native) if native_seen_anywhere else 'n/a'} | {total_real} | "
        f"{fmt_n(total_wins) if native_seen_anywhere else 'n/a'} |"
    )
    lines.append("")

    all_archaeology = [a for r in reports for a in r.archaeology]
    lines.extend(render_archaeology(all_archaeology))
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
    only = sys.argv[1].split(",") if len(sys.argv) > 1 and sys.argv[1] != "--self-test" else []
    if len(sys.argv) > 1 and sys.argv[1] == "--self-test":
        return self_test()

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

    projects = parse_corpus(corpus_path)
    if only:
        projects = [p for p in projects if p.name in only]

    reports = []
    for project in projects:
        log(f"{project.name}: starting")
        r = run_repo(project, cache_root, cap, viv_bin, native_available)
        reports.append(r)
        if r.skipped_reason:
            update_corpus_note(corpus_path, project.name, f'skip = "{r.skipped_reason}"')
        else:
            update_corpus_note(corpus_path, project.name, f'examined = "{r.range_note}"')

    section = render(reports, cap, viv_bin, viv_version, native_available, viv_commit)
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

        def commit(msg, pkgs):
            (repo / "composer.lock").write_bytes(lock(pkgs))
            git(repo, "add", "composer.lock")
            git(repo, "commit", "--quiet", "-m", msg)
            return git(repo, "rev-parse", "HEAD").stdout.strip()

        # composer.json never changes across branches, so archaeology's
        # source-vs-lock-only split has a lock-only case to classify.
        (repo / "composer.json").write_bytes(b'{"name": "test/test"}\n')
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
        commit("ours2 bumps a and b", conflict_pkgs)
        git(repo, "checkout", "--quiet", "-b", "theirs2", "theirs")
        theirs2_pkgs = [
            {"name": "a/a", "version": "1.3.0", "source": {"reference": "aa3"}},
            {"name": "b/b", "version": "2.0.1", "source": {"reference": "bb3"}},
        ]
        commit("theirs2 bumps b further", theirs2_pkgs)
        git(repo, "checkout", "--quiet", "main")
        merge_sha_before = git(repo, "rev-parse", "HEAD").stdout.strip()
        git(repo, "checkout", "--quiet", "-b", "merge-target", "ours2")
        r = subprocess.run(["git", "merge", "--no-ff", "-m", "two-parent merge", "theirs2"], cwd=repo, capture_output=True)
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

        arch = compute_archaeology(repo, work, m, real_names, base_idx, ours_idx, theirs_idx, [])
        assert arch is not None, "expected archaeology to classify the resolved merge"
        assert arch.source_conflict is False, "composer.json never changed, expected lock-only"
        by_name = {p.name: p for p in arch.packages}
        assert by_name["a/a"].outcome == "ours", f"expected a/a resolved to ours, got {by_name['a/a']}"
        assert by_name["b/b"].outcome == "theirs" and by_name["b/b"].higher == "higher", (
            f"expected b/b resolved to theirs and higher, got {by_name['b/b']}"
        )
        assert arch.cascade == 0, f"expected no dragged-along packages, got {arch.cascade}"
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

    print("lockmerge: self-test OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
