#!/usr/bin/env python3
"""Chapter/candidate D (#309): how often does a lock solved on one PHP refuse
on the PHP that actually runs it, and does the project already carry a
`config.platform.php` guard for that gap?

Static only, no installs and no registry solves. For every project in
`compat/corpus.toml` and every public project in `compat/hunted.md` that has
a committed `composer.lock`, at the commit each file records (else the
default branch's current HEAD, noted as such):

  - clones the project shallowly into a scratch dir (network, read-only;
    reuses nothing local -- `bench/lockmerge`'s persistent BENCH_CACHE is a
    different corpus and out of scope here),
  - reads the committed `composer.json`, `composer.lock` and
    `.github/workflows/*.yml` at that commit,
  - computes the lock's PHP floor (highest lower bound of `php` across every
    locked package's own `require`, packages and packages-dev separately),
    `config.platform.php` and the lock's `platform-overrides`, and the CI
    matrix's PHP versions,
  - sweeps every PHP minor from the floor up to the newest CI-tested minor
    (plus the floor's own previous minor, kept separate as a sanity check --
    see below) and, for each, separates two different things a "refusal"
    could mean: (i) the root `composer.json`'s own `require.php` already
    refuses that PHP -- the project never claimed to support it, correct
    behaviour, not drift; (ii) root admits it but a locked package's own
    `require.php` refuses it anyway -- drift, the only kind this chapter
    counts. A drift instance is also classified prod vs packages-dev and
    upper-bound (the package caps out below a newer PHP, e.g. `~8.3.0`
    failing 8.5) vs lower-bound (the package needs newer than what's being
    tested -- shouldn't happen above the floor by construction, so a lower-
    bound drift there is itself a red flag, not just a result).

A CI matrix value with only two segments (`"8.2"`) is a minor, not a patch:
`shivammathur/setup-php` installs whatever the latest available 8.2.x patch
is at the time CI runs, not 8.2.0, and this script has no way to know which
patch that was on any given run. It is evaluated as satisfied if the
*highest* patch of that minor (approximated as `X.Y.999`) would satisfy the
constraint, since that is closer to what actually gets installed than the
literal `X.Y.0` -- a lower-bound constraint like `~8.2.27` is what this
flips (8.2.0 fails it, 8.2.999 doesn't). A three-segment CI value (`"8.2.3"`)
is an exact pin and is tested literally. Everything used for a floor or a
verdict, including this proxy, uses the same constraint checker
(`satisfies`) that mirrors `bench/lockmerge/run.py`'s `compare_versions`
(numeric-segment, dev-* incomparable) and additionally understands
`>=`/`<=`/`>`/`<`/`=`, `^`, `~`, wildcards (`7.4.*`/`7.4.x`), hyphenated
ranges (`7.4 - 8.1`), `|`/`||` (OR) and `,`/space (AND). Anything else is
"unparsed", never guessed.

`ext-*` requirements are recorded (locked packages vs root) but never used
for a refusal verdict: a static read of the repo has no way to know which
extensions the CI runner (or any other host) actually has loaded, only which
PHP version it ran, so an ext-* "refusal" would be a guess. Say so in the
write-up, don't fake a number.

Inputs (read-only, no writes outside PLATFORM_DRIFT_SCRATCH and the results
file):
  - compat/corpus.toml, compat/hunted.md -- transcribed into CORPUS below on
    2026-09-26; both files are curated by hand, not machine-readable enough
    to parse reliably, so a refresh of either needs a matching edit here.
  - network clone of each project's GitHub repo at a pinned commit (or HEAD).

Env:
  PLATFORM_DRIFT_SCRATCH  scratch dir for clones, default a mktemp -d
  PLATFORM_DRIFT_ONLY     comma-separated project names to run, skip the rest

Output: compat/results/platform-drift.md

Usage:
    compat/platform-drift.py
    compat/platform-drift.py --self-test
"""
from __future__ import annotations

import json
import os
import re
import subprocess
import sys
import tempfile
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# --- corpus --------------------------------------------------------------
# (name, repo url, `None` (=> resolve at runtime) or `NO_CHECKOUT`, commit or
# `None` (=> default branch HEAD), source). `source` records where the entry
# came from, for the write-up's corpus list. Local/anonymised entries in
# compat/hunted.md ("local project A".."K") are not public repos and are
# excluded, not guessed at.
#
# `compat/corpus.toml` entries carry their own authoritative `repo` verbatim.
# `compat/hunted.md` only records a package/project name, so its repo is
# resolved at runtime (`resolve_repo`): Packagist's source URL when the name
# is an installable package, else `github.com/<name>.git` literally, since
# that hunt's own "public applications" section used GitHub paths directly.
NO_CHECKOUT = "NO_CHECKOUT"

