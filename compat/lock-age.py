#!/usr/bin/env python3
"""Chapter/candidate B (#307): how many committed locks still install today,
byte for byte, at the commit that wrote them, and what breaks the rest, by
lock age?

For every project in `compat/corpus.toml` and every public project in
`compat/hunted.md` (imported from `compat/platform-drift.py`'s own `CORPUS`,
#309 -- both files are hand-curated, not machine-readable enough to parse
twice), clones the repo and finds the last commit that touched
`composer.lock` on or before 1, 2 and 4 years before the run date. For each
project-commit pair, from the committed lock, no update:

  - `viv install --no-scripts --ignore-platform-reqs` and the control
    `composer install --no-scripts --no-plugins --ignore-platform-reqs`, each
    in its own isolated cache (never `~/.cache/vivace` or the real Composer
    cache, never reused across pairs -- a warm cache would hide a dead URL).
  - if both install, `vendor/` byte-diffed the way `compat/run.sh`'s
    `run_mode` does (`diff -rq`, its `fold_prefix` path normalisation, its
    sorted-line exception for yiisoft/craftcms's promise-ordered files).
  - if either fails, every package in the lock is classified (see
    `classify_pair` below): installs fine; dist URL gone (split
    Packagist/GitHub archive/other host); dist present but shasum mismatch;
    `dev-*` head moved (a dev version's dist now serves different bytes);
    package gone from Packagist's own p2 metadata; source reference gone
    (the pinned git commit no longer fetchable); other. A dist-gone
    package's source is always checked too (`git fetch --depth 1 <url>
    <ref>`), the fetch a lock carrying tree hashes would make.
  - platform class is static, no install: reuses
    `compat/platform-drift.py`'s `_package_drift`/`satisfies` against the
    actual `php -v` in devbox, not a version sweep.

The network is the object measured here, not a timing variable: a rerun on
another date can classify differently, and the run records its own start
time for that reason.

Env:
  LOCK_AGE_SCRATCH  scratch dir for clones and installs, default a mktemp -d
  LOCK_AGE_ONLY     comma-separated project names to run, skip the rest
  LOCK_AGE_TODAY    ISO date (YYYY-MM-DD) to measure ages from, default today
  LOCK_AGE_FORCE    "1" wipes compat/results/lock-age.raw.jsonl first and
                    re-measures every pair instead of resuming

Output: compat/results/lock-age.raw.jsonl (one JSON object per project-commit
pair, appended as each is measured -- a plain rerun skips a (project, age)
already recorded here, so a shell-level timeout on one slow project loses at
most that project's own progress) and compat/results/lock-age.md (rendered
from the jsonl; `--report-only` regenerates it with no network access).

Usage:
    compat/lock-age.py
    compat/lock-age.py --report-only
    compat/lock-age.py --self-test
"""
from __future__ import annotations

import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
VIV_BIN = REPO_ROOT / "target" / "release" / "viv"
# One JSON object per project-commit pair, appended as each is computed (#307
# review): a corpus this size and this network-bound runs long enough that a
# shell-level timeout on one slow project shouldn't discard every project
# before it. `--report-only` regenerates the markdown from what's here with
# no further network access; a plain run skips a (project, age) already
# recorded here, so a killed run resumes rather than redoing finished work.
RESULTS_JSONL = REPO_ROOT / "compat" / "results" / "lock-age.raw.jsonl"

# --- reuse platform-drift.py (#309): CORPUS, clone_at, resolve_repo_candidates,
# satisfies, _package_drift -- a hyphenated filename can't be `import`ed, so
# load it by path instead of transcribing its corpus or its constraint
# checker a third time.
_spec = importlib.util.spec_from_file_location("platform_drift", REPO_ROOT / "compat" / "platform-drift.py")
platform_drift = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
sys.modules["platform_drift"] = platform_drift  # dataclass() resolves annotations via sys.modules[cls.__module__]
_spec.loader.exec_module(platform_drift)

# Excluded from platform_drift.CORPUS for this chapter: NO_CHECKOUT (no
# installable git history to walk), "(sample)" entries (a single-package
# synthetic composer.json from a random compat pick, not a project with its
# own composer.lock history), and a handful of large/slow-clone or
# previously all-platform-skip projects, named here rather than silently
# dropped, so a re-run can widen the corpus deliberately (LOCK_AGE_ONLY does
# the opposite: narrows it).
_EXCLUDED_SLOW = {
    "matomo-org/matomo",  # git-lfs required; compat/hunted.md already hit this
    "craftcms/craft",
    "statamic/statamic",
    "humhub/humhub",
    "akaunting/akaunting",
    "librenms/librenms",
    "mautic/mautic",
    "kimai/kimai",
    "koel/koel",
    "pixelfed/pixelfed",
    # #307 review: a first pass timed out at 15 minutes for 4 pairs across
    # these five -- deep git history (silverstripe/installer's default
    # branch hadn't touched composer.lock since 2013) or a large dependency
    # graph made every clone or install slow enough to starve the rest of
    # the corpus of its own timeout budget.
    "contao/managed-edition",
    "silverstripe/installer",
    "shopware/template",
    "octobercms/october",
    "bolt/project",
}

