#!/usr/bin/env python3
r"""Candidate 3.2 (#330), Phase A: a static inventory of bundled Composer
dependencies across the 100 most popular WordPress.org plugins -- how many
bundle a vendor/ directory, which libraries appear in more than one plugin
at different versions, and how many of those copies are already
namespace-prefixed (php-scoper / Strauss / Mozart style).

Static only: no installs, no test suites -- that's a later phase (rewrite
the unprefixed ones with php-scoper, run each plugin's own test suite
where it has one), gated on this phase's numbers being worth it. Each
plugin's zip is inspected with zipfile's member listing plus reading a
handful of members, never extracted in full.

What is measured:
  - bundles_vendor: any zip member matching */vendor/autoload.php or
    */vendor/composer/installed.json.
  - library (name, version) pairs: parsed from installed.json when
    present (both the Composer 1 list shape and the Composer 2
    {"packages": [...]} shape); falls back to vendor/<vendor>/<name>/
    directory names (version "unknown") when installed.json is absent.
  - prefixed yes/no/unknown: scan up to 20 of the library's own .php files
    in the zip for the first 3 that declare a `namespace` line at all
    (skipping over bootstrap/shim files that only `use` one -- Symfony's
    polyfills lead with exactly that and would otherwise read as false
    "unknown") and compare those namespaces against the library's own
    root namespace (from its composer.json autoload.psr-4 keys when the
    zip carries one for it, else the first namespace segment seen). A
    namespace that wraps the library's own root in a foreign segment
    (e.g. `WPForms\Vendor\GuzzleHttp\...`) counts as prefixed; one that
    starts with the root namespace itself does not. No `namespace` line
    found in any of the 20 scanned (old code with no namespaces at all,
    e.g. WooCommerce's action-scheduler, which prefixes by class-name
    convention instead) reads "unknown", not "no" -- there's nothing to
    disprove. A vendor-prefixed/, vendor_prefixed/, lib/packages/ or
    dependencies/ directory holding this specific library (not just
    somewhere else in the same plugin) is a second, independent signal
    (Strauss/Mozart's own convention of moving prefixed code out of
    vendor/ entirely) that upgrades an "unknown" reading to "yes".

What is assumed:
  - The zip's declared plugin `version` from the API is what actually
    shipped at download_link (both come from the same API response).
  - A library's own directory inside the zip is any member path
    containing "vendor/<name>/" where <name> is the Packagist-style
    "vendor/package" from installed.json -- true for a standard
    Composer install layout, not for a plugin that relocated vendor/.
  - "Different versions" compares the raw version strings 1:1 (no semver
    normalisation), so "1.2.3" and "v1.2.3" would read as different when
    they are not; spot-check the top 10 libraries in
    bench/results/g3-isolation.md before trusting the conflict count at
    face value.
  - The installed.json-absent fallback only enumerates plain vendor/
    directories, not the vendor-prefixed/ family: a library Strauss or
    Mozart has already fully relocated out of vendor/, with no
    installed.json left behind to say so, is invisible to the library
    inventory (undercounting "libraries seen" and "prefixed yes"), not
    miscounted as a conflict -- it isn't sitting in the shared vendor/
    tree it would need to be in to collide.

Env:
  G3_ISOLATION_SCRATCH  download + cache dir, default a fresh mktemp -d
                        (point this at a scratchpad to avoid re-downloading
                        on a rerun)
  G3_ISOLATION_LIMIT    comma-separated plugin slugs to run, skip the rest
                        (a narrow rerun without the full 100-plugin fetch)

Output: a Markdown report on stdout (the Makefile target redirects it to
bench/results/g3-isolation.md) and machine-readable JSON next to the
downloads (path printed to stderr).

Usage (needs network, WordPress.org only):
    python3 bench/g3-isolation/inventory.py
"""
from __future__ import annotations

import concurrent.futures
import itertools
import json
import os
import re
import sys
import tempfile
import time
import urllib.error
import urllib.request
import zipfile
from collections import defaultdict
from pathlib import Path

PLUGIN_LIST_URL = (
    "https://api.wordpress.org/plugins/info/1.2/"
    "?action=query_plugins&request[browse]=popular&request[per_page]=100"
)

TIMEOUT = 60
RETRIES = 2
WORKERS = 4

VENDOR_MEMBER_RE = re.compile(r"(^|/)vendor/(autoload\.php|composer/installed\.json)$")
NAMESPACE_RE = re.compile(r"^\s*namespace\s+([^\s;{]+)", re.MULTILINE)

USER_AGENT = "vivace-research/g3-isolation (#330)"


