#!/usr/bin/env python3
"""Candidate 3.1 (#329), Part B: time a fresh container to a first
successful `composer install --no-dev --no-scripts --no-plugins` three
ways -- Ubuntu packages, a third-party repo (ondrej/php PPA) for a non-8.3
PHP, and a static-php-cli binary -- with the network excluded from the
timed part (the benchmark rule: only measure what we control).

Three projects (bench/results/g3-toolchain.md says why these three):
  composer/composer    needs only the baseline PHP (no extra extension)
  BookStackApp/BookStack  needs five extensions beyond baseline
  symfony/demo          needs PHP >=8.4, which Ubuntu 24.04 doesn't ship

Inputs: network, to clone the three projects at Part A's pinned commits
(reusing compat/platform-drift.py's CORPUS/clone_at) and to warm the local
apt repo, the ondrej/php PPA snapshot, the static-php-cli tarballs and the
shared Composer cache -- all untimed. `docker` on PATH.

Outputs: <work>/results.jsonl (one row per timed run) and a summary printed
to stdout; bench/results/g3-toolchain.md's Part B table is transcribed from
this by hand (the write-up also carries the "why these three" prose this
script doesn't).

Design notes (see bench/results/g3-toolchain.md for the "what was excluded
and why" version):
  - The apt repo is pre-downloaded .debs + a flat `dpkg-scanpackages` index,
    read over `file://` -- no server container needed for it, unlike the
    static tarballs (an actual HTTP server, since the task asks the static
    way to be "served from a local HTTP server").
  - Ubuntu's own php8.3 and ondrej's php8.4 are downloaded into SEPARATE
    local repos: mixing them let apt pick ondrej's rebuild of php8.3 too
    (higher version, same package name), which would stop measuring
    "Ubuntu's own packages" at all.
  - `--download-only` in a container that has *not* already installed
    other packages: apt only downloads a dependency that isn't already
    satisfied, so a warm container that installed `software-properties-
    common` first (to add the PPA) silently absorbs shared libraries
    (libxml2, libsqlite3-0, ucf...) into "already installed" and never
    copies their .deb -- then a fresh, unrelated timed container can't
    find them in the local-only repo. The PPA is added by writing back the
    exact `*.sources` file a prior `add-apt-repository` produced (captured
    once, `ondrej.sources` below), never by installing
    software-properties-common in the same container that downloads.
  - Composer itself needs the zip extension or a system unzip/7z binary to
    extract dist archives, regardless of what the target project's own
    composer.json declares -- a real, measured gap between "what the
    project needs" (Part A) and "what a working `composer install` needs".
    `unzip` (one universal apt package, not a PHP extension) is added to
    every apt/ppa install for that reason; static-php-cli's own build
    already bundles the zip extension.
  - The shared COMPOSER_CACHE_DIR is mounted read-write, not read-only:
    empirically, Composer skips a read-only cache entirely and re-fetches
    over the network instead of erroring. Its content doesn't change
    across the run (checked by sha1sum of every file before/after), so the
    "one cache, identical for all three ways" requirement still holds.

Usage (from a clean checkout, `docker` on PATH, needs network until the
"measure" phase):
    python3 bench/g3-toolchain/setup-time.py warm      # ~5-8 min, real network
    python3 bench/g3-toolchain/setup-time.py measure   # no network, ~2 min for 3 runs/cell
    python3 bench/g3-toolchain/setup-time.py teardown  # removes containers/images/network this script made
    python3 bench/g3-toolchain/setup-time.py all       # the above three, in order

Env:
  G3_WORK   scratch dir, default a mktemp -d
  G3_RUNS   runs per cell, default 3 (task allows cutting 5 to 3)
"""
from __future__ import annotations