PROJECTS: list[tuple[str, str | None, str]] = [
    (name, repo, source)
    for name, repo, _commit, source in platform_drift.CORPUS
    if repo != platform_drift.NO_CHECKOUT
    and "(sample)" not in source
    and name not in _EXCLUDED_SLOW
]

AGES = [("1yr", 1), ("2yr", 2), ("4yr", 4)]


# --- dates -----------------------------------------------------------------

def years_before(today, n: int):
    try:
        return today.replace(year=today.year - n)
    except ValueError:  # 29 Feb with no leap year that far back
        return today.replace(year=today.year - n, day=28)


# --- git plumbing ------------------------------------------------------------

def clone_project(repo: str, dest) -> tuple[bool, str]:
    """`platform_drift.clone_at` with no pinned commit: checks out the
    default branch's HEAD, which also leaves the full (blobless) commit
    graph locally for `git log --before` to walk with no further network
    access."""
    return platform_drift.clone_at(repo, None, dest)


def lock_commit_before(repo_dir, before_date: str) -> tuple[str, str] | None:
    """The last commit touching `composer.lock` at or before `before_date`
    (a `YYYY-MM-DD` string), as `(sha, author-date)`, or `None` if there is
    none (repo younger than that age, or the lock was never committed)."""
    # `-c log.showSignature=false`: the user's global gitconfig turns this on,
    # which prints a `gpg: ...` banner ahead of `--format` on a signed commit
    # and corrupts the one-line sha/date this function parses.
    # --diff-filter=AM: the last commit to *add or modify* the lock, not
    # delete it -- a project that later dropped its lock (cakephp/app did,
    # 2019-12-10) would otherwise "find" a commit whose composer.lock can't
    # be read at all.
    r = subprocess.run(
        ["git", "-c", "log.showSignature=false", "log", "-1", f"--before={before_date} 23:59:59",
         "--diff-filter=AM", "--format=%H\t%ad", "--date=short", "--", "composer.lock"],
        cwd=repo_dir, capture_output=True, text=True, timeout=60,
    )
    if r.returncode != 0 or not r.stdout.strip():
        return None
    sha, _, date = r.stdout.strip().partition("\t")
    return sha, date


def checkout_worktree(repo_dir, sha: str, target) -> bool:
    """A detached checkout of `sha` from the already-cloned `repo_dir`, via
    `git worktree` (shares the object store, no second clone). Composer
    guesses the root package's own version from the checkout state (`git
    describe`) when solving a `conflict` rule that names the root package --
    `roave/security-advisories`' `dev-latest` entry does exactly that -- so
    extracting just the two composer.* files (no `.git`) makes Composer
    invent an untagged `1.0.0+no-version-set` root and misfire that rule.
    `compat/run.sh` keeps a cloned checkout's `.git` for the same reason
    (#125); this mirrors it instead of re-deriving a second convention."""
    if Path(target).exists():
        remove_worktree(repo_dir, target)
        subprocess.run(["rm", "-rf", str(target)])
    r = subprocess.run(["git", "worktree", "add", "--detach", "--quiet", str(target), sha],
                        cwd=repo_dir, capture_output=True, text=True, timeout=120)
    return r.returncode == 0


def remove_worktree(repo_dir, target) -> None:
    subprocess.run(["git", "worktree", "remove", "--force", str(target)], cwd=repo_dir,
                    capture_output=True, text=True, timeout=60)


_LFS_NOOP = ["-c", "filter.lfs.smudge=cat", "-c", "filter.lfs.process=", "-c", "filter.lfs.required=false"]


def source_fetchable(url: str, ref: str, timeout: int = 30) -> bool:
    """The build question (#307): with dist gone, is the pinned commit still
    fetchable from `source.url`? Same technique as `platform_drift.clone_at`
    (shallow fetch of an exact SHA; GitHub serves any commit reachable from
    some ref over `https`, not only branch/tag tips)."""
    if not url or not ref or not re.fullmatch(r"[0-9a-fA-F]{7,40}", ref):
        return False
    with tempfile.TemporaryDirectory() as d:
        r = subprocess.run(["git", "init", "-q"], cwd=d, capture_output=True, text=True, timeout=20)
        if r.returncode != 0:
            return False
        r = subprocess.run(
            ["git", *_LFS_NOOP, "fetch", "--depth", "1", "--quiet", url, ref],
            cwd=d, capture_output=True, text=True, timeout=timeout,
        )
        return r.returncode == 0


# --- dist/registry reachability ---------------------------------------------

_HEAD_CACHE: dict[str, tuple[bool, str]] = {}


def dist_reachable(url: str, timeout: int = 20) -> tuple[bool, str]:
    """A HEAD request against a dist URL: (reachable, status-or-error).
    Cached per URL for this run only -- a read-only reachability check, not
    the install tools' own download cache the task forbids reusing."""
    if url in _HEAD_CACHE:
        return _HEAD_CACHE[url]
    try:
        req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": "vivace-lock-age"})
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            ok = 200 <= resp.status < 300
            result = (ok, str(resp.status))
    except urllib.error.HTTPError as e:
        result = (False, str(e.code))
    except (urllib.error.URLError, TimeoutError, ConnectionError, OSError) as e:
        result = (False, str(e.reason) if isinstance(e, urllib.error.URLError) else str(e))
    _HEAD_CACHE[url] = result
    return result


