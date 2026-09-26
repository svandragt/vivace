#!/usr/bin/env python3
"""Measure `viv update` against the committed lock (#308, candidate C):
how much of the time goes to fetching and solving packages that end up
unchanged, cold vs warm HTTP metadata cache, one leaf and one hub package
per project plus a no-argument full update.

Usage: bench/profile/update-lock.py [--runs N] [--only laravel,drupal,symfony_demo] [--json OUT]
Run inside devbox (`devbox run -- bench/profile/update-lock.py`) so
composer/miniserve/curl/jq resolve for bench/mirror.sh. Env: BENCH_WORK
(scratch dir, never ~/.cache/vivace), VIV (release binary).

Python, not sh, because this script's own job -- parsing `RUST_LOG=vivace=debug`
spans out of several hundred runs and diffing two composer.lock files -- is
regex and JSON work throughout; bench/mirror.sh and bench/run.sh already lean
on embedded python for exactly this, this is just the whole script instead of
a heredoc inside one.

Each project gets its own recorded mirror (bench/mirror.sh) served locally
(miniserve, same as bench/run.sh's BENCH_MIRROR path) so cold and warm cache
states never touch the real registry -- "benchmark only what we control".
"""

import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import time
import hashlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORK = Path(os.environ.get("BENCH_WORK", "/tmp/vivace-profile-update-lock"))
VIV = os.environ.get("VIV", str(ROOT / "target/release/viv"))
FLAGS = ["--no-plugins", "--no-scripts", "--no-install"]

# name, project dir (created on demand), hub package (few dependents but the
# framework itself -- large fan-out), leaf package (direct root requirement,
# few or no dependents and few of its own dependencies). Picked from each
# project's own composer.lock `require` graph (own-require / dependent counts
# in the PR/commit that added this script), not guessed.
PROJECTS = {
    "laravel": {
        "dir": ROOT / "bench/laravel",
        "hub": ("laravel/framework", "root require, 37 of its own deps, central to the whole graph"),
        "leaf": ("fakerphp/faker", "root require-dev, 2 of its own deps, 0 non-root dependents"),
    },
    "symfony_demo": {
        "checkout": {"repo": "https://github.com/symfony/demo.git", "commit": "920d86dc809f837543cb519d3df5b364a2c36577"},
        "hub": ("symfony/framework-bundle", "root require, 15 of its own deps, 6 packages depend on it"),
        "leaf": ("symfony/polyfill-intl-messageformatter", "root require, 0 deps, 0 dependents"),
    },
    "drupal_recommended-project": {
        "checkout": {"create_project": "11.4.6"},
        "hub": ("drupal/core-recommended", "root require, 45 of its own deps, wraps drupal/core"),
        "leaf": ("composer/installers", "root require, 0 deps, 0 dependents"),
    },
}


def sh(cmd, **kw):
    return subprocess.run(cmd, check=True, **kw)


def checkout_project(label, spec, checkout_root):
    d = checkout_root / label
    if d.exists():
        return d
    d.parent.mkdir(parents=True, exist_ok=True)
    if "repo" in spec:
        sh(["git", "clone", "--quiet", spec["repo"], str(d)])
        sh(["git", "-C", str(d), "checkout", "--quiet", spec["commit"]])
        shutil.rmtree(d / ".git")
    else:
        sh(
            [
                "composer",
                "create-project",
                "--no-install",
                "--no-scripts",
                "--no-interaction",
                "--ignore-platform-reqs",
                label.replace("_", "/", 1) if "/" not in label else label,
                str(d),
                spec["create_project"],
            ]
        )
    if not (d / "composer.lock").exists():
        sh(
            ["composer", "-d", str(d), "update", "--no-install", "--no-scripts", "--no-plugins", "--ignore-platform-reqs"]
        )
    return d


def dist_key(url, reference):
    return reference or hashlib.sha1(url.encode()).hexdigest()


def record_mirror(project_dir, mirror_dir):
    mirror_dir.mkdir(parents=True, exist_ok=True)
    sh([str(ROOT / "bench/mirror.sh"), str(project_dir), str(mirror_dir)])


def serve_mirror(mirror_dir, served_dir, log_path):
    if served_dir.exists():
        shutil.rmtree(served_dir)
    shutil.copytree(mirror_dir, served_dir)
    composer_cache = served_dir / ".composer-cache"
    if composer_cache.exists():
        shutil.rmtree(composer_cache)
    log_path.write_text("")
    proc = subprocess.Popen(
        ["miniserve", "--port", "0", "--interfaces", "127.0.0.1", str(served_dir)],
        stdout=open(log_path, "w"),
        stderr=subprocess.STDOUT,
    )
    port = None
    for _ in range(100):
        text = log_path.read_text()
        m = re.search(r"Bound to [0-9.]+:(\d+)", text)
        if m:
            port = int(m.group(1))
            break
        time.sleep(0.1)
    if port is None:
        proc.kill()
        raise RuntimeError(f"mirror server on {served_dir} did not start, see {log_path}")
    for p2 in (served_dir / "p2").rglob("*.json"):
        p2.write_text(p2.read_text().replace("__PORT__", str(port)))
    return proc, port


