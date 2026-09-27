#!/usr/bin/env python3
"""Candidate 3.1 (#329), Part A: how many corpus projects need a PHP version
or extension set the Ubuntu 24.04 baseline PHP lacks, and how many extra apt
packages would a managed per-project PHP save someone from installing by
hand?

Static only: reads composer.json, composer.lock and .github/workflows/*.yml
at a pinned commit, no installs, no registry solve. Reuses
compat/platform-drift.py's CORPUS, clone_at, resolve_repo_candidates,
analyse, satisfies and _package_drift (a hyphenated filename can't be
`import`ed, so it's loaded by path -- the same technique
compat/lock-age.py uses). That gives the same project selection
platform-drift.py's own run already made
(compat/results/platform-drift.md): compat/corpus.toml plus the public
projects in compat/hunted.md, narrowed by `analyse()` to the ones that
commit a composer.lock. This script adds two things that report doesn't
print: require-dev's own ext-* requirements (analyse() only records
packages-dev's, not root require-dev's), and a check pinned at PHP 8.3
specifically (platform-drift's own sweep only covers the range a project's
CI actually tests, which for some projects never includes 8.3).

BASELINE_PHP/BASELINE_EXT/EXT_TO_APT_PACKAGE are measured, not assumed --
see bench/results/g3-toolchain.md "Baseline" for the container commands
that produced them (`apt-get install -y php-cli`, `php -v`, `php -m`,
`apt-cache search php8.3-`, all in `ubuntu:24.04`).

Env:
  PART_A_SCRATCH        scratch dir for clones, default a mktemp -d
  PLATFORM_DRIFT_ONLY   comma-separated project names to run, skip the rest
                        (shared with platform-drift.py's own env var, so a
                        narrow rerun of both scripts uses one setting)

Output: bench/results/g3-toolchain.raw.json, and a summary on stdout.

Usage (from a clean checkout, needs network for the clones):
    python3 bench/g3-toolchain/static.py
"""
from __future__ import annotations

import importlib.util
import json
import os
import re
import sys
import tempfile
from collections import Counter
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent

_spec = importlib.util.spec_from_file_location("platform_drift", REPO_ROOT / "compat" / "platform-drift.py")
platform_drift = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
sys.modules["platform_drift"] = platform_drift
_spec.loader.exec_module(platform_drift)

# --- baseline, measured 2026-09-27 in ubuntu:24.04 (see g3-toolchain.md) ---

BASELINE_PHP = "8.3.6"

# `php -m` output, lowercased, "Core" and the duplicate "Zend OPcache"
# zend-module entry dropped; "standard" isn't a real ext-* a project
# requires, kept out too.
BASELINE_EXT = {
    "ext-calendar", "ext-ctype", "ext-date", "ext-exif", "ext-ffi",
    "ext-fileinfo", "ext-filter", "ext-ftp", "ext-gettext", "ext-hash",
    "ext-iconv", "ext-json", "ext-libxml", "ext-openssl", "ext-pcntl",
    "ext-pcre", "ext-pdo", "ext-phar", "ext-posix", "ext-random",
    "ext-readline", "ext-reflection", "ext-session", "ext-shmop",
    "ext-sockets", "ext-sodium", "ext-spl", "ext-sysvmsg", "ext-sysvsem",
    "ext-sysvshm", "ext-tokenizer", "ext-zlib", "ext-opcache",
}

# `apt-cache search php8.3-` in the same container, restricted to the
# extensions this corpus actually asks for (`ext_missing` below); an
# extension this dict doesn't cover is reported as `UNKNOWN(ext-x)`, not
# guessed at. `ext-dom`/`ext-simplexml`/`ext-xml`/`ext-xmlwriter`/
# `ext-xmlreader` all ship in the one `php8.3-xml` package; `ext-xsl` is a
# separate ("dummy") package per the search output.
EXT_TO_APT_PACKAGE = {
    "ext-mbstring": "php8.3-mbstring",
    "ext-curl": "php8.3-curl",
    "ext-dom": "php8.3-xml",
    "ext-simplexml": "php8.3-xml",
    "ext-xml": "php8.3-xml",
    "ext-xmlwriter": "php8.3-xml",
    "ext-xmlreader": "php8.3-xml",
    "ext-xsl": "php8.3-xsl",
    "ext-intl": "php8.3-intl",
    "ext-gd": "php8.3-gd",
    "ext-zip": "php8.3-zip",
    "ext-mysqli": "php8.3-mysql",
    "ext-pdo_mysql": "php8.3-mysql",
    "ext-pgsql": "php8.3-pgsql",
    "ext-pdo_pgsql": "php8.3-pgsql",
    "ext-sqlite3": "php8.3-sqlite3",
    "ext-pdo_sqlite": "php8.3-sqlite3",
    "ext-bcmath": "php8.3-bcmath",
    "ext-soap": "php8.3-soap",
    "ext-ldap": "php8.3-ldap",
    "ext-imap": "php8.3-imap",
    "ext-redis": "php8.3-redis",
    "ext-imagick": "php8.3-imagick",
    "ext-gmp": "php8.3-gmp",
    "ext-bz2": "php8.3-bz2",
    "ext-apcu": "php8.3-apcu",
    "ext-memcached": "php8.3-memcached",
    "ext-mongodb": "php8.3-mongodb",
    "ext-tidy": "php8.3-tidy",
    "ext-amqp": "php8.3-amqp",
    "ext-ds": "php8.3-ds",
    "ext-igbinary": "php8.3-igbinary",
    "ext-msgpack": "php8.3-msgpack",
    "ext-snmp": "php8.3-snmp",
    "ext-uuid": "php8.3-uuid",
    "ext-yaml": "php8.3-yaml",
}