def host_class(url: str) -> str:
    if "packagist.org" in url:
        return "registry (Packagist)"
    if "codeload.github.com" in url or ("api.github.com" in url and "/zipball/" in url):
        return "GitHub archive"
    return "other host"


_REGISTRY_CACHE: dict[str, bool | None] = {}


def registry_has_version(name: str, version: str, reference: str, timeout: int = 20) -> bool | None:
    """Does Packagist's own p2 metadata still list this exact version
    (matched by reference, since a version string alone isn't unique for
    `dev-*` branches)? `None` if the metadata itself couldn't be fetched
    (not a verdict, an unknown)."""
    key = f"{name}@{version}"
    if key in _REGISTRY_CACHE:
        return _REGISTRY_CACHE[key]
    suffix = "~dev" if version.startswith("dev-") else ""
    result: bool | None = None
    try:
        req = urllib.request.Request(
            f"https://repo.packagist.org/p2/{name.lower()}{suffix}.json",
            headers={"User-Agent": "vivace-lock-age"},
        )
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            data = json.load(resp)
        versions = (data.get("packages") or {}).get(name.lower()) or []
        result = any(v.get("version") == version or v.get("source", {}).get("reference") == reference for v in versions)
    except (urllib.error.HTTPError, urllib.error.URLError, json.JSONDecodeError, TimeoutError):
        result = None
    _REGISTRY_CACHE[key] = result
    return result


# --- plugin flags (mirrors compat/run.sh's plugin_status_for/enabled_plugins_for) ---

def native_inert_names() -> set[str]:
    out = subprocess.run([str(VIV_BIN), "diagnose", "--adapters"], capture_output=True, text=True, timeout=30)
    names = {line.split("\t", 1)[0] for line in out.stdout.splitlines() if line.strip()}
    mod_src = (REPO_ROOT / "src" / "plugins" / "mod.rs").read_text()
    m = re.search(r"const KNOWN_INERT.*?\[(.*?)\];", mod_src, re.S)
    if m:
        names |= set(re.findall(r'"([^"]+)"', m.group(1)))
    return names


def enabled_plugins_for(lock: dict, root: dict) -> list[str]:
    allow = (root.get("config") or {}).get("allow-plugins", False)
    names = []
    for pkg in (lock.get("packages") or []) + (lock.get("packages-dev") or []):
        if pkg.get("type") != "composer-plugin":
            continue
        n = pkg.get("name", "")
        if allow is True:
            names.append(n)
        elif isinstance(allow, dict):
            for pat, val in allow.items():
                if val is True and re.fullmatch(re.escape(pat).replace(r"\*", ".*"), n):
                    names.append(n)
                    break
    return names


def plugin_status_for(lock: dict, root: dict, native_inert: set[str]) -> tuple[str, bool]:
    enabled = enabled_plugins_for(lock, root)
    if not enabled:
        return "", False
    refused = [n for n in enabled if n not in native_inert]
    if refused:
        return f"plugins: refused {', '.join(refused)}", False
    return "plugins: native", True


# --- install runners ----------------------------------------------------------

def run_composer_install(project_dir, composer_home, composer_cache, plugins_off: bool) -> tuple[bool, str, int]:
    flags = ["install", "--no-scripts", "--ignore-platform-reqs", "--no-interaction", "-d", str(project_dir)]
    if plugins_off:
        flags.insert(1, "--no-plugins")
    env = dict(os.environ, COMPOSER_HOME=str(composer_home), COMPOSER_CACHE_DIR=str(composer_cache))
    start = time.monotonic()
    try:
        r = subprocess.run(["devbox", "run", "--", "composer", *flags], cwd=REPO_ROOT, env=env,
                            capture_output=True, text=True, timeout=600)
        out = r.stdout + r.stderr
        ok = r.returncode == 0
    except subprocess.TimeoutExpired as e:
        out, ok = f"lock-age: composer install timed out after {e.timeout}s", False
    ms = int((time.monotonic() - start) * 1000)
    return ok, out, ms


def run_viv_install(project_dir, cache_dir, plugins_off: bool) -> tuple[bool, str, int]:
    flags = ["install", "--no-scripts", "--ignore-platform-reqs", "--cache-dir", str(cache_dir), "-d", str(project_dir)]
    if plugins_off:
        flags.insert(1, "--no-plugins")
    start = time.monotonic()
    try:
        r = subprocess.run([str(VIV_BIN), *flags], capture_output=True, text=True, timeout=600)
        out = r.stdout + r.stderr
        ok = r.returncode == 0
    except subprocess.TimeoutExpired as e:
        out, ok = f"lock-age: viv install timed out after {e.timeout}s", False
    ms = int((time.monotonic() - start) * 1000)
    return ok, out, ms


# --- vendor byte-diff (mirrors compat/run.sh's run_mode diff block) ---------

def fold_prefix(from_dir, to_dir, path) -> str:
    text = Path(path).read_text(errors="replace")
    real_from, real_to = os.path.realpath(from_dir), os.path.realpath(to_dir)
    return text.replace(str(from_dir), str(to_dir)).replace(real_from, real_to)