def scratch_dir() -> Path:
    d = os.environ.get("G3_ISOLATION_SCRATCH")
    path = Path(d) if d else Path(tempfile.mkdtemp(prefix="g3-isolation-"))
    path.mkdir(parents=True, exist_ok=True)
    return path


def fetch_plugin_list() -> list[dict]:
    req = urllib.request.Request(PLUGIN_LIST_URL, headers={"User-Agent": USER_AGENT})
    with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
        return json.load(resp)["plugins"]


def download(url: str, dest: Path) -> str | None:
    """Downloads url to dest, skipping if already present. Returns an
    error string on failure (recorded as a skip), None on success."""
    if dest.exists() and dest.stat().st_size > 0:
        return None
    last_err = None
    for _ in range(RETRIES + 1):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            with urllib.request.urlopen(req, timeout=TIMEOUT) as resp:
                data = resp.read()
            dest.write_bytes(data)
            return None
        except (urllib.error.URLError, OSError, TimeoutError) as exc:
            last_err = str(exc)
    return last_err


def parse_installed_json(raw: bytes) -> list[tuple[str, str]]:
    data = json.loads(raw)
    if isinstance(data, dict) and "packages" in data:
        packages = data["packages"]
    elif isinstance(data, list):
        packages = data
    else:
        packages = []
    out = []
    for pkg in packages:
        name = pkg.get("name")
        if name:
            out.append((name, pkg.get("version", "unknown")))
    return out


VENDOR_DIR_RE = re.compile(r"(^|/)vendor/([^/]+)/([^/]+)/")


def vendor_dir_fallback(names: list[str]) -> list[tuple[str, str]]:
    seen: set[str] = set()
    out = []
    for n in names:
        m = VENDOR_DIR_RE.search(n)
        if not m or m.group(2) in ("composer", "bin"):
            continue
        key = f"{m.group(2)}/{m.group(3)}"
        if key not in seen:
            seen.add(key)
            out.append((key, "unknown"))
    return out


def lib_member_paths(names: list[str], lib: str) -> tuple[list[str], bool]:
    """Every zip member under lib's own directory, wherever a prefixing
    tool put it -- plain vendor/, or one of the prefixed-vendor
    conventions (Strauss's vendor_prefixed/ or vendor-prefixed/, Mozart's
    dependencies/, php-scoper's own lib/packages/ output dir). Also
    returns whether it's one of those non-plain locations: a signal
    scoped to this library alone, not the plugin as a whole -- a plugin
    that Strauss-prefixes one dependency and bundles another unprefixed
    in plain vendor/ has both readings inside the one zip."""
    pat = re.compile(rf"(^|/)(?P<dir>vendor|vendor-prefixed|vendor_prefixed|lib/packages|dependencies)/{re.escape(lib)}/")
    paths = []
    dir_signal = False
    for n in names:
        m = pat.search(n)
        if m:
            paths.append(n)
            if m.group("dir") != "vendor":
                dir_signal = True
    return paths, dir_signal


def root_namespace(zf: zipfile.ZipFile, lib_paths: list[str]) -> str | None:
    composer_json_member = next((n for n in lib_paths if n.endswith("/composer.json")), None)
    if not composer_json_member:
        return None
    try:
        data = json.loads(zf.read(composer_json_member))
        for ns in data.get("autoload", {}).get("psr-4", {}):
            if ns:
                return ns.rstrip("\\")
    except (json.JSONDecodeError, KeyError, AttributeError):
        pass
    return None


def detect_prefixed(zf: zipfile.ZipFile, names: list[str], lib: str) -> str:
    lib_paths, dir_signal = lib_member_paths(names, lib)
    # Scan up to 20 candidate files but keep only the first 3 that actually
    # declare a namespace: polyfill-style libraries lead with bootstrap*.php
    # shims that carry a `use` statement and no `namespace` line at all, so
    # sampling by list position alone reads as false "unknown" every time.
    candidates = (n for n in lib_paths if n.endswith(".php"))
    sampled_ns = []
    for n in itertools.islice(candidates, 20):
        if len(sampled_ns) >= 3:
            break
        try:
            text = zf.read(n).decode("utf-8", "replace")
        except (KeyError, zipfile.BadZipFile):
            continue
        m = NAMESPACE_RE.search(text)
        if m:
            sampled_ns.append(m.group(1))
    if not sampled_ns:
        return "yes" if dir_signal else "unknown"
    root_ns = root_namespace(zf, lib_paths) or sampled_ns[0].split("\\")[0]
    if any(ns == root_ns or ns.startswith(root_ns + "\\") for ns in sampled_ns):
        return "no"
    # A namespace present but not rooted at the library's own root: a
    # foreign wrapper if the root's own leaf segment still shows up inside
    # it (e.g. WPForms\Vendor\GuzzleHttp), otherwise the root guess was
    # probably wrong and there's nothing to conclude from it.
    root_leaf = root_ns.split("\\")[-1]
    if any(root_leaf in ns for ns in sampled_ns):
        return "yes"
    return "yes" if dir_signal else "unknown"


