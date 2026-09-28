#!/usr/bin/env python3
"""Candidate 3.5 (#333), Part 1: a static count of which Composer resolution
features the corpus actually uses in its committed `composer.json` and
`composer.lock`, plus Part 2, a hand classification of closed compatibility
bugs by the feature that caused them.

Static only, same technique as bench/g3-toolchain/static.py: loads
compat/platform-drift.py by path (a hyphenated filename can't be
`import`ed) for CORPUS, NO_CHECKOUT, resolve_repo_candidates, clone_at and
analyse -- the same project selection platform-drift.py's own run already
made (compat/corpus.toml plus the public projects in compat/hunted.md),
narrowed to the ones that commit a composer.lock (analyse()'s own skip).
No installs, no registry solve, no fresh clone if the destination is
already checked out (`clone_at` reuses).

Part 1 features (exact tests, all read straight from the parsed JSON, no
constraint parsing beyond a literal substring/prefix check):

  - min-stab: root `minimum-stability` present and not the string "stable".
  - pref-stable: root `prefer-stable` is truthy.
  - no-version: root has no `version` key at all (Composer then guesses the
    root version from git -- "root-version guessing", #125/#128/#312).
  - branch-alias(root): root `extra.branch-alias` is truthy.
  - branch-alias(lock): count of lock packages (packages + packages-dev)
    whose own `extra.branch-alias` is truthy.
  - replace / provide: root `replace` / `provide` is truthy.
  - self.version: any value in root `replace` or `provide` is literally
    the string "self.version".
  - inline-alias(root): count of root require/require-dev constraint
    strings containing the literal substring " as " (Composer's inline
    alias syntax, `1.0 as 2.0-dev`).
  - lock-aliases: length of the lock's `aliases` array.
  - dev-*(root): count of root require/require-dev constraints with a
    whitespace/comma/pipe-delimited token starting with "dev-".
  - stability-flags(lock): length of the lock's `stability-flags` object.
  - repo:<type>: count of root `repositories` entries (list or the
    keyed-dict form) with that `type`, for vcs/path/composer/package/
    artifact.
  - no-packagist: root `repositories` disables packagist.org (the keyed
    form's `"packagist.org": false`, in either list or dict shape).
  - conflict: root `conflict` is truthy.
  - config.platform / allow-plugins: root `config.platform` /
    `config.allow-plugins` is truthy / present.
  - lock-type: count of lock packages (packages + packages-dev) whose
    `type` is neither "library" nor "metapackage" (Composer defaults an
    absent `type` to "library", so absent doesn't count).
  - lock-dev-version: count of lock packages whose `version` starts with
    "dev-".

What is assumed: a `repositories` entry that is neither a dict-of-name nor
a list-of-entries (malformed JSON) is skipped, not guessed at; an entry's
`type` outside the five named above is not counted anywhere (this census
only asks about those five).

Part 2 is a hand classification, not computed at runtime: closed issues
were read individually (title, body and, where the reasoning was
ambiguous, the closing commit message) and classified by the resolution
feature that caused the bug, using the same feature list as Part 1 plus
"none of these" for autoload/plugin/output-formatting bugs. The
classification, the selection rule and the excluded-but-considered issues
are all recorded as data below, not fetched fresh by this script, so a
rerun reproduces the same Part 2 table -- Part 1's numbers are the only
part this script actually measures.

Env:
  G3_RULES_SCRATCH   scratch dir for clones, default a mktemp -d
  G3_RULES_ONLY      comma-separated project names to run, skip the rest

Output: a Markdown report on stdout; a JSON dump of Part 1's rows in
G3_RULES_SCRATCH (printed on stderr).

Usage (from a clean checkout, needs network for the clones):
    python3 bench/g3-rules/features.py
"""
from __future__ import annotations

import importlib.util
import json
import os
import re
import sys
import tempfile
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent

_spec = importlib.util.spec_from_file_location("platform_drift", REPO_ROOT / "compat" / "platform-drift.py")
platform_drift = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
sys.modules["platform_drift"] = platform_drift
_spec.loader.exec_module(platform_drift)

REPO_TYPES = ["vcs", "path", "composer", "package", "artifact"]


def scratch_dir() -> Path:
    d = os.environ.get("G3_RULES_SCRATCH")
    path = Path(d) if d else Path(tempfile.mkdtemp(prefix="g3-rules-"))
    path.mkdir(parents=True, exist_ok=True)
    return path