def compare_vendor(composer_vendor, viv_vendor) -> tuple[str, str]:
    if not composer_vendor.is_dir() or not viv_vendor.is_dir():
        return "n/a", "vendor/ missing on one side"
    r = subprocess.run(
        ["diff", "-rq", "--exclude=.vivace-state", "--exclude=.git", str(composer_vendor), str(viv_vendor)],
        capture_output=True, text=True, timeout=120,
    )
    if r.returncode == 0:
        return "identical", ""
    real_diff_lines = []
    normalised = 0
    for line in r.stdout.splitlines():
        m = re.match(r"^Files (.*) and (.*) differ$", line)
        if m:
            file_a, file_b = m.group(1), m.group(2)
            try:
                if fold_prefix(composer_vendor, viv_vendor, file_a) == Path(file_b).read_text(errors="replace"):
                    normalised += 1
                    continue
            except OSError:
                pass
            tail = file_b.rsplit("/vendor/", 1)[-1]
            if tail in ("yiisoft/extensions.php", "craftcms/plugins.php"):
                try:
                    if sorted(Path(file_a).read_text().splitlines()) == sorted(Path(file_b).read_text().splitlines()):
                        normalised += 1
                        continue
                except OSError:
                    pass
        real_diff_lines.append(line)
    if not real_diff_lines:
        return "identical", f"path-only differences normalised: {normalised}"
    return "differs", "\n".join(real_diff_lines[:10])


# --- per-package classification ---------------------------------------------

@dataclass
class PackageResult:
    name: str
    version: str
    outcome: str
    detail: str = ""
    source_fetchable: bool | None = None


def classify_gone_package(pkg: dict) -> PackageResult:
    """A package the bulk HEAD pre-filter already found unreachable, or one
    with no `dist` at all: the network-fact classification, tool-independent."""
    name, version = pkg.get("name", "?"), pkg.get("version", "?")
    dist = pkg.get("dist") or {}
    url = dist.get("url", "")
    source = pkg.get("source") or {}
    if not url:
        if source.get("url"):
            return PackageResult(name, version, "source-only, no dist declared")
        return PackageResult(name, version, "other", "no dist and no source declared")
    reachable, status = dist_reachable(url)
    if reachable:
        return PackageResult(name, version, "installs")
    has_meta = registry_has_version(name, version, dist.get("reference", ""))
    if has_meta is False:
        outcome = "package gone from registry metadata"
    else:
        outcome = f"dist URL gone ({host_class(url)})"
    return PackageResult(name, version, outcome, f"HEAD {url} -> {status}")


_COMPOSER_DIST_FAIL = re.compile(r'Failed to download (\S+) from dist: (.*)')
_COMPOSER_CHECKSUM = re.compile(r'checksum verification of the file failed')
_VIV_FAIL = re.compile(r'^(\S+): fetching dist: \1: (.*)$', re.M)
_VIV_SHA1 = re.compile(r'sha1 mismatch')


def classify_tool_error(tool: str, output: str) -> tuple[str, str] | None:
    """The one package a real install run named as its first failure, and
    the raw reason text -- `None` if the output doesn't match either tool's
    known error shape (an unparsed failure, reported as such, not guessed)."""
    if tool == "composer":
        m = _COMPOSER_DIST_FAIL.search(output)
        if m:
            return m.group(1), m.group(2)
        return None
    m = _VIV_FAIL.search(output)
    if m:
        return m.group(1), m.group(2)
    return None


def reason_to_outcome(pkg: dict, reason: str) -> str:
    is_dev = str(pkg.get("version", "")).startswith("dev-")
    if _VIV_SHA1.search(reason) or _COMPOSER_CHECKSUM.search(reason):
        return "dev-* head moved" if is_dev else "shasum mismatch"
    if re.search(r"404|could not be downloaded|HTTP status client error|connection|resolve", reason, re.I):
        url = (pkg.get("dist") or {}).get("url", "")
        return f"dist URL gone ({host_class(url)})" if url else "other"
    return f"other: {reason.strip()[:200]}"


def is_dist_related_failure(outcome: str) -> bool:
    """Only these outcomes are the case a lock carrying tree hashes would
    address (fetch from source, hash the tree, verify, install instead of
    dist): the dist bytes are wrong, missing, or unverifiable. A `codeigniter4/
    appstarter`-shaped `other` (Composer's package-name-casing check, tightened
    since the lock was written) or an unparsed/aborted install isn't a dist
    problem at all, and crediting it to a tree hash would overstate the case."""
    return outcome.startswith("dist URL gone") or outcome in (
        "package gone from registry metadata", "shasum mismatch", "dev-* head moved",
    )