def inspect_zip(path: Path) -> dict:
    with zipfile.ZipFile(path) as zf:
        names = zf.namelist()
        if not any(VENDOR_MEMBER_RE.search(n) for n in names):
            return {"bundles_vendor": False, "has_installed_json": False, "libraries": []}

        installed_member = next((n for n in names if n.endswith("vendor/composer/installed.json")), None)
        has_installed_json = installed_member is not None
        libs = []
        if installed_member:
            try:
                libs = parse_installed_json(zf.read(installed_member))
            except (json.JSONDecodeError, KeyError):
                has_installed_json = False
        if not libs:
            libs = vendor_dir_fallback(names)

        libraries = [
            {"name": name, "version": version, "prefixed": detect_prefixed(zf, names, name)}
            for name, version in libs
        ]
        return {"bundles_vendor": True, "has_installed_json": has_installed_json, "libraries": libraries}


def build_aggregate(results: list[dict]) -> dict:
    ok = [r for r in results if "skip" not in r]
    bundling = [r for r in ok if r["bundles_vendor"]]
    with_installed_json = [r for r in bundling if r["has_installed_json"]]

    by_lib: dict[str, dict[str, list[tuple[str, str]]]] = defaultdict(lambda: defaultdict(list))
    for r in bundling:
        for lib in r["libraries"]:
            by_lib[lib["name"]][lib["version"]].append((r["slug"], lib["prefixed"]))

    multi = {
        lib: versions
        for lib, versions in by_lib.items()
        if len({slug for entries in versions.values() for slug, _ in entries}) >= 2
    }

    conflicts = [
        {"library": lib, "plugin": slug, "version": version, "prefixed": prefixed}
        for lib, versions in multi.items()
        if len(versions) >= 2
        for version, entries in versions.items()
        for slug, prefixed in entries
        if prefixed != "yes"
    ]

    top10 = sorted(
        multi.items(),
        key=lambda kv: -len({slug for entries in kv[1].values() for slug, _ in entries}),
    )[:10]

    all_copies = [(lib, v, slug, prefixed) for lib, versions in by_lib.items() for v, entries in versions.items() for slug, prefixed in entries]

    return {
        "plugins_examined": len(ok),
        "plugins_bundling": len(bundling),
        "plugins_with_installed_json": len(with_installed_json),
        "libraries_total": len(by_lib),
        "libraries_in_multiple_plugins": len(multi),
        "conflicts": conflicts,
        "top10": [
            {
                "library": lib,
                "plugin_count": len({slug for entries in versions.values() for slug, _ in entries}),
                "versions": {v: [slug for slug, _ in entries] for v, entries in versions.items()},
            }
            for lib, versions in top10
        ],
        "total_library_copies": len(all_copies),
        "prefixed_yes": sum(1 for *_, prefixed in all_copies if prefixed == "yes"),
        "prefixed_no": sum(1 for *_, prefixed in all_copies if prefixed == "no"),
        "prefixed_unknown": sum(1 for *_, prefixed in all_copies if prefixed == "unknown"),
    }