CORPUS: list[tuple[str, str | None, str | None, str]] = [
    # compat/corpus.toml
    ("laravel/laravel", "https://github.com/laravel/laravel.git",
     "aa0cf127fc365a56ee016867144ddffabc2290ae", "corpus.toml"),
    ("symfony/demo", "https://github.com/symfony/demo.git",
     "920d86dc809f837543cb519d3df5b364a2c36577", "corpus.toml"),
    ("drupal/recommended-project", NO_CHECKOUT, None, "corpus.toml"),  # create-project, no git checkout
    ("roots/bedrock", "https://github.com/roots/bedrock.git",
     "fb226251bf6d7aab1b7be632f769c0aa6afe4782", "corpus.toml"),
    ("composer/composer", "https://github.com/composer/composer.git",
     "85ae0251528d5f155548868e637ef6c68ed5be6b", "corpus.toml"),
    ("phpunit/phpunit", "https://github.com/sebastianbergmann/phpunit.git",
     "2c534d37d8a5edfc6cc885ef483d4a22db6a1d6e", "corpus.toml"),
    ("slimphp/Slim-Skeleton", "https://github.com/slimphp/Slim-Skeleton.git",
     "0ef01549870b3234a3a9f602904a39c3ed73f44c", "corpus.toml"),
    ("yiisoft/yii2-app-basic", "https://github.com/yiisoft/yii2-app-basic.git",
     "575120b0949c357c911f28b38ba9d4b6c978a15c", "corpus.toml"),
    ("statamic/statamic", "https://github.com/statamic/statamic.git",
     "d819487a4ce0ead2648dea32328060742ef5fbb4", "corpus.toml"),
    ("craftcms/craft", "https://github.com/craftcms/craft.git",
     "a1747d5be2a3d3045711c135938e1945fe754884", "corpus.toml"),
    # compat/hunted.md, 2026-09-08
    ("typo3/cms-base-distribution", None, "c374dbc", "hunted 2026-09-08"),
    ("cakephp/app", None, "d9feb07", "hunted 2026-09-08"),
    ("contao/managed-edition", None, "bc544bf", "hunted 2026-09-08"),
    ("silverstripe/installer", None, "2156492", "hunted 2026-09-08"),
    ("shopware/template", None, "d671d32", "hunted 2026-09-08"),
    ("octobercms/october", None, "623a2b9", "hunted 2026-09-08"),
    ("wp-cli/wp-cli-bundle", None, "789ab57", "hunted 2026-09-08"),
    ("bolt/project", None, "6f4aa21", "hunted 2026-09-08"),
    ("firstphp/ip2region", None, None, "hunted 2026-09-08 (sample)"),
    ("whoa-php/flute", None, None, "hunted 2026-09-08 (sample)"),
    ("accessd/yii2-rollbar", None, None, "hunted 2026-09-08 (sample)"),
    ("tsg/ar", None, None, "hunted 2026-09-08 (sample)"),
    ("tumtum/oxid-inline-translator", None, None, "hunted 2026-09-08 (sample)"),
    ("mozart/event-dispatcher", None, None, "hunted 2026-09-08 (sample)"),
    ("ahmadarif/laravel-pagination", None, None, "hunted 2026-09-08 (sample)"),
    ("tokimikichika/text-analysis", None, None, "hunted 2026-09-08 (sample)"),
    ("8xprovn/microservice", None, None, "hunted 2026-09-08 (sample)"),
    ("dmstr/api-configuration-bundle", None, None, "hunted 2026-09-08 (sample)"),
    ("numesia/all-my-sms", None, None, "hunted 2026-09-08 (sample)"),
    ("gamebetr/provable", None, None, "hunted 2026-09-08 (sample)"),
    ("thoughtco/statamic-cp-resources", None, None, "hunted 2026-09-08 (sample)"),
    ("gento-arg/module-oca", None, None, "hunted 2026-09-08 (sample)"),
    ("reedware/laravel-api", None, None, "hunted 2026-09-08 (sample)"),
    # compat/hunted.md, 2026-09-13 (update path; laravel/laravel, yiisoft/yii2-app-basic,
    # slimphp/Slim-Skeleton and drupal/recommended-project already listed above)
    ("symfony/skeleton", None, "c7e48b6", "hunted 2026-09-13"),
    ("api-platform/api-platform", None, "5152cb1", "hunted 2026-09-13"),
    ("laminas/laminas-mvc-skeleton", None, None, "hunted 2026-09-13"),
    ("spiral/app", None, "04ae9df", "hunted 2026-09-13"),
    ("codeigniter4/appstarter", None, "8d252c8", "hunted 2026-09-13"),
    ("doctrine/orm", None, "7d857bf", "hunted 2026-09-13"),
    # compat/hunted.md, 2026-09-15 (local project A-K excluded: anonymised, not public repos)
    ("phpmyadmin/phpmyadmin", None, "9e4dc5b", "hunted 2026-09-15"),
    ("matomo-org/matomo", None, "bdb35cf", "hunted 2026-09-15"),
    ("monicahq/monica", None, "e08e917", "hunted 2026-09-15"),
    ("koel/koel", None, "8befe78", "hunted 2026-09-15"),
    ("pixelfed/pixelfed", None, "472b4c4", "hunted 2026-09-15"),
    ("BookStackApp/BookStack", None, "b5641aa", "hunted 2026-09-15"),
    ("snipe/snipe-it", None, "16362cc", "hunted 2026-09-15"),
    ("mautic/mautic", None, "1f0a58c", "hunted 2026-09-15"),
    ("kimai/kimai", None, "c7b8f18", "hunted 2026-09-15"),
    ("firefly-iii/firefly-iii", None, "6143c0f", "hunted 2026-09-15"),
    ("pterodactyl/panel", None, "113ea43", "hunted 2026-09-15"),
    ("librenms/librenms", None, "c0950ba", "hunted 2026-09-15"),
    ("humhub/humhub", None, "ffb6701", "hunted 2026-09-15"),
    ("akaunting/akaunting", None, "50a0293", "hunted 2026-09-15"),
]

PHP_MINORS = [
    "5.3", "5.4", "5.5", "5.6",
    "7.0", "7.1", "7.2", "7.3", "7.4",
    "8.0", "8.1", "8.2", "8.3", "8.4", "8.5",
]


# --- version/constraint checking -----------------------------------------
# `compare_versions` below mirrors bench/lockmerge/run.py's function of the
# same name: numeric-segment comparison, dev branches incomparable. Built on
# top of it, `satisfies`/`lower_bound` add the constraint operators
# composer.lock's `require.php` fields actually use.

def _segments(v: str) -> list[str] | None:
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
    """-1/0/1 for v1 vs v2, or None when either side is incomparable."""
    if v1 is None or v2 is None:
        return None
    s1, s2 = _segments(v1), _segments(v2)
    if s1 is None or s2 is None:
        return None
    for a, b in zip(s1, s2):
        c = _cmp_segment(a, b)
        if c != 0:
            return c
    return (len(s1) > len(s2)) - (len(s1) < len(s2))


def _num_parts(v: str) -> list[int]:
    return [int(p) for p in v.split(".") if p != ""]


def _pad(parts: list[int], n: int) -> list[int]:
    return (parts + [0] * n)[:n]


def _cmp_numeric(a: list[int], b: list[int]) -> int:
    n = max(len(a), len(b))
    a, b = _pad(a, n), _pad(b, n)
    return (a > b) - (a < b)