import importlib.util
import json
import os
import re
import shutil
import statistics
import subprocess
import sys
import tempfile
import textwrap
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent.parent
WORK = Path(os.environ.get("G3_WORK") or tempfile.mkdtemp(prefix="g3-toolchain-b-"))
RUNS = int(os.environ.get("G3_RUNS", "3"))

_spec = importlib.util.spec_from_file_location("platform_drift", REPO_ROOT / "compat" / "platform-drift.py")
platform_drift = importlib.util.module_from_spec(_spec)
assert _spec.loader is not None
sys.modules["platform_drift"] = platform_drift
_spec.loader.exec_module(platform_drift)

NET = "g3net"
BASE_IMAGE = "g3-toolchain-base"
STATICPHP_CONTAINER = "g3-staticphp"

# (local name, corpus name in platform_drift.CORPUS, static-php-cli tarball)
PROJECTS = [
    ("composer", "composer/composer", "php-8.3.32-cli-linux-x86_64.tar.gz"),
    ("bookstack", "BookStackApp/BookStack", "php-8.3.32-cli-linux-x86_64.tar.gz"),
    ("symfonydemo", "symfony/demo", "php-8.4.23-cli-linux-x86_64.tar.gz"),
]
APT_PKGS = {
    "composer": "php8.3-cli unzip",
    "bookstack": "php8.3-cli php8.3-mbstring php8.3-xml php8.3-curl php8.3-gd php8.3-zip unzip",
    # deliberately the baseline set -- expected to fail the platform check
    "symfonydemo": "php8.3-cli php8.3-mbstring php8.3-xml php8.3-curl php8.3-gd php8.3-zip unzip",
}
PPA_PKGS = "php8.4-cli php8.4-mbstring php8.4-sqlite3 php8.4-xml unzip"
CELLS = [
    ("composer", "apt"),
    ("composer", "static"),
    ("bookstack", "apt"),
    ("bookstack", "static"),
    ("symfonydemo", "ppa"),
    ("symfonydemo", "static"),
]

STATIC_BASE_URL = "https://dl.static-php.dev/static-php-cli/common"
COMPOSER_PHAR_URL = "https://getcomposer.org/download/2.8.9/composer.phar"

# `add-apt-repository -y ppa:ondrej/php`'s own deb822 source file, captured
# once from a container that ran software-properties-common -- reused
# directly so the actual .deb download step (below) never runs in a
# container polluted by that tool's own dependency closure. See the module
# docstring's "already installed" note for why that matters.
ONDREJ_SOURCES = REPO_ROOT / "bench" / "g3-toolchain" / "ondrej-ubuntu-php-noble.sources"


def sh(*args: str, **kw) -> subprocess.CompletedProcess:
    return subprocess.run(args, check=True, **kw)


def docker_run(*args: str, **kw) -> subprocess.CompletedProcess:
    return sh("docker", "run", "--rm", *args, **kw)


# --- warm phase (real network, untimed) -------------------------------

def warm_fetch_static() -> None:
    static_dir = WORK / "static"
    static_dir.mkdir(parents=True, exist_ok=True)
    for _, _, tarball in PROJECTS:
        dest = static_dir / tarball
        if dest.exists():
            continue
        print(f"fetching {tarball}")
        sh("curl", "-sL", "-o", str(dest), f"{STATIC_BASE_URL}/{tarball}")
    phar = WORK / "composer.phar"
    if not phar.exists():
        sh("curl", "-sL", "-o", str(phar), COMPOSER_PHAR_URL)


