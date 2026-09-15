#!/usr/bin/env python3
"""Build vivace.vandragt.com from site/pages/*.md into site/dist/.

Usage: python3 site/build.py

Renders each page with a shared HTML shell, copies site/static/ into
site/dist/, and writes CNAME and .nojekyll for GitHub Pages. index.md's
`{{viv_cold}}` style placeholders are filled from bench/results/corpus.md
and compat/results/v*.md so every number on the page has a real source --
see AGENTS.md and #249.
"""
import re
import shutil
import sys
import json
import urllib.request
from pathlib import Path

import markdown

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
DIST = SITE / "dist"
FEED = "https://vandragt.com/tag/vivace/feed.json"
GITHUB_BLOB = "https://github.com/svandragt/vivace/blob/main/"

NAV = [("Home", "index.html"), ("Compare", "compare.html"), ("Migrate", "migrate.html"),
       ("GitHub", "https://github.com/svandragt/vivace")]

SHELL = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="stylesheet" href="styles.css">
</head>
<body>
<header class="site-header">
<a class="site-title" href="index.html">viv</a>
<nav>{nav}</nav>
</header>
<main>
{content}
</main>
<footer class="site-footer">
<a href="https://github.com/svandragt/vivace">svandragt/vivace</a> on GitHub
</footer>
</body>
</html>
"""


def render_nav():
    return "\n".join(f'<a href="{href}">{label}</a>' for label, href in NAV)


def wrap_tables(html):
    """Wrap each <table> in a scrollable div so phone width never scrolls the page."""
    return re.sub(r"<table>", '<div class="table-wrap"><table>', html).replace(
        "</table>", "</table></div>"
    )


def corpus_cold_times():
    """Return (viv_cold, composer_cold, section_heading) for laravel/laravel's
    Cold column in the latest section of bench/results/corpus.md."""
    text = (ROOT / "bench/results/corpus.md").read_text()
    sections = re.split(r"^## (\S+)$", text, flags=re.MULTILINE)[1:]
    heading, body = max(zip(sections[0::2], sections[1::2]))
    row_re = re.compile(
        r"^\| laravel/laravel \| \d+ \| (composer|viv) \| (\S+) \|", re.MULTILINE
    )
    times = dict(row_re.findall(body))
    return times["viv"], times["composer"], heading


def newest_compat_file():
    def version_key(path):
        return tuple(int(p) for p in re.findall(r"\d+", path.stem))

    return max((ROOT / "compat/results").glob("v*.md"), key=version_key)


def compat_identical_count(path):
    """Return (identical_projects, total_projects) from the Pinned corpus
    table: a project counts as identical only if every row (dev, no-dev)
    for it is a byte-identical result."""
    text = path.read_text()
    section = text.split("## Pinned corpus", 1)[1].split("\n## ", 1)[0]
    projects = {}
    for line in section.splitlines():
        m = re.match(r"^\| (\S.*?) \| (dev|no-dev) \| (\S.*?) \|", line)
        if m:
            project, _, result = m.groups()
            projects.setdefault(project, True)
            projects[project] &= result.startswith("identical")
    total = len(projects)
    identical = sum(projects.values())
    return identical, total


def latest_posts():
    """Render the newest five vivace-tagged posts, or "" when the feed is
    unreachable (a vandragt.com outage must not block a deploy) or empty."""
    try:
        with urllib.request.urlopen(FEED, timeout=10) as resp:
            items = json.load(resp).get("items", [])
    except Exception as exc:  # noqa: BLE001
        print(f"site/build.py: feed unavailable, skipping posts: {exc}", file=sys.stderr)
        return ""
    if not items:
        return ""
    lines = ["## Latest posts", ""]
    for item in items[:5]:
        date = item.get("date_published", "")[:10]
        lines.append(f"- [{item['title']}]({item['url']}) <span class=\"source\">{date}</span>")
    return "\n".join(lines)


def index_placeholders():
    viv_cold, composer_cold, heading = corpus_cold_times()
    date = heading.split("T")[0]
    corpus_path = "bench/results/corpus.md"
    compat_path = newest_compat_file()
    identical, total = compat_identical_count(compat_path)
    compat_rel = compat_path.relative_to(ROOT).as_posix()
    return {
        "viv_cold": f"{float(viv_cold):.2f}s",
        "composer_cold": f"{float(composer_cold):.2f}s",
        "viv_cold_source": (
            f'<a href="{GITHUB_BLOB}{corpus_path}">{corpus_path}</a> ({date})'
        ),
        "compat_identical": f"{identical}/{total}",
        "compat_source": f'<a href="{GITHUB_BLOB}{compat_rel}">{compat_rel}</a>',
        "posts": latest_posts(),
    }


def render_page(md_path, placeholders=None):
    text = md_path.read_text()
    if placeholders:
        for key, value in placeholders.items():
            text = text.replace(f"{{{{{key}}}}}", value)
    title = text.splitlines()[0].lstrip("# ").strip()
    if title.lower() != "viv":
        title = f"{title} - viv"
    body_html = markdown.markdown(text, extensions=["tables", "fenced_code"])
    return title, wrap_tables(body_html)


def main():
    if DIST.exists():
        shutil.rmtree(DIST)
    DIST.mkdir(parents=True)

    for md_path in sorted((SITE / "pages").glob("*.md")):
        placeholders = index_placeholders() if md_path.stem == "index" else None
        title, content_html = render_page(md_path, placeholders)
        html = SHELL.format(title=title, nav=render_nav(), content=content_html)
        (DIST / f"{md_path.stem}.html").write_text(html)

    shutil.copytree(SITE / "static", DIST, dirs_exist_ok=True)
    (DIST / "CNAME").write_text("vivace.vandragt.com\n")
    (DIST / ".nojekyll").write_text("")

    index_html = (DIST / "index.html").read_text()
    if "{{" in index_html:
        print("site/build.py: unfilled {{placeholder}} in index.html", file=sys.stderr)
        return 1
    print(f"site/build.py: wrote {DIST}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