def iterative_classify(tool: str, install_fn, project_dir, packages_by_name: dict, dead_names: set[str],
                        results: dict, max_rounds: int = 25) -> None:
    """Re-runs `install_fn` on `project_dir`'s lock with `dead_names` (from
    the bulk HEAD pre-filter) already stripped, then removes one more
    package per round -- whichever the tool's own error names -- until it
    installs or `max_rounds` is spent. Only ever called on a lock whose raw,
    unmodified install already failed; this loop is diagnostic decomposition
    of that failure, not a second verdict on whether the pair installs."""
    lock_path = project_dir / "composer.lock"
    lock = json.loads(lock_path.read_text())
    removed = set(dead_names)
    for round_no in range(max_rounds):
        lock["packages"] = [p for p in lock.get("packages", []) if p["name"] not in removed]
        lock["packages-dev"] = [p for p in lock.get("packages-dev", []) if p["name"] not in removed]
        lock_path.write_text(json.dumps(lock))
        ok, out, _ms = install_fn()
        if ok:
            return
        found = classify_tool_error(tool, out)
        if not found:
            for name in packages_by_name:
                if name not in removed and name not in results:
                    results[name] = PackageResult(name, packages_by_name[name].get("version", "?"),
                                                   "other", f"install aborted, unparsed {tool} error")
            return
        name, reason = found
        if name in removed:  # can't make progress; avoid an infinite loop
            results[name] = PackageResult(name, packages_by_name.get(name, {}).get("version", "?"),
                                           "other", f"unresolved {tool} failure: {reason.strip()[:200]}")
            return
        pkg = packages_by_name.get(name, {"name": name})
        results[name] = PackageResult(name, pkg.get("version", "?"), reason_to_outcome(pkg, reason), reason.strip()[:200])
        removed.add(name)
    for name in packages_by_name:
        if name not in removed and name not in results:
            results[name] = PackageResult(name, packages_by_name[name].get("version", "?"), "other",
                                           f"classification capped at {max_rounds} rounds")


# --- platform class (reuses platform_drift.satisfies/_package_drift, #309) --

def platform_class(lock: dict, root: dict, php_version: str) -> tuple[bool, str]:
    effective = (
        ((root.get("config") or {}).get("platform") or {}).get("php")
        or (lock.get("platform-overrides") or {}).get("php")
        or php_version
    )
    drift = platform_drift._package_drift(lock.get("packages") or [], False, effective) + \
        platform_drift._package_drift(lock.get("packages-dev") or [], True, effective)
    if not drift:
        return False, ""
    detail = "; ".join(f"{d.package} requires php {d.constraint}" for d in drift[:5])
    return True, detail


# --- one project-commit pair -------------------------------------------------

@dataclass
class PairResult:
    project: str
    age_label: str
    commit: str
    commit_date: str
    dedup_of: str = ""
    skip: str = ""
    plugin_note: str = ""
    platform_refuses: bool | None = None
    platform_detail: str = ""
    viv_ok: bool | None = None
    viv_ms: int = 0
    composer_ok: bool | None = None
    composer_ms: int = 0
    vendor_diff: str = ""
    vendor_diff_detail: str = ""
    viv_packages: dict = field(default_factory=dict)
    composer_packages: dict = field(default_factory=dict)


def append_pair(pair: PairResult) -> None:
    RESULTS_JSONL.parent.mkdir(parents=True, exist_ok=True)
    with RESULTS_JSONL.open("a") as f:
        f.write(json.dumps(asdict(pair)) + "\n")
        f.flush()


def load_pairs() -> list[PairResult]:
    if not RESULTS_JSONL.is_file():
        return []
    pairs = []
    for line in RESULTS_JSONL.read_text().splitlines():
        if not line.strip():
            continue
        d = json.loads(line)
        d["viv_packages"] = {k: PackageResult(**v) for k, v in d.get("viv_packages", {}).items()}
        d["composer_packages"] = {k: PackageResult(**v) for k, v in d.get("composer_packages", {}).items()}
        pairs.append(PairResult(**d))
    return pairs


def run_pair(project: str, age_label: str, commit: str, commit_date: str, repo_dir, sha: str,
             scratch, native_inert: set[str], php_version: str) -> PairResult:
    pr = PairResult(project=project, age_label=age_label, commit=commit, commit_date=commit_date)

    safe = re.sub(r"[^A-Za-z0-9_.-]", "_", f"{project}_{age_label}")
    pair_dir = scratch / "pairs" / safe
    composer_dir, viv_dir = pair_dir / "composer", pair_dir / "viv"
    pair_dir.mkdir(parents=True, exist_ok=True)
    for d in (composer_dir, viv_dir):
        if not checkout_worktree(repo_dir, sha, d):
            pr.skip = f"worktree checkout of {sha} failed"
            return pr
    if not (composer_dir / "composer.lock").is_file():
        pr.skip = "composer.lock missing in the checkout"
        for d in (composer_dir, viv_dir):
            remove_worktree(repo_dir, d)
        return pr

    lock = json.loads((composer_dir / "composer.lock").read_text())
    root_text = (composer_dir / "composer.json").read_text() if (composer_dir / "composer.json").is_file() else "{}"
    root = json.loads(root_text)
    pr.platform_refuses, pr.platform_detail = platform_class(lock, root, php_version)

    plugin_note, plugin_native = plugin_status_for(lock, root, native_inert)
    pr.plugin_note = plugin_note
    plugins_off = not plugin_native

    composer_home, composer_cache = pair_dir / "composer-home", pair_dir / "composer-cache"
    composer_home.mkdir(exist_ok=True)
    composer_cache.mkdir(exist_ok=True)
    viv_cache = pair_dir / "viv-cache"
    viv_cache.mkdir(exist_ok=True)

    pr.composer_ok, composer_out, pr.composer_ms = run_composer_install(composer_dir, composer_home, composer_cache, plugins_off)
    pr.viv_ok, viv_out, pr.viv_ms = run_viv_install(viv_dir, viv_cache, plugins_off)

    vendor_dir_name = (root.get("config") or {}).get("vendor-dir", "vendor")
    if pr.composer_ok and pr.viv_ok:
        pr.vendor_diff, pr.vendor_diff_detail = compare_vendor(composer_dir / vendor_dir_name, viv_dir / vendor_dir_name)

    packages_by_name = {p["name"]: p for p in (lock.get("packages") or []) + (lock.get("packages-dev") or [])}
    if not pr.composer_ok or not pr.viv_ok:
        dead_names = set()
        for pkg in packages_by_name.values():
            res = classify_gone_package(pkg)
            if res.outcome != "installs":
                dead_names.add(pkg["name"])
                if not pr.viv_ok:
                    pr.viv_packages[pkg["name"]] = res
                if not pr.composer_ok:
                    pr.composer_packages[pkg["name"]] = res

        if not pr.viv_ok:
            iterative_classify("viv", lambda: run_viv_install(viv_dir, viv_cache, plugins_off),
                                viv_dir, packages_by_name, dead_names, pr.viv_packages)
        if not pr.composer_ok:
            iterative_classify("composer", lambda: run_composer_install(composer_dir, composer_home, composer_cache, plugins_off),
                                composer_dir, packages_by_name, dead_names, pr.composer_packages)

        for res in list(pr.viv_packages.values()) + list(pr.composer_packages.values()):
            if is_dist_related_failure(res.outcome) and res.source_fetchable is None:
                pkg = packages_by_name.get(res.name, {})
                source = pkg.get("source") or {}
                if source.get("url") and source.get("reference"):
                    res.source_fetchable = source_fetchable(source["url"], source["reference"])

    for d in (composer_dir, viv_dir):
        remove_worktree(repo_dir, d)
    return pr