def warm_debs() -> None:
    debs = WORK / "debs"
    debs84 = WORK / "debs84"
    if not (debs / "Packages.gz").exists():
        debs.mkdir(parents=True, exist_ok=True)
        docker_run(
            "-v", f"{debs}:/debs",
            "ubuntu:24.04", "bash", "-c",
            "apt-get update -qq && "
            "apt-get install -y -qq --download-only "
            "php8.3-cli php8.3-mbstring php8.3-xml php8.3-curl php8.3-gd php8.3-zip unzip && "
            "cp /var/cache/apt/archives/*.deb /debs/",
        )
        docker_run(
            "-v", f"{debs}:/repo", "ubuntu:24.04", "bash", "-c",
            "apt-get update -qq >/dev/null && apt-get install -y -qq dpkg-dev >/dev/null && "
            "cd /repo && dpkg-scanpackages . /dev/null > Packages 2>/dev/null && gzip -kf Packages",
        )
    if not (debs84 / "Packages.gz").exists():
        debs84.mkdir(parents=True, exist_ok=True)
        docker_run(
            "-v", f"{debs84}:/debs84", "-v", f"{ONDREJ_SOURCES}:/ondrej.sources:ro",
            "ubuntu:24.04", "bash", "-c",
            "apt-get update -qq && apt-get install -y -qq ca-certificates && "
            "cp /ondrej.sources /etc/apt/sources.list.d/ondrej-ubuntu-php-noble.sources && "
            "apt-get update -qq && "
            "apt-get install -y -qq --download-only php8.4-cli php8.4-mbstring php8.4-sqlite3 php8.4-xml unzip && "
            "cp /var/cache/apt/archives/*.deb /debs84/",
        )
        docker_run(
            "-v", f"{debs84}:/repo", "ubuntu:24.04", "bash", "-c",
            "apt-get update -qq >/dev/null && apt-get install -y -qq dpkg-dev >/dev/null && "
            "cd /repo && dpkg-scanpackages . /dev/null > Packages 2>/dev/null && gzip -kf Packages",
        )


def warm_base_image() -> None:
    dockerfile = WORK / "Dockerfile.base"
    dockerfile.write_text(
        "FROM ubuntu:24.04\n"
        "RUN apt-get update -qq && apt-get install -y -qq curl ca-certificates "
        "&& rm -rf /var/lib/apt/lists/*\n"
    )
    sh("docker", "build", "-q", "-t", BASE_IMAGE, "-f", str(dockerfile), str(WORK))


def clone_projects() -> None:
    projects_dir = WORK / "projects"
    for local_name, corpus_name, _ in PROJECTS:
        dest_files = projects_dir / local_name
        if (dest_files / "composer.lock").exists():
            continue
        entry = next(c for c in platform_drift.CORPUS if c[0] == corpus_name)
        _, repo, commit, _source = entry
        candidates = [repo] if repo is not None else platform_drift.resolve_repo_candidates(corpus_name)
        scratch_clone = WORK / "clones" / local_name
        ok, label = False, "no candidate repo URL"
        for candidate in candidates:
            ok, label = platform_drift.clone_at(candidate, commit, scratch_clone)
            if ok:
                break
        if not ok:
            raise RuntimeError(f"clone failed for {corpus_name}: {label}")
        dest_files.mkdir(parents=True, exist_ok=True)
        shutil.copy(scratch_clone / "composer.json", dest_files / "composer.json")
        shutil.copy(scratch_clone / "composer.lock", dest_files / "composer.lock")


def warm_composer_cache() -> None:
    cache = WORK / "composer-cache"
    marker = cache / ".warmed"
    if marker.exists():
        return
    cache.mkdir(parents=True, exist_ok=True)
    static_84 = "php-8.4.23-cli-linux-x86_64.tar.gz"  # broadest ext coverage, runs composer.phar for all three
    script = WORK / "warm-cache.sh"
    script.write_text(
        textwrap.dedent(f"""\
        set -eux
        export DEBIAN_FRONTEND=noninteractive
        apt-get update -qq && apt-get install -y -qq git ca-certificates >/dev/null
        mkdir -p /opt/php && tar -xzf /static/{static_84} -C /opt/php
        for p in {' '.join(n for n, _, _ in PROJECTS)}; do
          rm -rf /tmp/$p && mkdir -p /tmp/$p
          cp /projects/$p/composer.json /projects/$p/composer.lock /tmp/$p/
          cd /tmp/$p
          COMPOSER_CACHE_DIR=/cache /opt/php/php /composer.phar install --no-dev --no-scripts --no-plugins --no-interaction --ignore-platform-reqs
        done
        """)
    )
    docker_run(
        "-v", f"{WORK / 'static'}:/static:ro",
        "-v", f"{WORK / 'composer.phar'}:/composer.phar:ro",
        "-v", f"{WORK / 'projects'}:/projects:ro",
        "-v", f"{cache}:/cache",
        "-v", f"{script}:/w.sh:ro",
        "ubuntu:24.04", "bash", "/w.sh",
    )
    marker.write_text("ok\n")