NUM = r"\d+(?:\.\d+)*"
_TOKEN_RE = re.compile(
    rf"^(?:(?P<op>>=|<=|>|<|!=|=)(?P<v1>{NUM})"
    rf"|(?P<caret>\^)(?P<v2>{NUM})"
    rf"|(?P<tilde>~)(?P<v3>{NUM})"
    rf"|(?P<wv>{NUM})\.(?P<w>\*|[xX])"
    rf"|(?P<w2>\*|[xX])"
    rf"|(?P<bare>{NUM}))$"
)


class Unparsed(Exception):
    pass


def _bounds_for_token(tok: str) -> tuple[list[int] | None, list[int] | None]:
    """(lower, upper-exclusive) numeric-part bounds for one AND'd token, or
    raises Unparsed. Either bound may be None (unconstrained on that side)."""
    m = _TOKEN_RE.match(tok)
    if not m:
        raise Unparsed(tok)
    if m.group("op"):
        op, v = m.group("op"), _num_parts(m.group("v1"))
        if op == ">=":
            return v, None
        if op == ">":
            return v, None  # approximation: treated as >=, granularity is minors/patches
        if op == "<=":
            return None, v[:-1] + [v[-1] + 1]
        if op == "<":
            return None, v
        if op == "!=":
            return None, None
        if op == "=":
            return v, v[:-1] + [v[-1] + 1]
    if m.group("caret"):
        v = _num_parts(m.group("v2"))
        upper = [v[0] + 1] + [0] * (len(v) - 1) if v else [1]
        return v, upper
    if m.group("tilde"):
        v = _num_parts(m.group("v3"))
        idx = len(v) - 2 if len(v) >= 2 else 0
        upper = list(v[:idx]) + [v[idx] + 1] + [0] * (len(v) - idx - 1)
        return v, upper
    if m.group("wv") is not None:
        v = _num_parts(m.group("wv"))
        upper = v[:-1] + [v[-1] + 1]
        return v, upper
    if m.group("w2"):
        return None, None
    if m.group("bare"):
        v = _num_parts(m.group("bare"))
        return v, v[:-1] + [v[-1] + 1]
    raise Unparsed(tok)  # pragma: no cover - regex covers every named group


def _split_or(constraint: str) -> list[str]:
    # Composer's own parser accepts a single `|` as well as `||` for OR
    # (real corpus data uses both, e.g. dragonmantank/cron-expression's
    # `^8.2|^8.3|^8.4|^8.5`).
    return [g.strip() for g in re.split(r"\|\|?", constraint) if g.strip()]


def _split_and(group: str) -> list[str]:
    return [t for t in re.split(r"[\s,]+", group.strip()) if t]


_HYPHEN_RANGE_RE = re.compile(rf"({NUM})\s+-\s+({NUM})")


def _parse_group(group: str) -> list[tuple[list[int] | None, list[int] | None]]:
    """Every AND'd token's (lower, upper-exclusive) bounds, parsed up front --
    so an invalid token is always "unparsed", never masked by a bound that
    happens to decide the result first (a short-circuit would make the
    verdict depend on which version is being checked).

    A hyphenated range (`"8.1 - 8.5"`, real syntax per Composer's own version
    docs, inclusive on both ends) is matched before the plain AND split, since
    otherwise its dash reads as a separate, invalid token. The upper end is
    approximated the same way a bare/wildcard upper bound is elsewhere: the
    last given segment bumped by one (`"- 8.5"` admits all of the 8.5.x
    patch range, not only the exact version 8.5.0). An operator and its
    version may also carry whitespace between them (real corpus data:
    `">= 7"`), folded away before the hyphen/AND split so it isn't mistaken
    for two separate tokens."""
    group = re.sub(r"(>=|<=|!=|>|<|=|\^|~)\s+(?=\d)", r"\1", group)
    bounds: list[tuple[list[int] | None, list[int] | None]] = []
    for m in _HYPHEN_RANGE_RE.finditer(group):
        low = _num_parts(m.group(1))
        high = _num_parts(m.group(2))
        bounds.append((low, high[:-1] + [high[-1] + 1]))
    remainder = _HYPHEN_RANGE_RE.sub(" ", group)
    bounds.extend(_bounds_for_token(tok) for tok in _split_and(remainder))
    return bounds


def lower_bound(constraint: str) -> tuple[list[int] | None, bool]:
    """(lower bound as numeric parts, or None if unconstrained; unparsed?)."""
    try:
        groups = _split_or(constraint)
        if not groups:
            return None, True
        group_lows: list[list[int] | None] = []
        for g in groups:
            low = None
            for b_low, _high in _parse_group(g):
                if b_low is not None and (low is None or _cmp_numeric(b_low, low) > 0):
                    low = b_low
            group_lows.append(low)
        if any(low is None for low in group_lows):
            return None, False
        best = group_lows[0]
        for low in group_lows[1:]:
            if _cmp_numeric(low, best) < 0:
                best = low
        return best, False
    except Unparsed:
        return None, True


def satisfies(constraint: str, version: str) -> bool | None:
    """Whether `version` (e.g. "8.1.0") satisfies `constraint`. None means
    unparsed -- never guessed as True or False."""
    v = _num_parts(version)
    try:
        parsed_groups = [_parse_group(g) for g in _split_or(constraint)]
    except Unparsed:
        return None
    for bounds in parsed_groups:
        ok = True
        for low, high in bounds:
            if low is not None and _cmp_numeric(v, low) < 0:
                ok = False
                break
            if high is not None and _cmp_numeric(v, high) >= 0:
                ok = False
                break
        if ok:
            return True
    return False


def proxy_patch(v: str) -> str:
    """A bare minor (`"8.2"`) becomes `"8.2.999"`, standing in for whatever
    the highest available patch of that minor actually was on some past CI
    run (`setup-php` installs the latest patch, not `.0`, and this script has
    no way to know which patch that was). A version that already names a
    patch (`"8.2.3"`) is returned unchanged -- an exact pin is tested
    literally."""
    return v + ".999" if v.count(".") == 1 else v