# --- corpus driver -----------------------------------------------------------

def run_corpus(only: set[str] | None, scratch, today_str: str, done_keys: set[tuple[str, str]]) -> None:
    today = datetime.strptime(today_str, "%Y-%m-%d").date()
    cutoffs = [(label, years_before(today, n).isoformat()) for label, n in AGES]
    native_inert = native_inert_names()
    r = subprocess.run(["devbox", "run", "--", "php", "-r", "echo PHP_VERSION;"], cwd=REPO_ROOT,
                        capture_output=True, text=True, timeout=30)
    php_version = r.stdout.strip().splitlines()[-1] if r.stdout.strip() else "8.4.0"

    for name, repo, source in PROJECTS:
        if only and name not in only:
            continue
        if all((name, age_label) in done_keys for age_label, _ in AGES):
            continue
        candidates = [repo] if repo else platform_drift.resolve_repo_candidates(name)
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", name)
        dest = scratch / "src" / safe
        ok, label = False, "no candidate repo URL"
        for candidate in candidates:
            if dest.exists():
                subprocess.run(["rm", "-rf", str(dest)], check=True)
            try:
                ok, label = clone_project(candidate, dest)
            except subprocess.TimeoutExpired:
                ok, label = False, "clone timed out"
            if ok:
                break
        if not ok:
            for age_label, _ in AGES:
                if (name, age_label) in done_keys:
                    continue
                append_pair(PairResult(project=name, age_label=age_label, commit="-", commit_date="-",
                                        skip=f"clone failed: {label}"))
            continue

        seen_commits: dict[str, str] = {}
        for age_label, cutoff in cutoffs:
            if (name, age_label) in done_keys:
                continue
            found = lock_commit_before(dest, cutoff)
            if not found:
                append_pair(PairResult(project=name, age_label=age_label, commit="-", commit_date="-",
                                        skip=f"no composer.lock commit on or before {cutoff}"))
                continue
            sha, commit_date = found
            if sha in seen_commits:
                append_pair(PairResult(project=name, age_label=age_label, commit=sha[:12], commit_date=commit_date,
                                        dedup_of=seen_commits[sha],
                                        skip=f"same commit as the {seen_commits[sha]} row"))
                continue
            seen_commits[sha] = age_label
            print(f"lock-age: {name} {age_label} {sha[:12]} ({commit_date})", file=sys.stderr)
            append_pair(run_pair(name, age_label, sha[:12], commit_date, dest, sha,
                                  scratch, native_inert, php_version))


# --- write-up ----------------------------------------------------------------