def dev_ext(project_dir: Path) -> set[str]:
    """ext-* named by packages-dev's own `require` or root's `require-dev`,
    which `platform_drift.analyse` doesn't record (it only reads packages
    and root require)."""
    lock = json.loads((project_dir / "composer.lock").read_text(errors="replace"))
    root_path = project_dir / "composer.json"
    root = json.loads(root_path.read_text(errors="replace")) if root_path.is_file() else {}
    packages_dev = lock.get("packages-dev") or []
    found = {k for pkg in packages_dev for k in (pkg.get("require") or {}) if k.startswith("ext-")}
    found |= {k for k in (root.get("require-dev") or {}) if k.startswith("ext-")}
    return found


def excludes_baseline(project_dir: Path, pr) -> bool:
    """Does PHP 8.3 fail this project: either root `require.php` refuses it,
    or a locked package's own `require.php` does (platform_drift's drift
    check), pinned at 8.3 specifically rather than platform-drift's
    CI-bounded sweep (which, for a project whose CI never tests up to 8.3,
    never checks 8.3 at all)."""
    lock = json.loads((project_dir / "composer.lock").read_text(errors="replace"))
    packages = lock.get("packages") or []
    packages_dev = lock.get("packages-dev") or []
    target = "8.3.999"
    if pr.root_require_php:
        root_ok = platform_drift.satisfies(pr.root_require_php, target)
        if root_ok is False:
            return True
    drift = platform_drift._package_drift(packages, False, target) + platform_drift._package_drift(
        packages_dev, True, target
    )
    return bool(drift)


def run() -> list[dict]:
    scratch = Path(os.environ.get("PART_A_SCRATCH") or tempfile.mkdtemp(prefix="g3-toolchain-a-"))
    only = None
    if os.environ.get("PLATFORM_DRIFT_ONLY"):
        only = {n.strip() for n in os.environ["PLATFORM_DRIFT_ONLY"].split(",")}
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
        # PHP extension names are case-insensitive (both to the engine and
        # to Composer's own platform check); composer.json authors don't
        # agree on a case, so normalise before any set comparison.
        ext_prod = {e.lower() for e in (set(pr.ext_root) | set(pr.ext_locked))}
        ext_dev = {e.lower() for e in dev_ext(dest)}
        ext_all = ext_prod | ext_dev
        ext_missing = sorted(e for e in ext_all if e not in BASELINE_EXT)
        apt_extra = sorted({EXT_TO_APT_PACKAGE.get(e, f"UNKNOWN({e})") for e in ext_missing})
        ci_non83 = sorted(v for v in pr.ci_versions if not (v == "8.3" or v.startswith("8.3.")))
        rows.append(
            {
                "name": name,
                "source": source,
                "commit": label,
                "prod_floor": ".".join(str(x) for x in pr.prod_floor) if pr.prod_floor else None,
                "root_require_php": pr.root_require_php,
                "config_platform_php": pr.config_platform_php,
                "ci_versions": pr.ci_versions,
                "ci_non83": ci_non83,
                "excludes_baseline_83": excludes_baseline(dest, pr),
                "ext_prod": sorted(ext_prod),
                "ext_dev_only": sorted(ext_dev - ext_prod),
                "ext_missing_vs_baseline": ext_missing,
                "apt_packages_extra": apt_extra,
            }
        )
    return rows


def summarise(rows: list[dict]) -> dict:
    analysed = [r for r in rows if "skip" not in r]
    excl_php = [r for r in analysed if r["excludes_baseline_83"]]
    ci_non83 = [r for r in analysed if r["ci_non83"]]
    need_ext = [r for r in analysed if r["ext_missing_vs_baseline"]]
    ext_counter: Counter[str] = Counter()
    for r in analysed:
        ext_counter.update(r["ext_missing_vs_baseline"])
    apt_counts = [len(r["apt_packages_extra"]) for r in analysed]
    return {
        "projects_total": len(rows),
        "projects_analysed": len(analysed),
        "projects_skipped": len(rows) - len(analysed),
        "excludes_baseline_83": len(excl_php),
        "ci_tests_non_83": len(ci_non83),
        "needs_missing_extension": len(need_ext),
        "extension_ranking": ext_counter.most_common(),
        "apt_packages_extra_per_project": apt_counts,
        "apt_packages_extra_median": sorted(apt_counts)[len(apt_counts) // 2] if apt_counts else 0,
    }


def main() -> int:
    rows = run()
    summary = summarise(rows)
    out = REPO_ROOT / "bench" / "results" / "g3-toolchain.raw.json"
    out.write_text(json.dumps({"projects": rows, "summary": summary}, indent=2) + "\n")
    print(f"wrote {out}")
    print(json.dumps(summary, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