def _dev_token(constraint: str) -> bool:
    return any(tok.startswith("dev-") for tok in re.split(r"[\s,|]+", constraint) if tok)


def _root_constraints(root: dict) -> list[str]:
    out = []
    for key in ("require", "require-dev"):
        for v in (root.get(key) or {}).values():
            if isinstance(v, str):
                out.append(v)
    return out


def _repo_entries(root: dict) -> tuple[list, bool]:
    """Every repository entry (dict values or list items), plus whether
    packagist.org is disabled -- root `repositories` can be a keyed dict
    (`{"packagist.org": false, "satis": {...}}`) or a plain list."""
    repos = root.get("repositories")
    entries: list = []
    no_packagist = False
    if isinstance(repos, dict):
        for name, val in repos.items():
            if val is False:
                if name == "packagist.org":
                    no_packagist = True
                continue
            entries.append(val)
    elif isinstance(repos, list):
        for val in repos:
            if val is False:
                continue
            if isinstance(val, dict) and val.get("packagist.org") is False:
                no_packagist = True
                continue
            entries.append(val)
    return entries, no_packagist


def features_for(root: dict, lock: dict) -> dict:
    packages = (lock.get("packages") or []) + (lock.get("packages-dev") or [])

    ms = root.get("minimum-stability")
    replace, provide = root.get("replace") or {}, root.get("provide") or {}
    self_version = any(v == "self.version" for v in list(replace.values()) + list(provide.values()))
    root_constraints = _root_constraints(root)
    entries, no_packagist = _repo_entries(root)
    repo_counts = {t: sum(1 for e in entries if isinstance(e, dict) and e.get("type") == t) for t in REPO_TYPES}
    config = root.get("config") or {}

    return {
        "min_stability": bool(ms) and ms != "stable",
        "prefer_stable": bool(root.get("prefer-stable")),
        "root_version_absent": "version" not in root,
        "branch_alias_root": bool((root.get("extra") or {}).get("branch-alias")),
        "branch_alias_lock": sum(1 for p in packages if (p.get("extra") or {}).get("branch-alias")),
        "replace_root": bool(replace),
        "provide_root": bool(provide),
        "self_version": self_version,
        "inline_alias_root": sum(1 for c in root_constraints if " as " in c),
        "lock_aliases": len(lock.get("aliases") or []),
        "dev_constraint_root": sum(1 for c in root_constraints if _dev_token(c)),
        "lock_stability_flags": len(lock.get("stability-flags") or {}),
        **{f"repo_{t}": n for t, n in repo_counts.items()},
        "repo_no_packagist": no_packagist,
        "conflict_root": bool(root.get("conflict")),
        "config_platform": bool(config.get("platform")),
        "config_allow_plugins": "allow-plugins" in config,
        "lock_nonlib_type": sum(1 for p in packages if (p.get("type") or "library") not in ("library", "metapackage")),
        "lock_dev_version": sum(1 for p in packages if str(p.get("version", "")).startswith("dev-")),
    }


BOOL_COLS = [
    ("min_stability", "min-stab"),
    ("prefer_stable", "pref-stable"),
    ("root_version_absent", "no-version"),
    ("branch_alias_root", "branch-alias(root)"),
    ("replace_root", "replace"),
    ("provide_root", "provide"),
    ("self_version", "self.version"),
    ("repo_no_packagist", "no-packagist"),
    ("conflict_root", "conflict"),
    ("config_platform", "config.platform"),
    ("config_allow_plugins", "allow-plugins"),
]
COUNT_COLS = [
    ("branch_alias_lock", "branch-alias(lock)"),
    ("inline_alias_root", "inline-alias(root)"),
    ("lock_aliases", "lock-aliases"),
    ("dev_constraint_root", "dev-*(root)"),
    ("lock_stability_flags", "stability-flags(lock)"),
    ("repo_vcs", "repo:vcs"),
    ("repo_path", "repo:path"),
    ("repo_composer", "repo:composer"),
    ("repo_package", "repo:package"),
    ("repo_artifact", "repo:artifact"),
    ("lock_nonlib_type", "lock-type≠lib/meta"),
    ("lock_dev_version", "lock-dev-version"),
]
ALL_COLS = BOOL_COLS + COUNT_COLS