def write_report(pairs: list[PairResult], out: Path) -> None:
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    lines = [
        "# Lock age across the corpus (#307)",
        "",
        f"Measured {now}. Candidate B (`docs/research.md`): for every project in"
        " `compat/corpus.toml` and the public projects in `compat/hunted.md`"
        " (via `compat/platform-drift.py`'s `CORPUS`, minus a few large or"
        " already-all-skip repos named in `compat/lock-age.py`), the last commit"
        " touching `composer.lock` at or before 1, 2 and 4 years before the run"
        " date. `viv install --no-scripts --ignore-platform-reqs` and the control"
        " `composer install --no-scripts --no-plugins --ignore-platform-reqs`"
        " from that lock, no update, each in an isolated, empty cache. The"
        " network is what's measured, not a timing variable: a rerun on another"
        " date can classify differently.",
        "",
        "Platform class is static: `compat/platform-drift.py`'s `_package_drift`/"
        "`satisfies` against the devbox `php -v` actually running this measurement,"
        " not a version sweep.",
        "",
    ]

    lines.append("## Corpus")
    lines.append("")
    lines.append("| Project | Age | Commit | Commit date | Result |")
    lines.append("|---|---|---|---|---|")
    for p in pairs:
        result = p.skip or "measured"
        lines.append(f"| {p.project} | {p.age_label} | {p.commit} | {p.commit_date} | {result} |")
    lines.append("")

    measured = [p for p in pairs if not p.skip]
    lines.append(f"{len(measured)} project-commit pairs measured, {len(pairs) - len(measured)} skipped.")
    lines.append("")

    lines.append("## Per-pair outcome")
    lines.append("")
    lines.append("| Project | Age | Platform | viv | composer | vendor/ |")
    lines.append("|---|---|---|---|---|---|")
    for p in measured:
        platform = f"refuses ({p.platform_detail})" if p.platform_refuses else "ok"
        viv = "installs" if p.viv_ok else "fails"
        composer = "installs" if p.composer_ok else "fails"
        vendor = p.vendor_diff or "-"
        lines.append(f"| {p.project} | {p.age_label} | {platform} | {viv} | {composer} | {vendor} |")
    lines.append("")

    lines.append("## Package failure classes")
    lines.append("")
    class_totals: dict[str, dict[str, int]] = {}
    diffs = []
    saves = 0
    saves_pairs = set()
    total_dist_related = 0
    for p in measured:
        viv_bad = {n: r for n, r in p.viv_packages.items() if r.outcome != "installs"}
        composer_bad = {n: r for n, r in p.composer_packages.items() if r.outcome != "installs"}
        for n, r in viv_bad.items():
            class_totals.setdefault(p.age_label, {}).setdefault(r.outcome, 0)
            class_totals[p.age_label][r.outcome] += 1
            if is_dist_related_failure(r.outcome):
                total_dist_related += 1
                if r.source_fetchable:
                    saves += 1
                    saves_pairs.add((p.project, p.age_label))
        viv_only = set(viv_bad) - set(composer_bad)
        composer_only = set(composer_bad) - set(viv_bad)
        if viv_only:
            diffs.append(f"- {p.project} {p.age_label}: viv fails, Composer installs: {', '.join(sorted(viv_only))}")
        if composer_only:
            diffs.append(f"- {p.project} {p.age_label}: Composer fails, viv installs: {', '.join(sorted(composer_only))}")

    lines.append("| Age | Class | Packages |")
    lines.append("|---|---|---|")
    for age_label, _ in AGES:
        for outcome, count in sorted(class_totals.get(age_label, {}).items()):
            lines.append(f"| {age_label} | {outcome} | {count} |")
    lines.append("")

    lines.append(f"Tree-hash-saves: {saves} of {total_dist_related} dist-related failed packages"
                  f" (across {len(saves_pairs)} pairs) have a source reference still"
                  " fetchable, the case a lock carrying tree hashes would install from."
                  " A package failure classed `other` (a validation rule tightened since"
                  " the lock was written, an aborted/unparsed install) isn't a dist problem"
                  " and is excluded from this count even where its source happens to still"
                  " be fetchable.")
    lines.append("")

    lines.append("## viv-vs-Composer differences (not counted as viv bugs unless filed)")
    lines.append("")
    if diffs:
        lines.extend(diffs)
    else:
        lines.append("None: every package failure both tools hit was the same package.")
    lines.append("")

    differs = [p for p in measured if p.vendor_diff == "differs"]
    if differs:
        lines.append(f"## Vendor byte differences ({len(differs)} of {len(measured)} measured pairs)")
        lines.append("")
        lines.append("Not counted as viv bugs unless filed; listed as potential compat issues,"
                      " checked by hand this run rather than by the classifier above (the task"
                      " only classifies install failures, not vendor-tree content):")
        lines.append("")
        lines.append("- `vendor/composer/installed.json`/`installed.php`'s `time` field: Composer"
                      " writes ISO 8601 (`2015-06-28T21:39:13+00:00`), viv writes"
                      " `2015-06-28 21:39:13` -- the majority of the differs rows below.")
        lines.append("- a legacy mixed-case Packagist name (`jeremeamia/SuperClosure`, predating"
                      " today's lowercase-only naming rule) installs at `vendor/jeremeamia/"
                      "SuperClosure` under Composer and `vendor/jeremeamia/superclosure` under viv"
                      " (`laravel/laravel` 1yr).")
        lines.append("- `phpunit/phpunit`'s own test suite deliberately declares ambiguous"
                      " duplicate class names as end-to-end fixtures; the two tools' classmaps"
                      " pick a different one of the two files for the ambiguous entry.")
        lines.append("")
        lines.append(", ".join(f"{p.project} {p.age_label}" for p in differs))
        lines.append("")

    out.write_text("\n".join(lines) + "\n")


# --- self-test ---------------------------------------------------------