def failure_kind(constraint: str, version: str) -> str | None:
    """Why `version` fails `constraint` (call only when `satisfies` is
    already `False`): `"upper"` if every OR'd group failed because `version`
    is at or past that group's upper bound (the package caps out before this
    PHP -- the classic "not updated for the new PHP yet" drift), `"lower"`
    if every group failed because `version` is below the group's lower bound
    (the package needs newer than what's being tested), `"mixed"` if
    different groups failed for different reasons, `None` if unparsed. Within
    one AND'd group this takes the first bound that trips, so a group with
    both an unmet lower and an unmet upper bound is reported by whichever
    bound its tokens list first -- a simplification worth this script's
    modest size, not a full constraint solver."""
    v = _num_parts(version)
    try:
        parsed_groups = [_parse_group(g) for g in _split_or(constraint)]
    except Unparsed:
        return None
    kinds: set[str] = set()
    for bounds in parsed_groups:
        kind = None
        for low, high in bounds:
            if low is not None and _cmp_numeric(v, low) < 0:
                kind = "lower"
                break
            if high is not None and _cmp_numeric(v, high) >= 0:
                kind = "upper"
                break
        if kind is None:
            return None  # this group actually matched; satisfies() should have been True
        kinds.add(kind)
    return kinds.pop() if len(kinds) == 1 else "mixed"


def fmt_bound(b: list[int] | None) -> str:
    return "-" if b is None else ".".join(str(p) for p in b)


def minor_of(parts: list[int]) -> str:
    return ".".join(str(p) for p in _pad(parts, 2))


def prev_next_minor(minor: str) -> tuple[str | None, str | None]:
    if minor not in PHP_MINORS:
        return None, None
    i = PHP_MINORS.index(minor)
    prev = PHP_MINORS[i - 1] if i > 0 else None
    nxt = PHP_MINORS[i + 1] if i < len(PHP_MINORS) - 1 else None
    return prev, nxt


# --- CI workflow PHP versions ----------------------------------------------

_PHP_VER_RE = re.compile(r"^\d+\.\d+(?:\.\d+)?$")  # a minor ("8.2") or an exact patch pin ("8.2.3")


def _flatten_php_values(node) -> list[str]:
    out: list[str] = []
    if isinstance(node, str):
        if _PHP_VER_RE.match(node.strip().lstrip("'\"")):
            out.append(node.strip())
    elif isinstance(node, (int, float)):
        out.append(str(node))
    elif isinstance(node, list):
        for item in node:
            out.extend(_flatten_php_values(item))
    return out


def _expr_ref(val: str) -> str | None:
    """`${{ <ref> }}` -> `<ref>`, or None if `val` isn't (only) one expression."""
    v = val.strip()
    if v.startswith("${{") and v.endswith("}}"):
        return v[3:-2].strip()
    return None


def ci_php_versions(workflows_dir: Path) -> tuple[list[str], bool]:
    """(sorted unique PHP versions named literally, unresolved-expression-seen?).
    `${{ matrix.X }}` resolves against that job's own `strategy.matrix` --
    its top-level lists and its `include` entries, both collected up front --
    and `${{ env.X }}` against the workflow's or job's own literal `env:`
    block. Anything else referencing `secrets.`/`github.`/a step output, or a
    matrix value itself computed (`fromJSON`, a generated list) is left
    unresolved, not guessed."""
    versions: set[str] = set()
    unresolved = False
    if not workflows_dir.is_dir():
        return [], False
    try:
        import yaml  # type: ignore
    except ImportError:
        yaml = None
    for wf in sorted(workflows_dir.glob("*.yml")) + sorted(workflows_dir.glob("*.yaml")):
        text = wf.read_text(errors="replace")
        if yaml is None:
            if "${{" in text and re.search(
                r"php(?:-version[s]?)?\s*:\s*.*\$\{\{", text, re.I
            ):
                unresolved = True
            for m in re.finditer(r"php(?:-versions?)?\s*:\s*(.+)", text, re.I):
                for tok in re.findall(r"\d+\.\d+", m.group(1)):
                    versions.add(tok)
            continue
        try:
            doc = yaml.safe_load(text)
        except Exception:
            unresolved = True
            continue
        if not isinstance(doc, dict):
            continue
        top_env = doc.get("env") if isinstance(doc.get("env"), dict) else {}
        for job in (doc.get("jobs") or {}).values():
            if not isinstance(job, dict):
                continue
            matrix = (job.get("strategy") or {}).get("matrix")
            if isinstance(matrix, str):
                unresolved = True  # the whole matrix is computed, e.g. fromJSON(...)
                matrix = {}
            elif not isinstance(matrix, dict):
                matrix = {}
            matrix_keys: set[str] = set()
            for key in ("php", "php-version", "php-versions"):
                if key in matrix:
                    matrix_keys.add(key)
                    val = matrix[key]
                    if isinstance(val, str):
                        unresolved = True  # e.g. fromJSON(...): a computed list
                    else:
                        versions.update(_flatten_php_values(val))
            for entry in matrix.get("include") or []:
                if not isinstance(entry, dict):
                    continue
                for key in ("php", "php-version", "php-versions"):
                    if key in entry:
                        matrix_keys.add(key)
                        versions.update(_flatten_php_values(entry[key]))
            job_env = job.get("env") if isinstance(job.get("env"), dict) else {}
            env = {**top_env, **job_env}
            for step in job.get("steps") or []:
                if not isinstance(step, dict):
                    continue
                with_ = step.get("with") or {}
                val = with_.get("php-version") or with_.get("php-versions")
                if isinstance(val, str):
                    ref = _expr_ref(val)
                    if ref is None:
                        versions.update(_flatten_php_values(val))
                    elif ref.startswith("matrix.") and ref.split(".", 1)[1] in matrix_keys:
                        pass  # already collected from strategy.matrix above
                    elif ref.startswith("env.") and ref.split(".", 1)[1] in env:
                        versions.update(_flatten_php_values(str(env[ref.split(".", 1)[1]])))
                    else:
                        unresolved = True
                elif val is not None:
                    versions.update(_flatten_php_values(val))
    return sorted(versions, key=lambda s: _num_parts(s)), unresolved


# --- per-project analysis --------------------------------------------------

@dataclass
class Drift:
    package: str
    constraint: str
    kind: str | None  # "upper" | "lower" | "mixed" | None (unparsed cause, shouldn't happen)
    dev: bool