def run() -> list[dict]:
    scratch = Path(os.environ.get("G3_RULES_SCRATCH") or tempfile.mkdtemp(prefix="g3-rules-clones-"))
    only = {n.strip() for n in os.environ["G3_RULES_ONLY"].split(",")} if os.environ.get("G3_RULES_ONLY") else None
    rows: list[dict] = []
    for name, repo, commit, source in platform_drift.CORPUS:
        if only and name not in only:
            continue
        if repo == platform_drift.NO_CHECKOUT:
            rows.append({"name": name, "source": source, "skip": "no installable git checkout"})
            continue
        candidates = [repo] if repo is not None else platform_drift.resolve_repo_candidates(name)
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", name)
        dest = scratch / safe
        ok, label = False, "no candidate repo URL"
        for candidate in candidates:
            if not (dest / ".git").exists():
                ok, label = platform_drift.clone_at(candidate, commit, dest)
                if ok:
                    break
            else:
                ok, label = True, "reused"
                break
        if not ok:
            rows.append({"name": name, "source": source, "skip": f"clone failed: {label}"})
            continue
        pr = platform_drift.analyse(name, source, dest, label)
        if pr.skip:
            rows.append({"name": name, "source": source, "skip": pr.skip})
            continue
        lock = json.loads((dest / "composer.lock").read_text(errors="replace"))
        root_path = dest / "composer.json"
        root = json.loads(root_path.read_text(errors="replace")) if root_path.is_file() else {}
        row = {"name": name, "source": source, "commit": label}
        row.update(features_for(root, lock))
        rows.append(row)
    return rows


def summarise(rows: list[dict]) -> dict:
    analysed = [r for r in rows if "skip" not in r]
    n = len(analysed)
    bool_totals = {label: sum(1 for r in analysed if r[key]) for key, label in BOOL_COLS}
    count_totals = {label: sum(r[key] for r in analysed) for key, label in COUNT_COLS}
    count_nonzero = {label: sum(1 for r in analysed if r[key]) for key, label in COUNT_COLS}
    return {
        "projects_total": len(rows),
        "projects_analysed": n,
        "projects_skipped": len(rows) - n,
        "bool_totals": bool_totals,
        "count_totals": count_totals,
        "count_nonzero_projects": count_nonzero,
    }


def render_part1(rows: list[dict], summary: dict) -> list[str]:
    lines = []
    lines.append("## Part 1: feature census")
    lines.append("")
    n_total, n_ana, n_skip = summary["projects_total"], summary["projects_analysed"], summary["projects_skipped"]
    lines.append(f"Projects: {n_ana} analysed of {n_total} in the corpus ({n_skip} skipped, no committed lock or no checkout).")
    lines.append("")
    header = ["project"] + [label for _, label in ALL_COLS]
    lines.append("| " + " | ".join(header) + " |")
    lines.append("|" + "---|" * len(header))
    for r in rows:
        if "skip" in r:
            lines.append(f"| {r['name']} | " + " | ".join(["skip"] * len(ALL_COLS)) + " |")
            continue
        cells = []
        for key, _ in BOOL_COLS:
            cells.append("Y" if r[key] else "")
        for key, _ in COUNT_COLS:
            cells.append(str(r[key]) if r[key] else "")
        lines.append(f"| {r['name']} | " + " | ".join(cells) + " |")
    lines.append("")
    lines.append("### Totals")
    lines.append("")
    for key, label in BOOL_COLS:
        lines.append(f"- {label}: **{summary['bool_totals'][label]}** of {n_ana} projects.")
    for key, label in COUNT_COLS:
        lines.append(
            f"- {label}: **{summary['count_nonzero_projects'][label]}** of {n_ana} projects use it, "
            f"{summary['count_totals'][label]} total."
        )
    lines.append("")
    return lines


# --- Part 2: closed compatibility bugs by feature (hand classification) ---
# Selected from `gh issue list --state closed --limit 400 --json
# number,title,labels,body` (307 closed issues, fetched 2026-09-28): every
# issue whose title or body describes viv's resolved packages, lock
# content, installed.json/php, autoload output or install/update behaviour
# differing from real Composer's on the same input, or a solver crash/
# failure where Composer succeeds -- read individually, not grepped for
# keywords. #312 and #316 are still OPEN (candidate 3.5 names them anyway,
# per the task) and are included with that noted.