def rewrite_src(project_dir, port, src_dir):
    if src_dir.exists():
        shutil.rmtree(src_dir)
    src_dir.mkdir(parents=True)
    lock = json.loads((project_dir / "composer.lock").read_text())
    for key in ("packages", "packages-dev"):
        for pkg in lock.get(key, []):
            dist = pkg.get("dist")
            if not dist or not dist.get("url"):
                continue
            vendor, name = pkg["name"].split("/", 1)
            key_id = dist_key(dist["url"], dist.get("reference") or "")
            dist["url"] = f"http://127.0.0.1:{port}/dists/{vendor}/{name}/{key_id}.zip"
    (src_dir / "composer.lock").write_text(json.dumps(lock, indent=4))

    cj = json.loads((project_dir / "composer.json").read_text())
    cj["repositories"] = [{"type": "composer", "url": f"http://127.0.0.1:{port}"}, {"packagist.org": False}]
    cj.setdefault("config", {})["secure-http"] = False
    (src_dir / "composer.json").write_text(json.dumps(cj, indent=4))
    return src_dir


ANSI = re.compile(r"\x1b\[[0-9;]*m")
FIELD = re.compile(r"(\w+)=(\S+|\"[^\"]*\")")


def parse_log(log_text):
    """Pulls the phases #308 asked for out of `RUST_LOG=vivace=debug -v`
    output: one dict of elapsed_ms per named span, `requests` from the
    closure summary, and the package name behind every `/p2/...json` fetch
    (for the wasted-fetch share)."""
    phases = {
        "closure_ms": 0,
        "pool_build_ms": 0,
        "solve_ms": 0,
        "lock_write_ms": 0,
        "requests": None,
    }
    fetched_packages = []
    for raw in log_text.splitlines():
        line = ANSI.sub("", raw)
        fields = dict((k, v.strip('"')) for k, v in FIELD.findall(line))
        if "loaded metadata closure" in line:
            phases["closure_ms"] += int(fields.get("elapsed_ms", 0))
            phases["requests"] = int(fields.get("requests", 0))
        elif "detected platform packages" in line:
            phases["pool_build_ms"] += int(fields.get("elapsed_ms", 0))
        elif "converted the metadata closure into pool packages" in line:
            phases["pool_build_ms"] += int(fields.get("elapsed_ms", 0))
        elif "filtered the pool for advisories" in line:
            phases["pool_build_ms"] += int(fields.get("elapsed_ms", 0))
        elif "pruned the pool before rule generation" in line:
            phases["pool_build_ms"] += int(fields.get("elapsed_ms", 0))
        elif "solved pool" in line:
            phases["solve_ms"] += int(fields.get("rule_generation_ms", 0)) + int(fields.get("sat_ms", 0))
        elif "wrote composer.lock" in line:
            phases["lock_write_ms"] += int(fields.get("elapsed_ms", 0))
        elif "fetch hop (body complete)" in line:
            label = fields.get("label", "")
            m = re.search(r"/p2/([^/]+)/([^/]+?)(~dev)?\.json$", label)
            if m:
                fetched_packages.append(f"{m.group(1)}/{m.group(2)}")
    if phases["requests"] is None:
        phases["requests"] = len(fetched_packages)
    return phases, fetched_packages


def lock_by_name(lock_path):
    lock = json.loads(Path(lock_path).read_text())
    by_name = {}
    for section in ("packages", "packages-dev"):
        for pkg in lock.get(section, []):
            ref = (pkg.get("source") or {}).get("reference") or (pkg.get("dist") or {}).get("reference")
            by_name[pkg["name"]] = (pkg.get("version"), ref)
    return by_name


def run_once(run_dir, cache_dir, src_dir, package):
    shutil.copy(src_dir / "composer.json", run_dir / "composer.json")
    shutil.copy(src_dir / "composer.lock", run_dir / "composer.lock")
    args = [VIV, "update", "-d", str(run_dir), *FLAGS, "-v"]
    if package:
        args.append(package)
    env = dict(os.environ, RUST_LOG="vivace=debug", XDG_CACHE_HOME=str(cache_dir))
    started = time.perf_counter()
    proc = subprocess.run(args, env=env, capture_output=True, text=True)
    wall_ms = (time.perf_counter() - started) * 1000
    if proc.returncode != 0:
        raise RuntimeError(f"viv update failed ({run_dir}, package={package}):\n{proc.stderr[-4000:]}")
    return wall_ms, proc.stderr


def median(xs):
    return statistics.median(xs) if xs else None