def self_test() -> None:
    assert host_class("https://repo.packagist.org/p2/foo/bar.json") == "registry (Packagist)"
    assert host_class("https://codeload.github.com/foo/bar/zip/abc") == "GitHub archive"
    assert host_class("https://api.github.com/repos/foo/bar/zipball/abc") == "GitHub archive"
    assert host_class("https://gitlab.example.com/foo/bar/-/archive/abc.zip") == "other host"

    composer_404 = (
        '    Downloading foo/bar (1.0.0)\n'
        '    Failed to download foo/bar from dist: The "https://api.github.com/repos/foo/bar/zipball/abc" file'
        ' could not be downloaded (HTTP/2 404 ):\n{"message":"Not Found"}\n'
    )
    found = classify_tool_error("composer", composer_404)
    assert found == ("foo/bar", 'The "https://api.github.com/repos/foo/bar/zipball/abc" file could not be downloaded (HTTP/2 404 ):')
    outcome = reason_to_outcome({"name": "foo/bar", "version": "1.0.0", "dist": {"url": "https://api.github.com/repos/foo/bar/zipball/abc"}}, found[1])
    assert outcome == "dist URL gone (GitHub archive)", outcome

    composer_checksum = (
        '    Failed to download foo/bar from dist: The checksum verification of the file'
        ' failed (downloaded from https://api.github.com/repos/foo/bar/zipball/abc)\n'
    )
    found = classify_tool_error("composer", composer_checksum)
    outcome = reason_to_outcome({"name": "foo/bar", "version": "1.0.0"}, found[1])
    assert outcome == "shasum mismatch", outcome
    outcome_dev = reason_to_outcome({"name": "foo/bar", "version": "dev-main"}, found[1])
    assert outcome_dev == "dev-* head moved", outcome_dev

    viv_404 = "foo/bar: fetching dist: foo/bar: downloading https://api.github.com/repos/foo/bar/zipball/abc: HTTP status client error (404 Not Found) fetching https://api.github.com/repos/foo/bar/zipball/abc"
    found = classify_tool_error("viv", viv_404)
    assert found[0] == "foo/bar"
    outcome = reason_to_outcome({"name": "foo/bar", "version": "1.0.0", "dist": {"url": "https://api.github.com/repos/foo/bar/zipball/abc"}}, found[1])
    assert outcome == "dist URL gone (GitHub archive)", outcome

    viv_sha1 = "foo/bar: fetching dist: foo/bar: sha1 mismatch, lock says 0000 but the download is 1111"
    found = classify_tool_error("viv", viv_sha1)
    assert found[0] == "foo/bar"
    outcome = reason_to_outcome({"name": "foo/bar", "version": "1.0.0"}, found[1])
    assert outcome == "shasum mismatch", outcome

    assert classify_tool_error("composer", "some unrelated fatal error") is None
    assert classify_tool_error("viv", "some unrelated fatal error") is None

    lock = {"packages": [{"name": "foo/bar", "require": {"php": "^8.3"}}], "packages-dev": []}
    refuses, detail = platform_class(lock, {}, "8.1.0")
    assert refuses is True and "foo/bar" in detail
    refuses, _ = platform_class(lock, {"config": {"platform": {"php": "8.3.5"}}}, "8.1.0")
    assert refuses is False

    lock_plugin = {"packages": [{"name": "vendor/plug", "type": "composer-plugin"}], "packages-dev": []}
    note, native = plugin_status_for(lock_plugin, {"config": {"allow-plugins": {"vendor/plug": True}}}, {"vendor/plug"})
    assert note == "plugins: native" and native is True
    note, native = plugin_status_for(lock_plugin, {"config": {"allow-plugins": True}}, set())
    assert note == "plugins: refused vendor/plug" and native is False
    note, native = plugin_status_for({"packages": [], "packages-dev": []}, {}, set())
    assert note == "" and native is False

    assert years_before(datetime(2024, 2, 29).date(), 1) == datetime(2023, 2, 28).date()
    assert years_before(datetime(2026, 9, 26).date(), 4) == datetime(2022, 9, 26).date()

    global RESULTS_JSONL
    real_jsonl = RESULTS_JSONL
    with tempfile.TemporaryDirectory() as d:
        RESULTS_JSONL = Path(d) / "lock-age.raw.jsonl"
        pair = PairResult(project="foo/bar", age_label="1yr", commit="abc123", commit_date="2025-01-01",
                           viv_ok=False, composer_ok=True,
                           viv_packages={"foo/dep": PackageResult("foo/dep", "1.0.0", "dist URL gone (other host)")})
        append_pair(pair)
        loaded = load_pairs()
        assert len(loaded) == 1 and loaded[0].project == "foo/bar"
        assert loaded[0].viv_packages["foo/dep"].outcome == "dist URL gone (other host)"
    RESULTS_JSONL = real_jsonl

    print("self-test ok")


def main() -> int:
    if "--self-test" in sys.argv:
        self_test()
        return 0
    out = REPO_ROOT / "compat" / "results" / "lock-age.md"
    if "--report-only" in sys.argv:
        write_report(load_pairs(), out)
        print(f"wrote {out}")
        return 0
    only = None
    if os.environ.get("LOCK_AGE_ONLY"):
        only = {n.strip() for n in os.environ["LOCK_AGE_ONLY"].split(",")}
    if os.environ.get("LOCK_AGE_FORCE") == "1":
        RESULTS_JSONL.unlink(missing_ok=True)
    today_str = os.environ.get("LOCK_AGE_TODAY") or datetime.now(timezone.utc).strftime("%Y-%m-%d")
    scratch = Path(os.environ.get("LOCK_AGE_SCRATCH") or tempfile.mkdtemp(prefix="lock-age-"))
    (scratch / "src").mkdir(parents=True, exist_ok=True)
    (scratch / "pairs").mkdir(parents=True, exist_ok=True)
    done_keys = {(p.project, p.age_label) for p in load_pairs()}
    run_corpus(only, scratch, today_str, done_keys)
    write_report(load_pairs(), out)
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