SELECTION_RULE = (
    "Closed issues (plus #312 and #316, both open, named by the candidate) "
    "describing a case where viv's resolved package set, lock bytes, "
    "installed.json/php, autoload output, or install/update/require "
    "success-vs-failure differs from real Composer's on the same input. "
    "Read individually (title, body, and the closing commit message where "
    "the cause was ambiguous from the issue text alone); not selected by "
    "title keyword match alone."
)

CLASSIFICATION = [
    (28, "installed.php gaps: root replace/provide, natural sort case, numeric alias check",
     "replace/provide",
     "root-level replace/provide missing from installed.php's `versions` map (also touches a branch-alias numeric-prefix check, secondary)"),
    (35, "lock: reject a package present in both packages and packages-dev",
     "none of these",
     "lock structural validation (duplicate package name across packages/packages-dev), not a resolution-rule feature"),
    (63, "Metapackages with a dist must not be installed",
     "package type",
     "viv keyed install-or-not on dist presence instead of `type: metapackage`"),
    (79, "Partial update of a transitively-required package fails: not found in any version",
     "none of these",
     "fix: \"partial update loads the allow-listed package through locked parents\" -- graph-traversal completeness, not a manifest feature"),
    (105, "viv update fails on wpackagist metadata: provider entry is not a list",
     "repositories",
     "composer-type repository (wpackagist) serving Composer v1's object-keyed provider format"),
    (115, "update: self.version in a dependency's require breaks the closure walk (bedrock, drupal)",
     "replace/provide",
     "self.version literal reached the constraint parser before substitution in the closure walk"),
    (116, "update: pool optimizer leaves alias_of unremapped, panics on phpunit/phpunit",
     "branch-alias",
     "triggered specifically by branch-aliased packages (phpunit) surviving pool pruning"),
    (117, "update: root replace/provide ignored, symfony/demo lock gains four polyfills",
     "replace/provide",
     "solver didn't model the root package's own replace/provide as satisfying a dependent's requirement"),
    (118, "update: php-64bit and lib-* platform packages missing from the solver",
     "none of these",
     "built-in platform-package completeness (php-64bit, lib-*), not the census's `config.platform` override field"),
    (119, "update: honour available-package-patterns so wpackagist isn't asked about every name",
     "repositories",
     "composer-type repository capability (`available-package-patterns`) not honoured"),
    (125, "Root package version: guess from git like Composer's VersionGuesser",
     "root-version guessing",
     "root `version` absent; guess-from-git never ran for installed.php/installed.json"),
    (128, "Root version guess differs from Composer on composer/composer and phpunit/phpunit checkouts",
     "branch-alias",
     "fix: \"apply extra.branch-alias to the root package's own aliases\""),
    (149, "Metapackage with no dist and no source fails install: shopware/conflicts",
     "package type",
     "same `type: metapackage` handling gap as #63, different project"),
    (158, "add and rm: the solve ignores --offline and the project's repositories",
     "repositories",
     "partial-update solve hard-coded Packagist, ignoring composer.json's own `repositories`"),
    (160, "add: bare-name constraint synthesis still resolves against Packagist only and ignores --offline",
     "repositories",
     "same repositories gap as #158, different call site (bare-name constraint synthesis)"),
    (161, "update, add, rm: composer-type repositories with a file:// URL are rejected",
     "repositories",
     "composer-type repository whose url is a local file:// mirror was rejected outright"),
    (172, "update: yii2-app-basic fails to resolve, viv sees only dev-master for yiisoft/yii2",
     "branch-alias",
     "fix: \"keep branch aliases in the dev-split second solve\" -- extra.branch-alias dropped when the pool was rebuilt"),
    (242, "solver: --ignore-platform-req(s) does not reach the solve, only the autoload write",
     "none of these",
     "CLI flag plumbing (fixed together with #300); not the census's `config.platform` manifest field"),
    (257, "install: dist URL placeholders (%prettyVersion%) are fetched literally",
     "none of these",
     "dist URL templating, not a resolution-rule feature"),
    (258, "install: lock check ignores replace/provide, refuses humhub/humhub",
     "replace/provide",
     "lock-freshness check didn't credit a locked package's replace/provide as satisfying a root requirement"),
    (261, "update: an inferred stability flag equal to minimum-stability is dropped",
     "dev-*/stability-flags",
     "lock's `stability-flags` differs when an inferred flag equals the root's own `minimum-stability`"),
    (267, "update: partial update fails when a held dependency is locked at a dev branch",
     "branch-alias",
     "fix: \"a held dev branch keeps its branch alias in a partial update\""),
    (282, "update rewrites a VCS package's source.url from SSH to HTTPS",
     "repositories",
     "vcs-type repository source.url normalisation differed from Composer's"),
    (283, "Contract break: the API-fallback path writes an https source.url where Composer writes ssh",
     "repositories",
     "same vcs-type source.url gap as #282, different code path (GitHub API unreachable)"),
    (294, "Inline package repositories are refused; 16 of 355 client merges cannot re-solve",
     "repositories",
     "`package`-type repository unsupported"),
    (300, "install: missing PHP extension required by the lock installs anyway, Composer refuses",
     "none of these",
     "ext-* platform requirement enforcement at install, not the census's `config.platform` field"),
    (304, "update: self.version in the root composer.json's own require still fails to parse",
     "replace/provide",
     "self.version substitution missing on the root's own require (#115 only covered a dependency's)"),
    (305, "update/install: a top-level path-type repository is refused outright",
     "repositories",
     "`path`-type repository unsupported"),
    (317, "install: installed.json keeps a Composer 1 lock's time format where Composer writes RFC 3339",
     "none of these",
     "output formatting (time field), not a resolution rule"),
    (318, "install: a mixed-case package name installs into a lowercase vendor directory",
     "none of these",
     "installer path casing, not a resolution rule"),
    (205, "update: config.bump-after-update is ignored",
     "none of these",
     "manifest-mutation feature (`config.bump-after-update`), not in the census's rule list"),
    (188, "Adopting a committed vendor/ rewrites autoload files with a different PSR-4 order",
     "none of these",
     "autoload generation order, not a resolution rule"),
    (175, "update: block versions with security advisories and abandoned packages by default",
     "none of these",
     "security-advisories/abandoned filtering, not in the census's rule list"),
    (322, "installed.php: a branch-aliased package's self.version replace lists only the branch, not the alias",
     "branch-alias",
     "named by the candidate; branch-alias interacting with a self.version replace"),
    (312, "update: the solve never uses the git-guessed root version, only installed.php does",
     "root-version guessing",
     "OPEN -- named by the candidate; root `version` absent, the solve (unlike installed.php) never guesses from git"),
    (316, "solver: craftcms/craft resolves yii2-shell 2.0.6 where Composer picks dev-master",
     "none of these",
     "OPEN -- named by the candidate; search-order/backjump interaction with the security-advisories feed, not one of the census's manifest features"),
]