@dataclass
class ProjectResult:
    name: str
    source: str
    commit: str
    skip: str | None = None
    prod_floor: list[int] | None = None
    dev_floor: list[int] | None = None
    floor_unparsed: bool = False
    config_platform_php: str | None = None
    lock_platform_overrides_php: str | None = None
    root_require_php: str | None = None
    ci_versions: list[str] = field(default_factory=list)
    ci_unresolved: bool = False
    ext_locked: set[str] = field(default_factory=set)
    ext_root: set[str] = field(default_factory=set)
    # PHP minor -> did this project's own root refuse it (not drift)?
    root_refuses: set[str] = field(default_factory=set)
    root_unparsed: set[str] = field(default_factory=set)
    # PHP minor -> drift instances (root admitted, a locked package refused anyway)
    drift: dict[str, list[Drift]] = field(default_factory=dict)
    ci_tested: set[str] = field(default_factory=set)  # subset of the sweep that came from CI, not just fill-in
    sanity_minor: str | None = None  # floor's own previous minor
    sanity_refuses: bool | None = None  # expected True by construction; False is a red flag


def _floor(packages: list[dict]) -> tuple[list[int] | None, bool]:
    best: list[int] | None = None
    unparsed = False
    for pkg in packages:
        c = (pkg.get("require") or {}).get("php")
        if not c:
            continue
        low, bad = lower_bound(c)
        unparsed = unparsed or bad
        if low is not None and (best is None or _cmp_numeric(low, best) > 0):
            best = low
    return best, unparsed


def _package_drift(packages: list[dict], dev: bool, version: str) -> list[Drift]:
    """Every locked package in `packages` whose own `require.php` refuses
    `version`, already proxied by the caller for a bare CI minor."""
    found = []
    for pkg in packages:
        c = (pkg.get("require") or {}).get("php")
        if not c:
            continue
        ok = satisfies(c, version)
        if ok is False:
            found.append(Drift(pkg.get("name", "?"), c, failure_kind(c, version), dev))
    return found


def analyse(name: str, source: str, project_dir: Path, commit_label: str) -> ProjectResult:
    pr = ProjectResult(name=name, source=source, commit=commit_label)
    lock_path = project_dir / "composer.lock"
    json_path = project_dir / "composer.json"
    if not lock_path.is_file():
        pr.skip = "no committed composer.lock"
        return pr
    lock = json.loads(lock_path.read_text(errors="replace"))
    root = json.loads(json_path.read_text(errors="replace")) if json_path.is_file() else {}

    packages = lock.get("packages") or []
    packages_dev = lock.get("packages-dev") or []
    pr.prod_floor, prod_bad = _floor(packages)
    pr.dev_floor, dev_bad = _floor(packages + packages_dev)
    pr.floor_unparsed = prod_bad or dev_bad

    pr.root_require_php = (root.get("require") or {}).get("php") or (lock.get("platform") or {}).get("php")
    pr.config_platform_php = ((root.get("config") or {}).get("platform") or {}).get("php")
    pr.lock_platform_overrides_php = (lock.get("platform-overrides") or {}).get("php")

    for pkg in packages:
        pr.ext_locked.update(k for k in (pkg.get("require") or {}) if k.startswith("ext-"))
    pr.ext_root.update(k for k in (root.get("require") or {}) if k.startswith("ext-"))

    pr.ci_versions, pr.ci_unresolved = ci_php_versions(project_dir / ".github" / "workflows")

    # Sweep: every PHP minor from the floor up to the newest CI-tested minor
    # (root can claim support anywhere in between, whether or not CI happens
    # to name that exact minor), plus every literal CI version (which may
    # itself be an exact patch pin, outside the bare-minor sweep). Falls back
    # to just the floor's next minor when there is no usable CI minor to
    # anchor the top of the range.
    floor_minor = minor_of(pr.prod_floor) if pr.prod_floor is not None else None
    ci_bare_minors = [v for v in pr.ci_versions if v.count(".") == 1]
    sweep: set[str] = set(pr.ci_versions)
    pr.ci_tested = set(pr.ci_versions)
    if floor_minor in PHP_MINORS:
        sweep.add(floor_minor)
        if ci_bare_minors:
            newest = max(ci_bare_minors, key=_num_parts)
            if newest in PHP_MINORS:
                i0, i1 = PHP_MINORS.index(floor_minor), PHP_MINORS.index(newest)
                if i1 >= i0:
                    sweep |= set(PHP_MINORS[i0 : i1 + 1])
        else:
            idx = PHP_MINORS.index(floor_minor)
            if idx + 1 < len(PHP_MINORS):
                sweep.add(PHP_MINORS[idx + 1])

        prev_m, _next_m = prev_next_minor(floor_minor)
        if prev_m:
            pr.sanity_minor = prev_m
            sanity_v = proxy_patch(prev_m)
            refuses = False
            if pr.root_require_php and satisfies(pr.root_require_php, sanity_v) is False:
                refuses = True
            if not refuses and _package_drift(packages + packages_dev, False, sanity_v):
                refuses = True
            pr.sanity_refuses = refuses

    for v in sorted(sweep, key=_num_parts):
        tv = proxy_patch(v)
        if not pr.root_require_php:
            root_admits = True
        else:
            root_ok = satisfies(pr.root_require_php, tv)
            if root_ok is None:
                pr.root_unparsed.add(v)
                continue
            root_admits = root_ok
        if not root_admits:
            pr.root_refuses.add(v)
            continue
        causers = _package_drift(packages, False, tv) + _package_drift(packages_dev, True, tv)
        if causers:
            pr.drift[v] = causers
    return pr


# --- cloning ---------------------------------------------------------------