def measure_cell(label, before_lock, run_dir, cache_dir, src_dir, package, cache_state, runs):
    if cache_state == "cold":
        wall_all, phase_all, survived_all, requests_all, total_all, wasted_all = [], [], [], [], [], []
        for _ in range(runs):
            if cache_dir.exists():
                shutil.rmtree(cache_dir)
            cache_dir.mkdir(parents=True)
            wall_ms, log_text = run_once(run_dir, cache_dir, src_dir, package)
            _collect(run_dir, before_lock, wall_ms, log_text, wall_all, phase_all, survived_all, requests_all, total_all, wasted_all)
    else:
        if cache_dir.exists():
            shutil.rmtree(cache_dir)
        cache_dir.mkdir(parents=True)
        run_once(run_dir, cache_dir, src_dir, package)  # warm-up, untimed
        wall_all, phase_all, survived_all, requests_all, total_all, wasted_all = [], [], [], [], [], []
        for _ in range(runs):
            wall_ms, log_text = run_once(run_dir, cache_dir, src_dir, package)
            _collect(run_dir, before_lock, wall_ms, log_text, wall_all, phase_all, survived_all, requests_all, total_all, wasted_all)
    return {
        "wall_ms": median(wall_all),
        "closure_ms": median([p["closure_ms"] for p in phase_all]),
        "pool_build_ms": median([p["pool_build_ms"] for p in phase_all]),
        "solve_ms": median([p["solve_ms"] for p in phase_all]),
        "lock_write_ms": median([p["lock_write_ms"] for p in phase_all]),
        "requests": median(requests_all),
        "packages_total": median(total_all),
        "packages_survived": median(survived_all),
        "surviving_share": median(survived_all) / median(total_all) if median(total_all) else None,
        "wasted_fetch": median(wasted_all),
        "wasted_share": (median(wasted_all) / median(requests_all)) if median(requests_all) else None,
        "runs": runs,
    }


def _collect(run_dir, before_lock, wall_ms, log_text, wall_all, phase_all, survived_all, requests_all, total_all, wasted_all):
    phases, fetched_packages = parse_log(log_text)
    after_lock = lock_by_name(run_dir / "composer.lock")
    survived = sum(1 for name, v in after_lock.items() if before_lock.get(name) == v)
    wasted = sum(1 for pkg in fetched_packages if before_lock.get(pkg) == after_lock.get(pkg))
    wall_all.append(wall_ms)
    phase_all.append(phases)
    survived_all.append(survived)
    total_all.append(len(after_lock))
    requests_all.append(phases["requests"])
    wasted_all.append(wasted)


def main():
    runs = int(os.environ.get("BENCH_RUNS", "5"))
    only = None
    args = sys.argv[1:]
    json_out = None
    i = 0
    while i < len(args):
        if args[i] == "--runs":
            runs = int(args[i + 1])
            i += 2
        elif args[i] == "--only":
            only = set(args[i + 1].split(","))
            i += 2
        elif args[i] == "--json":
            json_out = Path(args[i + 1])
            i += 2
        else:
            sys.exit(f"unknown argument: {args[i]}")

    WORK.mkdir(parents=True, exist_ok=True)
    checkout_root = WORK / "checkouts"
    results = {}
    procs = []
    try:
        for label, spec in PROJECTS.items():
            if only and label not in only:
                continue
            project_dir = spec.get("dir") or checkout_project(label, spec["checkout"], checkout_root)
            mirror_dir = WORK / f"mirror-{label}"
            record_mirror(project_dir, mirror_dir)
            served_dir = WORK / f"served-{label}"
            proc, port = serve_mirror(mirror_dir, served_dir, WORK / f"mirror-{label}.log")
            procs.append(proc)
            src_dir = rewrite_src(project_dir, port, WORK / f"src-{label}")
            before_lock = lock_by_name(src_dir / "composer.lock")

            run_dir = WORK / f"run-{label}"
            run_dir.mkdir(parents=True, exist_ok=True)
            project_results = {}
            for scenario, package in (("leaf", spec["leaf"][0]), ("hub", spec["hub"][0]), ("update", None)):
                for cache_state in ("cold", "warm"):
                    cache_dir = WORK / f"cache-{label}-{scenario}-{cache_state}"
                    cell = measure_cell(label, before_lock, run_dir, cache_dir, src_dir, package, cache_state, runs)
                    project_results[f"{scenario}/{cache_state}"] = cell
                    print(f"{label} {scenario}/{cache_state}: {json.dumps(cell)}", file=sys.stderr)
            results[label] = {
                "hub": spec["hub"],
                "leaf": spec["leaf"],
                "cells": project_results,
            }
    finally:
        for proc in procs:
            proc.kill()

    if json_out:
        json_out.write_text(json.dumps(results, indent=2))
    print(json.dumps(results, indent=2))


if __name__ == "__main__":
    main()