EXCLUDED_CONSIDERED = [
    (293, "The pre-merge performance gate cannot fail on its current project", "bench-harness gate, not a viv/Composer divergence"),
    (148, "Compat sweep aborts on a failed clone instead of skipping the row", "compat/run.sh harness bug, not viv's own output"),
    (151, "Plugins refused on popular skeletons: TYPO3, CakePHP, Contao, Silverstripe, Bolt", "a record of known adapter gaps, not itself a fix (adapters tracked separately)"),
    (214, "install: --ignore-platform-reqs is unreachable, so a migrated build stage gets a platform_check.php Composer omits", "first stage of the same gap superseded by #242"),
    (231, "update/require/remove: --ignore-platform-reqs still unreachable", "second stage of the same gap superseded by #242"),
    (73, "parse_constraint panics on multi-byte input (semver-php slices at a non-char boundary)", "parser encoding robustness (non-ASCII constraint), not a resolution-rule divergence"),
]

ADAPTER_ISSUES = [51, 52, 53, 75, 92, 93, 98, 101, 126, 129, 130, 131, 157, 162, 218]


def render_part2() -> list[str]:
    lines = []
    lines.append("## Part 2: compatibility bugs by feature")
    lines.append("")
    lines.append(f"Selection rule: {SELECTION_RULE}")
    lines.append("")
    lines.append("| Issue | Title | Feature | Reason |")
    lines.append("|---|---|---|---|")
    for num, title, feature, reason in CLASSIFICATION:
        lines.append(f"| #{num} | {title} | {feature} | {reason} |")
    lines.append("")
    feature_counts: dict[str, int] = {}
    for _, _, feature, _ in CLASSIFICATION:
        feature_counts[feature] = feature_counts.get(feature, 0) + 1
    lines.append("### Totals by feature")
    lines.append("")
    for feature, n in sorted(feature_counts.items(), key=lambda kv: -kv[1]):
        lines.append(f"- {feature}: **{n}**")
    lines.append("")
    lines.append(f"{len(CLASSIFICATION)} issues classified ({sum(1 for f in feature_counts if f != 'none of these')} distinct resolution-rule features, {feature_counts.get('none of these', 0)} 'none of these').")
    lines.append("")
    lines.append("### Excluded but considered")
    lines.append("")
    for num, title, reason in EXCLUDED_CONSIDERED:
        lines.append(f"- #{num} {title} -- {reason}")
    lines.append(
        f"- Plugin-adapter completeness bugs (#{', #'.join(str(n) for n in ADAPTER_ISSUES)}): "
        "each is a real compatibility bug (a specific third-party plugin behaves differently under "
        "viv), but about that plugin, not a resolution rule this census counts; every one would "
        "classify `none of these`, so they're counted here rather than given a row each."
    )
    lines.append("")
    return lines