def resolve_repo_candidates(name: str) -> list[str]:
    """Repo URLs to try, in order, for a `compat/hunted.md` name that carries
    no explicit `repo` (unlike `compat/corpus.toml`'s own entries): Packagist's
    recorded source URL when `name` is an installable package, then
    `github.com/<name>.git` literally (the 2026-09-15 hunt's "public
    applications" -- not installable packages -- used GitHub paths as the
    name directly)."""
    candidates = []
    try:
        req = urllib.request.Request(
            f"https://repo.packagist.org/p2/{name.lower()}.json",
            headers={"User-Agent": "vivace-platform-drift"},
        )
        with urllib.request.urlopen(req, timeout=20) as resp:
            data = json.load(resp)
        versions = (data.get("packages") or {}).get(name.lower()) or []
        if versions:
            src = (versions[0].get("source") or {}).get("url")
            if src:
                candidates.append(src)
    except (urllib.error.HTTPError, urllib.error.URLError, json.JSONDecodeError, TimeoutError):
        pass
    literal = f"https://github.com/{name}.git"
    if literal not in candidates:
        candidates.append(literal)
    return candidates


def clone_at(repo: str, commit: str | None, dest: Path, timeout: int = 300) -> tuple[bool, str]:
    """Partial clone (`--filter=blob:none`, `bench/lockmerge/run.py`'s own
    technique): full commit graph, blobs fetched on demand, so an abbreviated
    commit (`compat/hunted.md` only records 7 hex digits) still resolves
    locally without a full clone's blob weight. `-c filter.lfs.*` overrides
    the git-lfs filter to a no-op (`cat`, i.e. the raw pointer file) instead
    of running `git-lfs smudge`: this script only ever reads
    `composer.json`/`composer.lock`/`.github/workflows`, never an LFS asset,
    but a checkout still runs every configured filter on every tracked path,
    and fails outright when `git-lfs` itself isn't installed (no binary, not
    a skippable smudge) -- `compat/hunted.md` already records this for
    matomo-org/matomo."""
    dest.mkdir(parents=True, exist_ok=True)
    lfs_noop = ["-c", "filter.lfs.smudge=cat", "-c", "filter.lfs.process=", "-c", "filter.lfs.required=false"]
    run = lambda *a: subprocess.run(
        ["git", *lfs_noop, *a], cwd=dest, capture_output=True, text=True, timeout=timeout
    )
    r = run("clone", "--quiet", "--filter=blob:none", "--no-checkout", repo, ".")
    if r.returncode != 0:
        return False, r.stderr.strip().splitlines()[-1] if r.stderr.strip() else "clone failed"
    if commit is not None:
        r = run("checkout", "-q", commit)
        if r.returncode != 0:
            return False, r.stderr.strip().splitlines()[-1] if r.stderr.strip() else "checkout failed"
    else:
        r = run("checkout", "-q", "HEAD")
        if r.returncode != 0:
            return False, "checkout failed"
    head = run("rev-parse", "HEAD")
    return True, head.stdout.strip()[:12]


def run_corpus(only: set[str] | None) -> list[ProjectResult]:
    scratch = Path(os.environ.get("PLATFORM_DRIFT_SCRATCH") or tempfile.mkdtemp(prefix="platform-drift-"))
    results: list[ProjectResult] = []
    for name, repo, commit, source in CORPUS:
        if only and name not in only:
            continue
        if repo == NO_CHECKOUT:
            pr = ProjectResult(name=name, source=source, commit="-")
            pr.skip = "no installable git checkout (composer create-project, not a clone)"
            results.append(pr)
            continue
        candidates = [repo] if repo is not None else resolve_repo_candidates(name)
        safe = re.sub(r"[^A-Za-z0-9_.-]", "_", name)
        dest = scratch / safe
        ok, label = False, "no candidate repo URL"
        for candidate in candidates:
            if dest.exists():
                subprocess.run(["rm", "-rf", str(dest)], check=True)
            ok, label = clone_at(candidate, commit, dest)
            if ok:
                break
        if not ok:
            pr = ProjectResult(name=name, source=source, commit=commit or "HEAD")
            pr.skip = f"clone failed: {label}"
            results.append(pr)
            continue
        results.append(analyse(name, source, dest, label))
    return results


# --- write-up ----------------------------------------------------------

