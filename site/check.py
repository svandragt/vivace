#!/usr/bin/env python3
"""Sanity checks on site/dist, run after site/build.py (see site.yml).

Asserts: no broken internal link, every page has a non-empty <title>, every
nav.toml section has an index page, and REDIRECTS (site/build.py) covers
every old page with a stub that resolves to a real page.
"""
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build  # noqa: E402

DIST = build.DIST

HREF_RE = re.compile(r'href="(/[^"#]*)(#[^"]*)?"')
ID_RE = re.compile(r' id="([^"]+)"')
TITLE_RE = re.compile(r"<title>(.*?)</title>", re.DOTALL)


def dist_file_for(path):
    """The dist file a root-relative href like `/guides/` or
    `/compare.html` resolves to, or None if nothing matches."""
    if path in ("", "/"):
        return DIST / "index.html"
    rel = path.lstrip("/")
    if rel.endswith("/"):
        return DIST / rel / "index.html"
    return DIST / rel


def check_links():
    problems = []
    html_files = sorted(DIST.rglob("*.html"))
    ids_by_file = {f: set(ID_RE.findall(f.read_text())) for f in html_files}
    for f in html_files:
        for path, fragment in HREF_RE.findall(f.read_text()):
            target = dist_file_for(path)
            if not target.exists():
                problems.append(f"{f.relative_to(DIST)}: broken link to {path}")
                continue
            if fragment and fragment[1:] not in ids_by_file.get(target, set()):
                problems.append(f"{f.relative_to(DIST)}: broken anchor {path}{fragment}")
    return problems


def check_titles():
    problems = []
    for f in sorted(DIST.rglob("*.html")):
        match = TITLE_RE.search(f.read_text())
        if not match or not match.group(1).strip():
            problems.append(f"{f.relative_to(DIST)}: no <title>")
    return problems


def check_section_indexes():
    problems = []
    for section in build.load_nav():
        if not (DIST / section["slug"] / "index.html").exists():
            problems.append(f"section '{section['slug']}' has no index page")
    return problems


def check_redirects():
    problems = []
    for old_name, url in build.REDIRECTS.items():
        stub = DIST / old_name
        if not stub.exists():
            problems.append(f"redirect stub missing for {old_name}")
            continue
        if not dist_file_for(url).exists():
            problems.append(f"{old_name} redirects to {url}, which doesn't exist")
    return problems


def check_css():
    """Fluid type/space tokens are defined, and main's max-width cap
    (which used to fight the grid column for width) is gone."""
    problems = []
    css = (DIST / "styles.css").read_text()
    for token in ("--step-0", "--space-m", "--measure"):
        if token not in css:
            problems.append(f"styles.css: missing {token}")
    match = re.search(r"(?m)^main\s*\{([^}]*)\}", css)
    if not match:
        problems.append("styles.css: no `main {}` rule found")
    elif "max-width" in match.group(1):
        problems.append("styles.css: `main {}` still sets max-width")
    return problems


def check_footnotes():
    """A literal [^n] in the HTML means the markdown "footnotes" extension
    was not applied to that page."""
    problems = []
    for f in sorted(DIST.rglob("*.html")):
        if re.search(r"\[\^\d+\]", f.read_text()):
            problems.append(f"{f.relative_to(DIST)}: literal footnote marker")
    return problems


def main():
    if not DIST.exists():
        raise SystemExit("site/check.py: run site/build.py first")
    problems = (
        check_links() + check_titles() + check_section_indexes() + check_redirects()
        + check_footnotes() + check_css()
    )
    if problems:
        for p in problems:
            print(f"site/check.py: {p}", file=sys.stderr)
        print(f"site/check.py: {len(problems)} problem(s)", file=sys.stderr)
        return 1
    print("site/check.py: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())