# --- Part 3: fixture and corpus coverage per feature ---
# Same feature list as Part 1 (ALL_COLS), asking a different question: does
# anything in viv's own test suite exercise it? Three sources, all static:
#  - tests/fixtures/**/composer.json (+ .before/.after) and a sibling
#    composer.lock, evaluated with the same features_for() as Part 1 --
#    nested manifests under a packages/ or vendor/ directory are dependency
#    fixtures, not the root under test, and are skipped.
#  - inline composer.json literals in tests/*.rs (whole file) and
#    src/**/*.rs (text from the first #[cfg(test)] marker on -- every test
#    module in this repo is one mod tests {..} block at file end, checked
#    by hand against the files INLINE_PATTERNS matched), one hit per file
#    per feature, keyed by the feature's JSON key or a quoted-string
#    literal.
#  - compat/corpus.toml projects, reusing Part 1's own per-project rows.
# `no-version` (a key's absence) and `lock-type≠lib/meta` (a lock package's
# `type` taking any value outside the two defaults) aren't literal
# substrings a grep can key on, so those two have no inline-literal source.

TESTS_DIR = REPO_ROOT / "tests"
SRC_DIR = REPO_ROOT / "src"
FIXTURES_DIR = TESTS_DIR / "fixtures"
FIXTURE_ROOT_NAMES = ("composer.json", "composer.json.before", "composer.json.after")

INLINE_PATTERNS: dict[str, re.Pattern] = {
    "min-stab": re.compile(r'"minimum-stability"'),
    "pref-stable": re.compile(r'"prefer-stable"'),
    "branch-alias(root)": re.compile(r'"branch-alias"'),
    "branch-alias(lock)": re.compile(r'"branch-alias"'),
    "replace": re.compile(r'"replace"'),
    "provide": re.compile(r'"provide"'),
    "self.version": re.compile(r'self\.version'),
    "no-packagist": re.compile(r'"packagist\.org"\s*:\s*false'),
    "conflict": re.compile(r'"conflict"'),
    "config.platform": re.compile(r'"platform"\s*:'),
    "allow-plugins": re.compile(r'"allow-plugins"'),
    "inline-alias(root)": re.compile(r'"[^"\n]* as [^"\n]*"'),
    "lock-aliases": re.compile(r'"aliases"'),
    "dev-*(root)": re.compile(r'"[^"\n]*dev-[^"\n]*"'),
    "stability-flags(lock)": re.compile(r'"stability-flags"'),
    "repo:vcs": re.compile(r'"type"\s*:\s*"vcs"'),
    "repo:path": re.compile(r'"type"\s*:\s*"path"'),
    "repo:composer": re.compile(r'"type"\s*:\s*"composer"'),
    "repo:package": re.compile(r'"type"\s*:\s*"package"'),
    "repo:artifact": re.compile(r'"type"\s*:\s*"artifact"'),
    "lock-dev-version": re.compile(r'"version"\s*:\s*"dev-'),
}