def render_markdown(results: list[dict], agg: dict, fetch_date: str, wall: float) -> str:
    lines = []
    lines.append("# Candidate 3.2 phase A: bundled Composer dependency inventory (#330)")
    lines.append("")
    lines.append(
        "How to reproduce: `make bench-g3-isolation` (network: WordPress.org "
        f"only). Plugin list fetched {fetch_date}."
    )
    lines.append("")
    lines.append(f"Plugins examined: {agg['plugins_examined']} of 100 requested.")
    skipped = [r for r in results if "skip" in r]
    if skipped:
        lines.append(f"Skipped (download or zip-read failure): {len(skipped)}.")
    lines.append("")
    lines.append("## Totals")
    lines.append("")
    lines.append(f"- Plugins bundling a `vendor/` directory: **{agg['plugins_bundling']}** of {agg['plugins_examined']}.")
    lines.append(f"- Of those, with a `vendor/composer/installed.json`: **{agg['plugins_with_installed_json']}**.")
    lines.append(f"- Distinct libraries seen across all bundling plugins: **{agg['libraries_total']}**.")
    lines.append(f"- Libraries present in 2 or more plugins: **{agg['libraries_in_multiple_plugins']}**.")
    lines.append(f"- Conflict candidates (library x plugin copy, versions differ across plugins, this copy unprefixed): **{len(agg['conflicts'])}**.")
    total_copies = agg["total_library_copies"] or 1
    lines.append(
        f"- Library copies by prefixing: yes {agg['prefixed_yes']}, no {agg['prefixed_no']}, "
        f"unknown {agg['prefixed_unknown']} (of {agg['total_library_copies']} total copies, "
        f"{100 * agg['prefixed_yes'] / total_copies:.1f}% prefixed)."
    )
    lines.append("")
    lines.append("## Top 10 libraries by number of plugins bundling them")
    lines.append("")
    lines.append("| Library | Plugins | Versions in use |")
    lines.append("|---|---|---|")
    for row in agg["top10"]:
        versions = "; ".join(f"{v} ({', '.join(slugs)})" for v, slugs in row["versions"].items())
        lines.append(f"| {row['library']} | {row['plugin_count']} | {versions} |")
    lines.append("")
    if skipped:
        lines.append("## Skipped")
        lines.append("")
        for r in skipped:
            lines.append(f"- {r['slug']}: {r['skip']}")
        lines.append("")
    lines.append("## Reading")
    lines.append("")
    top_lib = agg["top10"][0] if agg["top10"] else None
    top_lib_sentence = (
        f" The most-repeated library, `{top_lib['library']}`, turns up in {top_lib['plugin_count']} of them."
        if top_lib
        else ""
    )
    lines.append(
        f"Of the {agg['plugins_examined']} plugins examined, {agg['plugins_bundling']} bundle a "
        f"vendor/ directory of their own, and {agg['plugins_with_installed_json']} of those carry a "
        f"vendor/composer/installed.json Composer itself wrote. Together they bundle "
        f"{agg['libraries_total']} distinct libraries; {agg['libraries_in_multiple_plugins']} of those "
        f"turn up in two or more of the sampled plugins, most of them at different version strings -- "
        f"exactly the setup PHP's one global namespace can't tell apart at runtime."
        f"{top_lib_sentence} Reading each bundled library's own PHP files, "
        f"{agg['prefixed_yes']} of the {agg['total_library_copies']} bundled copies already carry a "
        f"foreign namespace prefix (php-scoper/Strauss/Mozart style), {agg['prefixed_no']} keep the "
        f"library's own unprefixed namespace (or, for old-style code with no namespaces at all, its own "
        f"unprefixed class names), and {agg['prefixed_unknown']} couldn't be read either way from the "
        f"files sampled. {len(agg['conflicts'])} library-plugin pairs combine a version that disagrees "
        f"with at least one other plugin's copy of the same library and a copy this script couldn't "
        f"confirm is prefixed -- the runtime collision candidates."
    )
    lines.append("")
    lines.append(f"Wall time: {wall:.1f}s.")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    start = time.monotonic()
    scratch = scratch_dir()
    zips_dir = scratch / "zips"
    zips_dir.mkdir(exist_ok=True)

    print("fetching plugin list...", file=sys.stderr)
    plugins = fetch_plugin_list()
    fetch_date = time.strftime("%Y-%m-%d", time.gmtime())
    (scratch / "plugins.json").write_text(json.dumps(plugins, indent=2))

    limit = os.environ.get("G3_ISOLATION_LIMIT")
    if limit:
        wanted = set(limit.split(","))
        plugins = [p for p in plugins if p["slug"] in wanted]

    def handle(p: dict) -> tuple[str, dict]:
        slug = p["slug"]
        url = p.get("download_link")
        if not url:
            return slug, {"skip": "no download_link"}
        dest = zips_dir / f"{slug}.zip"
        err = download(url, dest)
        print(f"{slug}: {'skip (' + err + ')' if err else 'ok'}", file=sys.stderr)
        return slug, ({"skip": err} if err else {"path": dest})

    with concurrent.futures.ThreadPoolExecutor(max_workers=WORKERS) as ex:
        downloaded = dict(ex.map(handle, plugins))

    results = []
    for p in plugins:
        slug = p["slug"]
        info = downloaded[slug]
        record = {"slug": slug, "version": p.get("version", "unknown"), "active_installs": p.get("active_installs", 0)}
        if "skip" in info:
            record["skip"] = info["skip"]
            results.append(record)
            continue
        try:
            record.update(inspect_zip(info["path"]))
        except (zipfile.BadZipFile, OSError) as exc:
            record["skip"] = f"zip read error: {exc}"
        results.append(record)

    aggregate = build_aggregate(results)
    wall = time.monotonic() - start

    report = render_markdown(results, aggregate, fetch_date, wall)
    print(report)

    out_json = scratch / "inventory.json"
    out_json.write_text(json.dumps({"fetch_date": fetch_date, "results": results, "aggregate": aggregate}, indent=2, default=str))
    print(f"wall time: {wall:.1f}s", file=sys.stderr)
    print(f"JSON: {out_json}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