def write_report(results: list[ProjectResult], out: Path) -> None:
    now = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    lines: list[str] = []
    lines.append("# Platform drift across the corpus (#309)")
    lines.append("")
    lines.append(
        "Candidate D's measurement (`docs/research.md`): for every committed lock in"
        " the compat corpus, does a locked package's own `require.php` refuse a PHP"
        " version the project's root `composer.json` claims to support -- drift,"
        " the only thing this chapter counts -- across the PHP minors from the"
        " lock's own floor up to the newest minor its CI matrix names? Static read"
        " of `composer.json`, `composer.lock` and `.github/workflows/*.yml` at a"
        " pinned commit, no installs, no registry solve. Method and the constraint"
        " checker: `compat/platform-drift.py`."
    )
    lines.append("")
    lines.append(
        "The lock's `php` floor is the highest lower bound of `require.php` across"
        " every locked package (packages and packages-dev separately; the prod"
        " floor is the deploy-relevant one). `ext-*` requirements are recorded"
        " (locked packages vs root) but never scored: a static read has no way to"
        " know which extensions a given PHP actually has loaded, only which"
        " version it is."
    )
    lines.append("")
    lines.append(
        "A refusal is one of two different things, and only one of them is drift."
        " (i) The root `composer.json`'s own `require.php` already refuses a PHP:"
        " the project never claimed to support it, correct behaviour, not counted."
        " (ii) Root admits a PHP but a locked package's own `require.php` refuses"
        " it anyway: drift. Drift is checked against the full install (packages"
        " and packages-dev), since a default `composer install`/`viv install`"
        " installs dev requirements too and that is what a CI matrix normally"
        " runs; a require-dev-only drift is marked `[dev]` so it can be read"
        " differently by a project that installs `--no-dev` in production. Each"
        " drift instance also names whether the failing bound is upper (the"
        " package caps out before this PHP, e.g. `~8.3.0` failing 8.5 -- the"
        " package hasn't caught up yet) or lower (the package needs newer than"
        " what's being tested, which shouldn't happen above the floor by"
        " construction, so a lower-bound drift there is a red flag on the method,"
        " not just a result)."
    )
    lines.append("")
    lines.append(
        "A CI matrix value with only two segments (`\"8.2\"`) names a minor, not a"
        " patch: `setup-php` installs whichever 8.2.x patch is newest when CI"
        " actually runs, which this script cannot know after the fact. It is"
        " evaluated at the highest patch of that minor (`8.2.999`, a stand-in),"
        " since that is closer to reality than the literal `8.2.0` and is what a"
        " lower-bound constraint like `~8.2.27` needs to be correctly satisfied."
        " A three-segment CI value (`\"8.2.3\"`) is an exact pin, tested literally."
        " The floor's own previous minor is a separate sanity column, not a"
        " drift count: it is expected to refuse by construction (the floor is"
        " defined as the highest lower bound found), so a project where it"
        " doesn't would mean the floor computation itself is wrong."
    )
    lines.append("")
    lines.append(
        "A CI matrix that tests multiple historical branches of the project"
        " itself against different PHP versions (only `phpunit/phpunit`'s"
        " `nightly.yaml` in this corpus) still gets compared against the pinned"
        " commit's own lock for every PHP version it names, even on legs that"
        " actually run an older branch's own, different lock; read that row with"
        " that in mind."
    )
    lines.append("")
    lines.append(f"Run: {now}. Corpus: `compat/corpus.toml` and `compat/hunted.md`"
                  " (public projects only; anonymised local entries excluded).")
    lines.append("")

    lines.append("## Corpus")
    lines.append("")
    lines.append("| Project | Source | Commit |")
    lines.append("|---|---|---|")
    for pr in results:
        lines.append(f"| {pr.name} | {pr.source} | `{pr.commit}` |")
    lines.append("")

    examined = [pr for pr in results if pr.skip is None]
    skipped = [pr for pr in results if pr.skip is not None]
    no_lock = [
        pr for pr in skipped
        if pr.skip and ("composer.lock" in pr.skip or "no installable git checkout" in pr.skip)
    ]
    clone_failed = [pr for pr in skipped if pr.skip and pr.skip.startswith("clone failed")]

    lines.append("## Skipped")
    lines.append("")
    if skipped:
        lines.append("| Project | Reason |")
        lines.append("|---|---|")
        for pr in skipped:
            lines.append(f"| {pr.name} | {pr.skip} |")
    else:
        lines.append("None.")
    lines.append("")
    lines.append(
        f"{len(skipped)} of {len(results)} projects skipped: {len(no_lock)} with no"
        f" committed lock to read, {len(clone_failed)} whose clone failed."
    )
    lines.append("")

    lines.append("## Per-project")
    lines.append("")
    lines.append(
        "Drift cells mark a CI-tested minor with `*`; an unmarked one is a fill-in"
        " between the floor and the newest CI minor that CI itself doesn't name."
        " `[dev]` marks a packages-dev-only cause. Sanity is the floor's own"
        " previous minor: `refuses` is the expected case; `OK (!)` would flag a"
        " floor bug."
    )
    lines.append("")
    lines.append(
        "| Project | Prod floor | `config.platform.php` | CI PHP | Sanity (floor-1) |"
        " Root refuses (not drift) | Drift |"
    )
    lines.append("|---|---|---|---|---|---|---|")

    drift_on_ci = 0
    drift_in_range = 0
    drift_on_ci_with_config_platform = 0
    drift_in_range_with_config_platform = 0
    drift_prod = 0
    drift_dev = 0
    drift_upper = 0
    drift_lower = 0
    drift_mixed = 0
    unresolved_ci_count = 0
    unparsed_floor_count = 0
    sanity_ok_count = 0  # unexpected: floor-1 did NOT refuse

    for pr in examined:
        if pr.ci_unresolved:
            unresolved_ci_count += 1
        if pr.floor_unparsed:
            unparsed_floor_count += 1
        if pr.sanity_refuses is False:
            sanity_ok_count += 1

        has_ci_drift = bool(pr.ci_tested & pr.drift.keys())
        has_range_drift = bool(pr.drift)
        if has_range_drift:
            drift_in_range += 1
            if pr.config_platform_php:
                drift_in_range_with_config_platform += 1
        if has_ci_drift:
            drift_on_ci += 1
            if pr.config_platform_php:
                drift_on_ci_with_config_platform += 1
        for causers in pr.drift.values():
            for d in causers:
                drift_dev += 1 if d.dev else 0
                drift_prod += 0 if d.dev else 1
                if d.kind == "upper":
                    drift_upper += 1
                elif d.kind == "lower":
                    drift_lower += 1
                else:
                    drift_mixed += 1

        floor_str = fmt_bound(pr.prod_floor) if pr.prod_floor else "-"
        if pr.dev_floor is not None and pr.dev_floor != pr.prod_floor:
            floor_str += f" (dev {fmt_bound(pr.dev_floor)})"
        cfg = pr.config_platform_php or "-"
        if pr.lock_platform_overrides_php and pr.lock_platform_overrides_php != pr.config_platform_php:
            cfg += f" (lock: {pr.lock_platform_overrides_php})"
        ci_str = ", ".join(pr.ci_versions) if pr.ci_versions else ("unresolved" if pr.ci_unresolved else "-")
        if pr.ci_unresolved and pr.ci_versions:
            ci_str += " (+unresolved)"
        sanity_str = "-"
        if pr.sanity_minor:
            sanity_str = f"{pr.sanity_minor}: {'refuses' if pr.sanity_refuses else 'OK (!)'}"
        root_refuses_str = ", ".join(sorted(pr.root_refuses, key=_num_parts)) or "-"
        if pr.root_unparsed:
            root_refuses_str += (
                "; " if root_refuses_str != "-" else ""
            ) + "unparsed: " + ", ".join(sorted(pr.root_unparsed, key=_num_parts))
        if pr.drift:
            parts = []
            for v in sorted(pr.drift, key=_num_parts):
                mark = "*" if v in pr.ci_tested else ""
                causers = "; ".join(
                    f"{d.package} ({d.constraint}, {d.kind}{', dev' if d.dev else ''})"
                    for d in pr.drift[v]
                )
                parts.append(f"{v}{mark}: {causers}")
            drift_str = " | ".join(parts)
        else:
            drift_str = "none"
        lines.append(
            f"| {pr.name} | {floor_str} | {cfg} | {ci_str} | {sanity_str} |"
            f" {root_refuses_str} | {drift_str} |"
        )

    lines.append("")
    lines.append("## Totals")
    lines.append("")
    lines.append(f"- Projects examined: {len(examined)}")
    lines.append(f"- Projects skipped: {len(skipped)} ({len(no_lock)} no lock, {len(clone_failed)} clone failed)")
    lines.append(f"- Projects with drift on a CI-tested PHP: {drift_on_ci}")
    lines.append(f"  - of those, `config.platform.php` already set: {drift_on_ci_with_config_platform}")
    lines.append(
        f"- Projects with drift anywhere root admits, floor up to the newest CI minor: {drift_in_range}"
    )
    lines.append(f"  - of those, `config.platform.php` already set: {drift_in_range_with_config_platform}")
    lines.append(
        f"- Drift instances: {drift_prod} in a prod package, {drift_dev} in a"
        " packages-dev-only package (dev deps normally install in CI too, so"
        " both are live failures there; only the prod count is a `--no-dev`"
        " production risk)"
    )
    lines.append(
        f"- Drift instances by bound: {drift_upper} upper (package capped below a"
        f" newer PHP), {drift_lower} lower (shouldn't happen above the floor --"
        " see sanity below), "
        f"{drift_mixed} mixed"
    )
    lines.append(
        f"- Sanity check: floor-1 unexpectedly did not refuse in {sanity_ok_count} of"
        f" {sum(1 for pr in examined if pr.sanity_minor)} projects with a checkable floor"
        " (0 expected; a non-zero count would flag a bug in the floor computation)"
    )
    lines.append(f"- Projects with an unresolved CI PHP expression: {unresolved_ci_count}")
    lines.append(f"- Projects with an unparsed `php` constraint in the floor computation: {unparsed_floor_count}")
    lines.append("")

    out.write_text("\n".join(lines) + "\n")