def _line_no(text: str, pos: int) -> int:
    return text.count("\n", 0, pos) + 1


def _is_nested_fixture(path: Path) -> bool:
    parts = path.relative_to(FIXTURES_DIR).parts[:-1]
    return "packages" in parts or "vendor" in parts


def fixture_root_pairs() -> list[tuple[Path, dict, dict]]:
    pairs = []
    for name in FIXTURE_ROOT_NAMES:
        for path in sorted(FIXTURES_DIR.rglob(name)):
            if _is_nested_fixture(path):
                continue
            try:
                root = json.loads(path.read_text(errors="replace"))
            except json.JSONDecodeError:
                continue
            lock_path = path.parent / "composer.lock"
            lock = json.loads(lock_path.read_text(errors="replace")) if lock_path.is_file() else {}
            pairs.append((path, root, lock))
    return pairs


def fixture_coverage() -> dict[str, list[Path]]:
    hits: dict[str, list[Path]] = {label: [] for _, label in ALL_COLS}
    for path, root, lock in fixture_root_pairs():
        feats = features_for(root, lock)
        for key, label in ALL_COLS:
            if feats[key]:
                hits[label].append(path)
    return hits


def inline_hits() -> dict[str, list[str]]:
    hits: dict[str, list[str]] = {label: [] for label in INLINE_PATTERNS}
    sources: list[tuple[Path, str, int]] = [(f, f.read_text(errors="replace"), 0) for f in sorted(TESTS_DIR.glob("*.rs"))]
    for f in sorted(SRC_DIR.rglob("*.rs")):
        text = f.read_text(errors="replace")
        idx = text.find("#[cfg(test)]")
        if idx != -1:
            sources.append((f, text, idx))
    for path, text, start in sources:
        rel = path.relative_to(REPO_ROOT)
        for label, pattern in INLINE_PATTERNS.items():
            m = pattern.search(text, start)
            if m:
                hits[label].append(f"{rel}:{_line_no(text, m.start())}")
    return hits


def corpus_examples(rows: list[dict]) -> dict[str, list[str]]:
    analysed = [r for r in rows if "skip" not in r]
    return {label: [r["name"] for r in analysed if r[key]] for key, label in ALL_COLS}


def render_part3(rows: list[dict], summary: dict) -> list[str]:
    lines = []
    lines.append("## Part 3: coverage")
    lines.append("")
    lines.append(
        "Scanned: `tests/fixtures/**/composer.json` (plus the "
        "`composer.json.before`/`.after` pairs, skipping manifests nested under "
        "a `packages/` or `vendor/` directory) and any sibling `composer.lock`, "
        "evaluated with the same `features_for` Part 1 uses; inline composer.json "
        "literals in `tests/*.rs` (whole file) and `src/**/*.rs` (text from the "
        "first `#[cfg(test)]` marker on, one hit per file), grepped per feature "
        "for its JSON key or a quoted-string literal; and `compat/corpus.toml` "
        "projects, reusing Part 1's per-project rows. `no-version` and "
        "`lock-type≠lib/meta` aren't literal substrings a grep can key on, "
        "so those two are fixture-file-only, with no inline-literal hits possible."
    )
    lines.append("")

    fx = fixture_coverage()
    il = inline_hits()
    corpus = corpus_examples(rows)
    n_ana = summary["projects_analysed"]

    lines.append("| feature | used by N of 20 | fixture files | corpus projects | example fixtures |")
    lines.append("|---|---|---|---|---|")
    used_no_fixture: list[str] = []
    fixture_no_use: list[str] = []
    for _, label in ALL_COLS:
        fixture_paths = [str(p.relative_to(REPO_ROOT)) for p in fx.get(label, [])]
        inline_paths = il.get(label, [])
        total_fixture = len(fixture_paths) + len(inline_paths)
        cproj = corpus.get(label, [])
        examples = (fixture_paths + inline_paths)[:3]
        lines.append(
            f"| {label} | {len(cproj)} of {n_ana} | {total_fixture} | "
            f"{', '.join(cproj) if cproj else '--'} | "
            f"{', '.join(examples) if examples else '--'} |"
        )
        if cproj and total_fixture == 0:
            used_no_fixture.append(label)
        if total_fixture > 0 and not cproj:
            fixture_no_use.append(label)
    lines.append("")
    lines.append(f"**used, no fixture** ({len(used_no_fixture)}): " + (", ".join(used_no_fixture) if used_no_fixture else "none") + ".")
    lines.append("")
    lines.append(f"**fixture, no use** ({len(fixture_no_use)}, informational): " + (", ".join(fixture_no_use) if fixture_no_use else "none") + ".")
    lines.append("")
    lines.append("### Reading")
    lines.append("")
    total_fixture_hits = sum(len(v) for v in fx.values())
    total_inline_hits = sum(len(v) for v in il.values())
    lines.append(
        f"Of the {len(ALL_COLS)} Part 1 features, {len(used_no_fixture)} are used by at least one corpus "
        f"project but have no fixture-file or inline-literal hit, and {len(fixture_no_use)} have fixture "
        f"coverage with no corpus project (of {n_ana} analysed) currently using them. The fixture-files "
        f"column combines {total_fixture_hits} tests/fixtures file hits and {total_inline_hits} inline "
        f"tests/*.rs and src/**/*.rs #[cfg(test)] literal hits (one hit per file per feature)."
    )
    lines.append("")
    return lines


