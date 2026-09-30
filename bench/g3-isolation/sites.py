#!/usr/bin/env python3
r"""Candidate 3.2 (#330) follow-up (#346): how often a Composer-managed
WordPress site's own dependency tree collides at runtime with a plugin's
bundled copy of the same library, and how often that was already hand-fixed
in the site's `composer.json` history (a pin, a `conflict` entry, a
`replace`).

Read-only: no installs, no `composer`/`viv` run against the sites, nothing
is written back into them. Every measurement comes from files already on
disk (`composer.lock`, `vendor/composer/installed.json`, the plugin/
mu-plugin directories) and `git log`/`git show` against the site's own
history.

What is measured, per site:
  1. Site-installed libraries: composer.lock's packages + packages-dev,
     name -> version. Whether vendor/composer/installed.json also exists
     and, if so, whether its versions agree.
  2. Active plugins with a bundled dependency tree: every top-level
     directory under the site's plugins/mu-plugins folders that contains a
     vendor/, vendor_prefixed/, vendor-prefixed/, lib/packages/ or
     dependencies/ directory anywhere inside it. Libraries come from
     vendor/composer/installed.json when the bundle has one (both the
     Composer 1 list shape and the Composer 2 {"packages": [...]} shape,
     via inventory.py's own parser); otherwise a directory-name fallback,
     version "unknown" (also inventory.py's, generalised to the
     vendor_prefixed/vendor-prefixed/dependencies families). Prefixing
     read the same way inventory.py reads a plugin zip (imported, not
     reimplemented). The install path -- wpackagist, composer (another
     vendor's name, installed by a Composer installer path into that
     directory), committed (tracked by git, not in the lock) or untracked
     (wp-admin or build output) -- comes from
     vendor/composer/installed.json's own "install-path" per package,
     cross-checked against composer.lock for the wpackagist/composer
     split.
  3. Runtime clashes: a library in the site's own lock, also bundled by a
     plugin at a different version, with that bundled copy not read as
     prefixed (mirrors inventory.py's own "conflict candidate" reading:
     prefixed != "yes").
  4. Solver-level clashes: walking `git log --follow -- composer.json`
     oldest to newest, a commit whose `require`/`require-dev` pins a
     package to an exact or narrow version where the prior revision
     didn't, or that adds a `conflict` or `replace` entry. WordPress
     assets (anything composer.lock currently types wordpress-plugin/
     -muplugin/-theme/-core, plus any wpackagist- name) are excluded from
     the pin scan: pinning a plugin's own version is routine site
     hygiene, not a library-version clash.

What is assumed:
  - A plugin's own vendor/composer/installed.json "install-path" is
    trustworthy for locating it (it's what `composer install` itself
    wrote); a site whose vendor/ doesn't match its lock (see 1) would
    make this unreliable, hence recording the agreement check.
  - "Different versions" compares raw version strings, as inventory.py
    does; no semver normalisation.
  - A pin is an exact version (`6.5.8`) or a comma-joined range with no
    `^`/`~` (`>=6.5,<6.6`); first time a package enters that state counts,
    not every later bump while it stays pinned.

Anonymisation: sites are labelled A, B, C... in the order given on the
command line; nothing that could identify one (a path, a repo name, a
domain, a commit message) reaches the report. A plugin directory whose
name starts with one of --private-prefix is reported as "private plugin
N" (numbered per site) instead of its real slug -- use it for a plugin
named after the client rather than the software it is.

Usage:
    python3 bench/g3-isolation/sites.py SITE [SITE ...] [--out PATH]
        [--private-prefix PREFIX ...]
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import inventory  # noqa: E402  (reused: parse_installed_json, vendor_dir_fallback, detect_prefixed)

MARKER_DIR_NAMES = {"vendor", "vendor_prefixed", "vendor-prefixed", "dependencies"}
LAYOUTS = (("web/app/plugins", "web/app/mu-plugins"), ("content/plugins", "content/mu-plugins"))


class FsReader:
    """Duck-types zipfile.ZipFile's .read() for inventory.py's prefixing
    detector, so the same code reads a plugin directory on disk as it
    reads a WordPress.org zip member."""

    def __init__(self, root: Path) -> None:
        self.root = root

    def read(self, name: str) -> bytes:
        return (self.root / name).read_bytes()


def discover_layout(site: Path) -> tuple[Path | None, Path | None]:
    for plugins_rel, mu_rel in LAYOUTS:
        if (site / plugins_rel).is_dir():
            return site / plugins_rel, site / mu_rel
    return None, None


def load_lock(site: Path) -> dict[str, str]:
    data = json.loads((site / "composer.lock").read_bytes())
    libs: dict[str, str] = {}
    for pkg in data.get("packages", []) + data.get("packages-dev", []):
        libs[pkg["name"]] = pkg.get("version", "unknown")
    return libs


def lock_vs_installed(site: Path, lock: dict[str, str]) -> tuple[bool, bool, int]:
    path = site / "vendor" / "composer" / "installed.json"
    if not path.is_file():
        return False, False, 0
    try:
        installed = dict(inventory.parse_installed_json(path.read_bytes()))
    except json.JSONDecodeError:
        return True, False, 0
    mismatches = sum(1 for name, version in lock.items() if name in installed and installed[name] != version)
    return True, mismatches == 0, mismatches


def load_install_map(site: Path) -> dict[Path, str]:
    """Maps every wordpress-plugin/wordpress-muplugin package's resolved
    directory (from its own install-path) to its composer.lock name."""
    path = site / "vendor" / "composer" / "installed.json"
    if not path.is_file():
        return {}
    try:
        data = json.loads(path.read_bytes())
    except json.JSONDecodeError:
        return {}
    packages = data.get("packages", []) if isinstance(data, dict) else data
    out: dict[Path, str] = {}
    for pkg in packages:
        if pkg.get("type") not in ("wordpress-plugin", "wordpress-muplugin"):
            continue
        install_path = pkg.get("install-path")
        if not install_path:
            continue
        out[(path.parent / install_path).resolve()] = pkg["name"]
    return out


def git_ls_files_nonempty(site: Path, rel_dir: Path) -> bool:
    r = subprocess.run(
        ["git", "-C", str(site), "ls-files", str(rel_dir)],
        capture_output=True, text=True,
    )
    return bool(r.stdout.strip())


def resolve_plugins(site: Path, plugins_dir: Path | None, mu_plugins_dir: Path | None) -> list[dict]:
    """One entry per active plugin/mu-plugin directory: {dir, name,
    category}. A directory that is only an installer-paths container
    (e.g. content/mu-plugins/vendor/, holding several plugins one level
    deeper) is replaced by the plugins nested inside it."""
    install_map = load_install_map(site)
    results = []
    for base_dir in (plugins_dir, mu_plugins_dir):
        if base_dir is None or not base_dir.is_dir():
            continue
        installed_here = {d: n for d, n in install_map.items() if d.is_relative_to(base_dir)}
        children = sorted((c for c in base_dir.iterdir() if c.is_dir()), key=lambda p: p.name)
        final: dict[Path, str | None] = {}
        for c in children:
            if c in installed_here:
                final[c] = installed_here[c]
                continue
            nested = [d for d in installed_here if d != c and d.is_relative_to(c)]
            if nested:
                for d in nested:
                    final[d] = installed_here[d]
            else:
                final[c] = None
        for d, name in sorted(final.items(), key=lambda kv: str(kv[0])):
            if name is not None:
                category = "wpackagist" if name.startswith(("wpackagist-plugin/", "wpackagist-muplugin/")) else "composer"
            else:
                tracked = git_ls_files_nonempty(site, d.relative_to(site))
                category = "committed" if tracked else "untracked"
            results.append({"dir": d, "name": name, "category": category})
    return results


def find_markers(plugin_dir: Path) -> list[Path]:
    import os

    markers = []
    for dirpath, dirnames, _ in os.walk(plugin_dir):
        base = Path(dirpath).name
        for name in dirnames:
            if name in MARKER_DIR_NAMES or (name == "packages" and base == "lib"):
                markers.append(Path(dirpath) / name)
    return markers


def all_relpaths(plugin_dir: Path) -> list[str]:
    import os

    out = []
    for dirpath, _, filenames in os.walk(plugin_dir):
        rel_dir = Path(dirpath).relative_to(plugin_dir).as_posix()
        for f in filenames:
            out.append(f"{rel_dir}/{f}" if rel_dir != "." else f)
    return out


def dir_fallback(names: list[str], label: str) -> list[tuple[str, str]]:
    """Generalises inventory.vendor_dir_fallback to the vendor_prefixed/
    vendor-prefixed/dependencies families: a prefixed bundle Strauss or
    Mozart relocated out of vendor/ usually carries no installed.json, so
    the only signal left is the directory names."""
    pat = re.compile(rf"(^|/){re.escape(label)}/([^/]+)/([^/]+)/")
    seen: set[str] = set()
    out = []
    for n in names:
        m = pat.search(n)
        if not m or m.group(2) in ("composer", "bin"):
            continue
        key = f"{m.group(2)}/{m.group(3)}"
        if key not in seen:
            seen.add(key)
            out.append((key, "unknown"))
    return out


def bundled_libraries(plugin_dir: Path, names: list[str]) -> list[dict]:
    installed_json_members = [n for n in names if n.endswith("vendor/composer/installed.json")]
    libs: dict[tuple[str, str], None] = {}
    for member in installed_json_members:
        try:
            for name, version in inventory.parse_installed_json((plugin_dir / member).read_bytes()):
                libs[(name, version)] = None
        except json.JSONDecodeError:
            continue
    if not libs:
        for name, version in inventory.vendor_dir_fallback(names):
            libs[(name, version)] = None
        for label in ("vendor_prefixed", "vendor-prefixed", "dependencies"):
            for name, version in dir_fallback(names, label):
                libs[(name, version)] = None

    reader = FsReader(plugin_dir)
    return [
        {"name": name, "version": version, "prefixed": inventory.detect_prefixed(reader, names, name)}
        for name, version in libs
    ]


def composer_json_revisions(site: Path) -> list[tuple[str, str]]:
    r = subprocess.run(
        # --no-show-signature: some environments set log.showsignature=true
        # globally, which would otherwise interleave gpg verification text
        # into this machine-parsed stdout.
        ["git", "-C", str(site), "log", "--no-show-signature", "--follow", "--reverse",
         "--format=%H\x1f%ad", "--date=format:%Y", "--", "composer.json"],
        capture_output=True, text=True,
    )
    out = []
    for line in r.stdout.splitlines():
        if not line:
            continue
        rev, year = line.split("\x1f")
        out.append((rev, year))
    return out


def git_show(site: Path, rev: str, path: str) -> str | None:
    r = subprocess.run(["git", "-C", str(site), "show", f"{rev}:{path}"], capture_output=True, text=True)
    return r.stdout if r.returncode == 0 else None


PIN_EXACT_RE = re.compile(r"v?\d+(\.\d+){0,3}")


def is_pin(constraint: str) -> bool:
    if not isinstance(constraint, str) or constraint in ("", "*") or constraint.startswith("dev-"):
        return False
    if "^" in constraint or "~" in constraint:
        return False
    if PIN_EXACT_RE.fullmatch(constraint):
        return True
    return "," in constraint


def wp_asset_names(site: Path) -> set[str]:
    """Package names composer.lock currently types as a WordPress asset
    (plugin, mu-plugin, theme, core) -- pinning one of these to an exact
    version is routine WordPress-site hygiene, not a library-version
    clash, so it is excluded from the pin scan below. Also matched by the
    wpackagist- vendor prefix alone, to catch a plugin since removed from
    the lock (and so absent from its type here)."""
    data = json.loads((site / "composer.lock").read_bytes())
    return {
        pkg["name"] for pkg in data.get("packages", []) + data.get("packages-dev", [])
        if pkg.get("type", "").startswith("wordpress-")
    }


def solver_clashes(site: Path, wp_assets: set[str]) -> list[dict]:
    clashes = []
    prev: dict | None = None
    for rev, year in composer_json_revisions(site):
        raw = git_show(site, rev, "composer.json")
        if raw is None:
            continue
        try:
            data = json.loads(raw)
        except json.JSONDecodeError:
            continue
        if prev is not None:
            for key, value in data.get("conflict", {}).items():
                if prev.get("conflict", {}).get(key) != value:
                    clashes.append({"year": year, "text": f'conflict `"{key}": "{value}"`'})
            for key, value in data.get("replace", {}).items():
                if prev.get("replace", {}).get(key) != value:
                    clashes.append({"year": year, "text": f'replace `"{key}": "{value}"`'})
            require = {**data.get("require", {}), **data.get("require-dev", {})}
            prev_require = {**prev.get("require", {}), **prev.get("require-dev", {})}
            for key, value in require.items():
                if key in wp_assets or key.startswith("wpackagist-"):
                    continue
                if is_pin(value) and value != prev_require.get(key) and not is_pin(prev_require.get(key, "")):
                    clashes.append({"year": year, "text": f'pin `"{key}": "{value}"`'})
        prev = data
    return clashes


def analyze_site(site: Path, private_prefixes: list[str]) -> dict:
    plugins_dir, mu_plugins_dir = discover_layout(site)
    lock = load_lock(site)
    installed_exists, installed_agrees, mismatches = lock_vs_installed(site, lock)

    plugins = resolve_plugins(site, plugins_dir, mu_plugins_dir)
    bundling = []
    for p in plugins:
        markers = find_markers(p["dir"])
        if not markers:
            continue
        names = all_relpaths(p["dir"])
        libs = bundled_libraries(p["dir"], names)
        bundling.append({**p, "libraries": libs})

    private_count = 0
    runtime_clashes = []
    for p in bundling:
        slug = p["dir"].name
        if any(slug.startswith(prefix) for prefix in private_prefixes):
            private_count += 1
            display = f"private plugin {private_count}"
        else:
            display = slug
        p["display"] = display
        for lib in p["libraries"]:
            site_version = lock.get(lib["name"])
            if site_version is None or site_version == lib["version"] or lib["prefixed"] == "yes":
                continue
            runtime_clashes.append({
                "plugin": display,
                "library": lib["name"],
                "site_version": site_version,
                "bundled_version": lib["version"],
                "prefixed": lib["prefixed"],
                "install_path": p["category"],
            })

    return {
        "lock_count": len(lock),
        "installed_exists": installed_exists,
        "installed_agrees": installed_agrees,
        "installed_mismatches": mismatches,
        "plugins_scanned": len(plugins),
        "bundling": bundling,
        "runtime_clashes": runtime_clashes,
        "solver_clashes": solver_clashes(site, wp_asset_names(site)),
    }


def render_markdown(reports: list[dict]) -> str:
    labels = [chr(ord("A") + i) for i in range(len(reports))]
    lines = []
    lines.append("# Candidate 3.2 follow-up: site-installed libraries an active plugin also bundles (#346)")
    lines.append("")
    lines.append(
        "How to reproduce: `make bench-g3-isolation-sites SITES=\"path path ...\"` "
        "(read-only against each site; no composer/viv run against them). "
        f"Invocation recorded here only as `sites.py <{len(reports)} paths, held outside the repository>`."
    )
    lines.append("")

    for label, r in zip(labels, reports):
        by_category: dict[str, int] = {}
        for p in r["bundling"]:
            by_category[p["category"]] = by_category.get(p["category"], 0) + 1
        lines.append(f"## Site {label}")
        lines.append("")
        lines.append("| Metric | Count |")
        lines.append("|---|---|")
        lines.append(f"| Site-installed libraries (composer.lock) | {r['lock_count']} |")
        lines.append(f"| vendor/composer/installed.json present | {'yes' if r['installed_exists'] else 'no'} |")
        if r["installed_exists"]:
            agree = "yes" if r["installed_agrees"] else f"no ({r['installed_mismatches']} version mismatches)"
            lines.append(f"| ...agrees with the lock | {agree} |")
        lines.append(f"| Plugin/mu-plugin directories scanned | {r['plugins_scanned']} |")
        lines.append(f"| ...with a bundled dependency tree | {len(r['bundling'])} |")
        for cat in ("wpackagist", "composer", "committed", "untracked"):
            if by_category.get(cat):
                lines.append(f"| &nbsp;&nbsp;installed via {cat} | {by_category[cat]} |")
        lines.append(f"| Runtime clashes (site lock vs. bundled, unprefixed) | {len(r['runtime_clashes'])} |")
        lines.append(f"| Solver-level clashes (composer.json history) | {len(r['solver_clashes'])} |")
        lines.append("")

    lines.append("## Runtime clashes")
    lines.append("")
    lines.append("| Site | Plugin | Library | Site version | Bundled version | Prefixed | Install path |")
    lines.append("|---|---|---|---|---|---|---|")
    any_runtime = False
    for label, r in zip(labels, reports):
        for c in r["runtime_clashes"]:
            any_runtime = True
            lines.append(
                f"| {label} | {c['plugin']} | {c['library']} | {c['site_version']} | "
                f"{c['bundled_version']} | {c['prefixed']} | {c['install_path']} |"
            )
    if not any_runtime:
        lines.append("| -- | -- | -- | -- | -- | -- | -- |")
    lines.append("")

    lines.append("## Solver-level clashes")
    lines.append("")
    lines.append("| Site | Constraint | Year |")
    lines.append("|---|---|---|")
    any_solver = False
    for label, r in zip(labels, reports):
        for c in r["solver_clashes"]:
            any_solver = True
            lines.append(f"| {label} | {c['text']} | {c['year']} |")
    if not any_solver:
        lines.append("| -- | -- | -- |")
    lines.append("")

    total_bundling = sum(len(r["bundling"]) for r in reports)
    total_scanned = sum(r["plugins_scanned"] for r in reports)
    total_runtime = sum(len(r["runtime_clashes"]) for r in reports)
    total_solver = sum(len(r["solver_clashes"]) for r in reports)
    sites_with_installed = sum(1 for r in reports if r["installed_exists"])
    sites_agreeing = sum(1 for r in reports if r["installed_exists"] and r["installed_agrees"])
    all_runtime = [c for r in reports for c in r["runtime_clashes"]]
    unknown_version = sum(1 for c in all_runtime if c["bundled_version"] == "unknown")
    coexisting = sum(
        1 for c in all_runtime
        if c["library"].startswith(("symfony/polyfill-", "psr/")) or c["library"] == "composer/installers"
    )
    lines.append("## Reading")
    lines.append("")
    lines.append(
        f"Across the {len(reports)} sites, {total_scanned} plugin/mu-plugin directories were scanned and "
        f"{total_bundling} of them carry a bundled dependency tree (a vendor/, vendor_prefixed/, "
        f"vendor-prefixed/, lib/packages/ or dependencies/ directory somewhere inside). "
        f"{sites_with_installed} of {len(reports)} sites also carry a "
        f"vendor/composer/installed.json, of which {sites_agreeing} agree with composer.lock on every "
        f"shared library's version. {total_runtime} of the bundled copies are a library the site's own "
        f"lock also installs, at a different version, read as unprefixed -- the runtime collision "
        f"candidates the WordPress.org sample in bench/results/g3-isolation.md could not show a rate "
        f"for. Of those {total_runtime}, {unknown_version} have a bundled version this script could not "
        f"read (a directory-name fallback, not an installed.json) and {coexisting} are psr/*, "
        f"symfony/polyfill-* or composer/installers -- the same built-to-coexist or install-time-only "
        f"families bench/results/g3-isolation.md classified out of its own conflict-candidate count. "
        f"Walking each site's composer.json history the same way (WordPress asset pins excluded) turned "
        f"up {total_solver} commits that pinned a library to an exact or narrow version, added a "
        f"conflict entry or a replace where the prior revision had neither."
    )
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("sites", nargs="+", type=Path, help="site checkouts, labelled A, B, C... in this order")
    ap.add_argument("--out", type=Path, help="write the report here instead of stdout")
    ap.add_argument(
        "--private-prefix", action="append", default=[], metavar="PREFIX",
        help="plugin directory name prefix to redact as 'private plugin N' (repeatable)",
    )
    args = ap.parse_args()

    reports = [analyze_site(site.resolve(), args.private_prefix) for site in args.sites]
    report = render_markdown(reports)
    if args.out:
        args.out.write_text(report)
    else:
        print(report)
    return 0


if __name__ == "__main__":
    sys.exit(main())