def warm_network_and_server() -> None:
    have_net = subprocess.run(["docker", "network", "inspect", NET], capture_output=True).returncode == 0
    if not have_net:
        sh("docker", "network", "create", "--internal", NET)
    have_container = subprocess.run(["docker", "inspect", STATICPHP_CONTAINER], capture_output=True).returncode == 0
    if not have_container:
        sh(
            "docker", "run", "-d", "--name", STATICPHP_CONTAINER, "--network", NET,
            "-v", f"{WORK / 'static'}:/srv:ro",
            "python:3.12-slim", "bash", "-c", "cd /srv && python3 -m http.server 8000",
        )


def warm() -> None:
    warm_fetch_static()
    warm_debs()
    warm_base_image()
    clone_projects()
    warm_composer_cache()
    warm_network_and_server()
    print(f"warm complete, work dir {WORK}")


# --- measure phase (network excluded) -----------------------------------

def run_cell(project: str, way: str) -> dict:
    static_tarball = next(t for n, _, t in PROJECTS if n == project)
    php_bin = "/opt/php/php" if way == "static" else "php"
    setup_cmd = {
        "apt": (
            "rm -f /etc/apt/sources.list && rm -rf /etc/apt/sources.list.d && mkdir -p /etc/apt/sources.list.d\n"
            "echo 'deb [trusted=yes] file:///debs ./' > /etc/apt/sources.list.d/local.list\n"
            "apt-get update -qq\n"
            f"apt-get install -y {APT_PKGS[project]} 2>&1 | tee /tmp/apt.log\n"
        ),
        "ppa": (
            "rm -f /etc/apt/sources.list && rm -rf /etc/apt/sources.list.d && mkdir -p /etc/apt/sources.list.d\n"
            "echo 'deb [trusted=yes] file:///debs84 ./' > /etc/apt/sources.list.d/local.list\n"
            "apt-get update -qq\n"
            f"apt-get install -y {PPA_PKGS} 2>&1 | tee /tmp/apt.log\n"
        ),
        "static": (
            f"curl -s -w 'BYTES=%{{size_download}}\\n' -o /tmp/php.tar.gz http://{STATICPHP_CONTAINER}:8000/{static_tarball} | tee /tmp/apt.log\n"
            "mkdir -p /opt/php && tar -xzf /tmp/php.tar.gz -C /opt/php\n"
        ),
    }[way]
    script = WORK / "run.sh"
    script.write_text(
        textwrap.dedent(f"""\
        set -eu
        t0=$EPOCHREALTIME
        {setup_cmd}
        t1=$EPOCHREALTIME
        mkdir -p /app
        cp /project-src/composer.json /project-src/composer.lock /app/
        cd /app
        set +e
        COMPOSER_CACHE_DIR=/cache {php_bin} /composer.phar install --no-dev --no-scripts --no-plugins --no-interaction
        rc=$?
        set -e
        t2=$EPOCHREALTIME
        echo "PHP_SETUP_S=$(awk -v a="$t1" -v b="$t0" 'BEGIN{{printf "%.3f", a-b}}')"
        echo "WHOLE_S=$(awk -v a="$t2" -v b="$t0" 'BEGIN{{printf "%.3f", a-b}}')"
        echo "INSTALL_RC=$rc"
        """)
    )
    mounts = [
        "-v", f"{WORK / 'composer.phar'}:/composer.phar:ro",
        "-v", f"{WORK / 'composer-cache'}:/cache",
        "-v", f"{WORK / 'projects' / project}:/project-src:ro",
        "-v", f"{script}:/run.sh:ro",
    ]
    net = "none"
    if way == "apt":
        mounts += ["-v", f"{WORK / 'debs'}:/debs:ro"]
    elif way == "ppa":
        mounts += ["-v", f"{WORK / 'debs84'}:/debs84:ro"]
    elif way == "static":
        net = NET
    proc = subprocess.run(
        ["docker", "run", "--rm", "--network", net, *mounts, BASE_IMAGE, "bash", "/run.sh"],
        capture_output=True, text=True,
    )
    out = proc.stdout + proc.stderr
    row = {"project": project, "way": way}
    for key, pat in (
        ("php_setup_s", r"PHP_SETUP_S=([0-9.]+)"),
        ("whole_s", r"WHOLE_S=([0-9.]+)"),
        ("install_rc", r"INSTALL_RC=(\d+)"),
    ):
        m = re.search(pat, out)
        row[key] = m.group(1) if m else None
    m = re.search(r"BYTES=(\d+)", out)
    if m:
        row["bytes"] = int(m.group(1))
    else:
        row["bytes"] = sum(
            float(n) * {"B": 1, "kB": 1000, "MB": 1_000_000, "GB": 1_000_000_000}[u]
            for n, u in re.findall(r"\[([\d.]+)\s*(B|kB|MB|GB)\]", out)
            if "Get:" in out
        )
    row["raw_tail"] = out[-400:]
    return row