def main() -> int:
    start = time.monotonic()
    rows = run()
    summary = summarise(rows)
    wall = time.monotonic() - start
    fetch_date = "2026-09-28"

    lines = []
    lines.append("# Candidate 3.5: simpler resolution rules, the measurement (#333)")
    lines.append("")
    lines.append(
        "How to reproduce: `make bench-g3-rules` (network: git clones for Part 1; Part 2 is a "
        f"fixed classification, not refetched). Closed-issue list fetched {fetch_date}."
    )
    lines.append("")
    lines.extend(render_part1(rows, summary))
    lines.extend(render_part2())
    lines.append("## Reading")
    lines.append("")
    n_ana = summary["projects_analysed"]
    bt = summary["bool_totals"]
    cnz = summary["count_nonzero_projects"]
    feature_counts: dict[str, int] = {}
    for _, _, feature, _ in CLASSIFICATION:
        feature_counts[feature] = feature_counts.get(feature, 0) + 1
    top_features = sorted((f for f in feature_counts if f != "none of these"), key=lambda f: -feature_counts[f])
    top_features_sentence = ", ".join(f"{f} {feature_counts[f]}" for f in top_features)
    lines.append(
        f"Of {n_ana} corpus projects with a committed lock, {bt['no-version']} have no root `version` "
        f"field (Composer guesses it from git on every one of them), {bt['branch-alias(root)']} set a root "
        f"`extra.branch-alias` and {cnz['branch-alias(lock)']} carry a locked package with one, "
        f"{bt['replace']} set root `replace` and {bt['provide']} set root `provide` "
        f"({bt['self.version']} of those use `self.version` as a value), {cnz['inline-alias(root)']} use an "
        f"inline alias (` as `) in a root constraint and {cnz['lock-aliases']} carry a non-empty lock "
        f"`aliases` array, {cnz['dev-*(root)']} have a root `dev-*` constraint and {cnz['stability-flags(lock)']} "
        f"a non-empty lock `stability-flags`, {bt['min-stab']} set a non-stable `minimum-stability` and "
        f"{bt['pref-stable']} set `prefer-stable`, {bt['conflict']} set root `conflict`, {bt['config.platform']} "
        f"set `config.platform` and {bt['allow-plugins']} set `config.allow-plugins`, and "
        f"{cnz['lock-type≠lib/meta']} carry a locked package whose type is neither library nor "
        f"metapackage. On the bug side, {len(CLASSIFICATION)} closed and candidate-named issues were "
        f"classified by feature ({top_features_sentence}, {feature_counts.get('none of these', 0)} "
        f"'none of these'), plus {len(ADAPTER_ISSUES)} plugin-adapter bugs folded into 'none of these' "
        f"rather than given a row each."
    )
    lines.append("")
    lines.append(f"Wall time: {wall:.1f}s.")
    lines.append("")
    lines.extend(render_part3(rows, summary))
    report = "\n".join(lines)
    print(report)

    scratch = scratch_dir()
    out_json = scratch / "features.json"
    out_json.write_text(json.dumps({"rows": rows, "summary": summary}, indent=2) + "\n")
    print(f"wall time: {wall:.1f}s", file=sys.stderr)
    print(f"JSON: {out_json}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