# --- self-test ---------------------------------------------------------

def self_test() -> None:
    assert compare_versions("1.10.0", "1.9.0") == 1
    assert compare_versions("dev-main", "1.0.0") is None

    assert satisfies(">=7.2.5", "7.2.5") is True
    assert satisfies(">=7.2.5", "7.2.4") is False
    assert satisfies(">= 7", "7.0.0") is True  # space between operator and version, real corpus syntax
    assert satisfies(">= 7", "6.9.0") is False
    assert satisfies("^7.4", "7.9.9") is True
    assert satisfies("^7.4", "8.0.0") is False
    assert satisfies("^7.4", "7.3.9") is False
    assert satisfies("~7.4", "7.9.0") is True
    assert satisfies("~7.4", "8.0.0") is False
    assert satisfies("~7.4.2", "7.4.9") is True
    assert satisfies("~7.4.2", "7.5.0") is False
    assert satisfies("7.4.*", "7.4.30") is True
    assert satisfies("7.4.*", "7.5.0") is False
    assert satisfies("7.*", "7.9.9") is True
    assert satisfies("7.*", "8.0.0") is False
    assert satisfies("^7.2 || ^8.0", "8.1.0") is True
    assert satisfies("^7.2 || ^8.0", "9.0.0") is False
    assert satisfies("^8.2|^8.3|^8.4", "8.3.0") is True  # single-pipe OR, real corpus syntax
    assert satisfies("^8.2|^8.3|^8.4", "7.9.0") is False
    assert satisfies(">=7.1,<8.0", "7.5.0") is True
    assert satisfies(">=7.1,<8.0", "8.0.0") is False
    assert satisfies(">=7.1 <8.0", "7.9.0") is True
    assert satisfies("*", "5.0.0") is True
    assert satisfies("8.1 - 8.5", "8.5.9") is True  # inclusive of the whole 8.5.x range
    assert satisfies("8.1 - 8.5", "8.6.0") is False
    assert satisfies("8.1 - 8.5", "8.0.9") is False
    assert satisfies("dev-main", "8.1.0") is None
    assert satisfies("1.0.0-beta1", "1.5.0") is None  # stability suffix: unparsed, not guessed

    low, bad = lower_bound("^7.2 || ^8.0")
    assert bad is False and low == [7, 2]
    low, bad = lower_bound(">=7.1 <8.0")
    assert bad is False and low == [7, 1]
    low, bad = lower_bound("*")
    assert bad is False and low is None
    low, bad = lower_bound("8.1 - 8.5")
    assert bad is False and low == [8, 1]
    low, bad = lower_bound(">= 7")
    assert bad is False and low == [7]
    low, bad = lower_bound("1.0.0-beta1")
    assert bad is True

    assert prev_next_minor("7.4") == ("7.3", "8.0")
    assert prev_next_minor("8.4") == ("8.3", "8.5")

    # Latest-patch rule (#309 review): a bare CI minor is proxied to its
    # highest patch, since setup-php installs the latest patch of a minor, not
    # `.0`; an exact pin (three segments) is left alone.
    assert proxy_patch("8.2") == "8.2.999"
    assert proxy_patch("8.2.3") == "8.2.3"
    assert satisfies("~8.2.27", proxy_patch("8.2")) is True  # would be False at the literal 8.2.0
    assert satisfies("~8.2.27", "8.2.3") is False  # an exact pin is tested literally, no proxy
    assert satisfies(">=8.4.1", proxy_patch("8.4")) is True  # symfony/asset-shaped case

    # failure_kind: which bound a failing constraint tripped on.
    assert failure_kind("~8.3.0", "8.5.0") == "upper"  # package capped below a newer PHP
    assert failure_kind(">=8.5.0", "8.4.0") == "lower"  # package needs newer than what's tested
    assert failure_kind("~8.2.0 || ~8.3.0", "8.5.0") == "upper"
    assert failure_kind("dev-main", "8.1.0") is None  # unparsed, not guessed
    print("self-test ok")


def main() -> int:
    if "--self-test" in sys.argv:
        self_test()
        return 0
    only = None
    if os.environ.get("PLATFORM_DRIFT_ONLY"):
        only = {n.strip() for n in os.environ["PLATFORM_DRIFT_ONLY"].split(",")}
    results = run_corpus(only)
    out = REPO_ROOT / "compat" / "results" / "platform-drift.md"
    write_report(results, out)
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