def measure() -> None:
    out_path = WORK / "results.jsonl"
    rows = []
    for project, way in CELLS:
        for i in range(RUNS):
            row = run_cell(project, way)
            row["run"] = i + 1
            rows.append(row)
            print(f"{project} {way} run {i + 1}: whole_s={row['whole_s']} rc={row['install_rc']}")
    # one confirmatory (not repeated) run for the expected-failure cell
    row = run_cell("symfonydemo", "apt")
    row["run"] = "confirmatory"
    rows.append(row)
    print(f"symfonydemo apt (expected fail): rc={row['install_rc']}")

    with out_path.open("w") as f:
        for row in rows:
            f.write(json.dumps(row) + "\n")

    print(f"\nwrote {out_path}\n")
    print("median PHP_SETUP_s / WHOLE_s / bytes per cell:")
    for project, way in CELLS:
        vals = [r for r in rows if r["project"] == project and r["way"] == way and r["run"] != "confirmatory"]
        setup = statistics.median(float(r["php_setup_s"]) for r in vals)
        whole = statistics.median(float(r["whole_s"]) for r in vals)
        by = vals[0]["bytes"]
        print(f"  {project:12s} {way:7s} setup={setup:.3f}s whole={whole:.3f}s bytes={by}")


# --- teardown -------------------------------------------------------------

def teardown() -> None:
    subprocess.run(["docker", "rm", "-f", STATICPHP_CONTAINER], capture_output=True)
    subprocess.run(["docker", "network", "rm", NET], capture_output=True)
    subprocess.run(["docker", "rmi", BASE_IMAGE], capture_output=True)
    print(f"removed {STATICPHP_CONTAINER}, {NET}, {BASE_IMAGE} (ubuntu:24.04/python:3.12-slim base images kept)")


def main() -> int:
    cmd = sys.argv[1] if len(sys.argv) > 1 else "all"
    if cmd in ("warm", "all"):
        warm()
    if cmd in ("measure", "all"):
        measure()
    if cmd in ("teardown", "all"):
        teardown()
    if cmd not in ("warm", "measure", "teardown", "all"):
        print(__doc__)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
