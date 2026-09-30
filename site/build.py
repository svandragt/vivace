#!/usr/bin/env python3
"""Build vivace.vandragt.com from site/pages/*.md into site/dist/.

Usage: python3 site/build.py

Renders each page with a shared HTML shell, copies site/static/ into
site/dist/, and writes CNAME and .nojekyll for GitHub Pages. Each page's
`{{placeholder}}` style tags are filled from bench/results/corpus.md,
compat/results/v*.md, docs/plugin-strategy.md, docs/stability.md and
README.md so every number and claim on the site has a real source -- see
AGENTS.md, #249, #251 and #252.
"""
import os
import re
import shutil
import statistics
import subprocess
import sys
import html as html_lib
import json
import tomllib
import urllib.request
from pathlib import Path

import markdown

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
DIST = SITE / "dist"
FEED = "https://vandragt.com/tag/vivace/feed.json"
GITHUB_BLOB = "https://github.com/svandragt/vivace/blob/main/"

GITHUB_URL = "https://github.com/svandragt/vivace"

# The sidebar's first group, above the section list (#site-vertical-nav):
# label, href, whether it's external (adds rel="external", no site page to
# check a link against).
TOP_NAV = [
    ("Compare", "/compare.html", False),
    ("News", "/news.html", False),
    ("GitHub", GITHUB_URL, True),
]

# Old flat page -> new URL, so every link this site ever published still
# resolves (#site-structure). Two entries collapse two old pages into one
# merged page (manual+getting-started, commands+reference).
REDIRECTS = {
    "manual.html": "/getting-started/",
    "getting-started.html": "/getting-started/",
    "cheatsheet.html": "/guides/cheatsheet.html",
    "install.html": "/getting-started/install.html",
    "commands.html": "/reference/",
    "shim.html": "/getting-started/shim.html",
    "migrate.html": "/guides/migrate.html",
    "frameworks.html": "/guides/frameworks.html",
    "plugins.html": "/guides/plugins.html",
    "cache.html": "/guides/cache.html",
    "compatibility.html": "/reference/compatibility.html",
    "reference.html": "/reference/",
    "troubleshooting.html": "/guides/troubleshooting.html",
    "support.html": "/getting-started/support.html",
}

SHELL = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<link rel="stylesheet" href="/styles.css">
</head>
<body>
<a class="skip-link" href="#main-content">Skip to content</a>
<header class="site-header">
<a class="site-title" href="/">viv</a>
<input type="search" id="site-search" placeholder="Search" aria-label="Search the manual" autocomplete="off">
</header>
<div id="search-results" hidden></div>
{layout}
<footer class="site-footer">
<a href="https://github.com/svandragt/vivace">svandragt/vivace</a> on GitHub
</footer>
<script src="/search.js" defer></script>
<script src="/nav.js" defer></script>
</body>
</html>
"""

REDIRECT_SHELL = """<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta http-equiv="refresh" content="0; url={url}">
<link rel="canonical" href="{url}">
<title>Moved - viv</title>
</head>
<body>
<p>This page moved to <a href="{url}">{url}</a>.</p>
</body>
</html>
"""


def render_top_nav(current_url):
    """The sidebar's first group: Compare, News, GitHub, above the section
    list. Current-page highlighting works the same as a section link."""
    links = []
    for label, href, external in TOP_NAV:
        attrs = ' rel="external"' if external else ""
        if href == current_url:
            attrs += ' class="current" aria-current="page"'
        links.append(f'<a href="{href}"{attrs}>{label}</a>')
    return '<div class="sidebar-top">\n' + "\n".join(links) + "\n</div>"


def load_nav():
    """The left sidebar's sections, from site/nav.toml, in reader order."""
    data = tomllib.loads((SITE / "nav.toml").read_text())
    return sorted(data["section"], key=lambda s: s["order"])


FRONT_MATTER_RE = re.compile(r"\A---\n(.*?)\n---\n", re.DOTALL)


def parse_front_matter(text):
    """Split a page's front matter (flat `key: value` lines only -- title,
    order, summary never need more) from its Markdown body."""
    match = FRONT_MATTER_RE.match(text)
    if not match:
        return {}, text
    meta = {}
    for line in match.group(1).splitlines():
        if not line.strip():
            continue
        key, _, value = line.partition(":")
        meta[key.strip()] = value.strip()
    if "order" in meta:
        meta["order"] = int(meta["order"])
    return meta, text[match.end():]


def read_section_page(path, section):
    """A section page's front matter plus body, as the page dict every
    renderer below shares: title is required so a page can't silently go
    nameless in the sidebar or a browser tab."""
    meta, body = parse_front_matter(path.read_text())
    if "title" not in meta:
        raise SystemExit(f"site/build.py: {path} is missing required front matter 'title'")
    rel = path.relative_to(ROOT).as_posix()
    return make_page(section, path.stem, meta.get("title"), meta.get("order", 999),
                      meta.get("summary", ""), body, src_rel=rel)


def make_page(section, slug, title, order, summary, body, src_rel=None, nav_slug=None,
              in_sequence=True):
    return {
        "section": section,
        "slug": slug,
        "title": title,
        "order": order,
        "summary": summary,
        "body": body,
        "src_rel": src_rel,
        "url": page_url(section, slug),
        "nav_slug": nav_slug if nav_slug is not None else slug,
        "in_sequence": in_sequence,
    }


def page_url(section, slug):
    if section is None:
        return f"/{slug}.html"
    if slug == "index":
        return f"/{section}/"
    return f"/{section}/{slug}.html"


def page_dist_path(page):
    if page["section"] is None:
        return DIST / f"{page['slug']}.html"
    d = DIST / page["section"]
    d.mkdir(parents=True, exist_ok=True)
    return d / "index.html" if page["slug"] == "index" else d / f"{page['slug']}.html"


def section_sequence(pages_by_section, slug):
    """A section's pages in prev/next and sidebar order: its index first,
    then the rest by `order`, then slug."""
    pages = pages_by_section[slug]
    rest = sorted((p for p in pages if p["slug"] != "index"), key=lambda p: (p["order"], p["slug"]))
    index_pages = [p for p in pages if p["slug"] == "index"]
    return index_pages + rest


def render_sidebar(sections, pages_by_section, current_section, current_slug, current_url):
    """The left nav tree: the top group (render_top_nav) above one
    collapsible group per section (a <details>, open when it's the current
    section) so expand/collapse needs no JS at all and the whole thing is
    already a plain nested list without it."""
    groups = [render_top_nav(current_url)]
    for sec in sections:
        seq = section_sequence(pages_by_section, sec["slug"])
        is_current_section = sec["slug"] == current_section
        links = []
        for p in seq:
            is_current = is_current_section and p["nav_slug"] == current_slug
            # Getting started's own index page is reached at / (#site-vertical-nav),
            # not /getting-started/, so its sidebar entry links straight there.
            href = "/" if sec["slug"] == "getting-started" and p["slug"] == "index" else p["url"]
            attrs = ' class="current" aria-current="page"' if is_current else ""
            tip = f' title="{html_lib.escape(p["summary"])}"' if p["summary"] else ""
            links.append(f'<a href="{href}"{attrs}{tip}>{html_lib.escape(p["title"])}</a>')
        groups.append(
            f'<details{" open" if is_current_section else ""}>\n'
            f'<summary>{html_lib.escape(sec["title"])}</summary>\n' + "\n".join(links) + "\n</details>"
        )
    return "\n".join(groups)


def render_breadcrumbs(sections_by_slug, page):
    if page["url"] == "/":
        return '<nav class="breadcrumbs" aria-label="Breadcrumb"><ol><li aria-current="page">Home</li></ol></nav>'
    items = ['<li><a href="/">Home</a></li>']
    section = page["section"]
    if section:
        title = sections_by_slug[section]["title"]
        if page["slug"] == "index":
            items.append(f'<li aria-current="page">{html_lib.escape(title)}</li>')
        else:
            items.append(f'<li><a href="/{section}/">{html_lib.escape(title)}</a></li>')
            items.append(f'<li aria-current="page">{html_lib.escape(page["title"])}</li>')
    else:
        items.append(f'<li aria-current="page">{html_lib.escape(page["title"])}</li>')
    return '<nav class="breadcrumbs" aria-label="Breadcrumb"><ol>' + "".join(items) + "</ol></nav>"


TOC_HEADING_RE = re.compile(r'<h([23]) id="([^"]+)">(.*?)</h\1>', re.DOTALL)


def render_toc(content_html):
    """"On this page": every h2/h3, ids from the toc extension's own
    output, every h3 nested under the h2 above it (a stray leading h3, with
    no h2 yet, just sits at the top level rather than nesting nowhere)."""
    headings = TOC_HEADING_RE.findall(content_html)
    if not headings:
        return ""

    def link(hid, text):
        return f'<a href="#{hid}">{excerpt(text, 80)}</a>'

    out = ['<p class="toc-title">On this page</p>', "<ul>"]
    h2_open = h3_list_open = False
    for level, hid, text in headings:
        if level == "2":
            if h3_list_open:
                out.append("</ul>")
                h3_list_open = False
            if h2_open:
                out.append("</li>")
            out.append(f"<li>{link(hid, text)}")
            h2_open = True
        else:
            if not h2_open:
                out.append(f"<li>{link(hid, text)}</li>")
                continue
            if not h3_list_open:
                out.append("<ul>")
                h3_list_open = True
            out.append(f"<li>{link(hid, text)}</li>")
    if h3_list_open:
        out.append("</ul>")
    if h2_open:
        out.append("</li>")
    out.append("</ul>")
    return "\n".join(out)


def render_prevnext(prev_page, next_page):
    if not prev_page and not next_page:
        return ""
    parts = ['<nav class="prev-next" aria-label="Page navigation">']
    parts.append(
        f'<a class="prev" href="{prev_page["url"]}">&larr; {html_lib.escape(prev_page["title"])}</a>'
        if prev_page else "<span></span>"
    )
    parts.append(
        f'<a class="next" href="{next_page["url"]}">{html_lib.escape(next_page["title"])} &rarr;</a>'
        if next_page else "<span></span>"
    )
    parts.append("</nav>")
    return "\n".join(parts)


def render_edit_link(src_rel):
    if not src_rel:
        return ""
    return f'<p class="edit-link"><a href="{GITHUB_BLOB}{src_rel}">Edit this page on GitHub</a></p>'


def find_viv_binary():
    """The `viv` binary to run for `--help` text: $VIV, else `viv` on PATH,
    else the release build. Missing entirely fails the build rather than
    letting the command reference go stale silently."""
    env_viv = os.environ.get("VIV")
    if env_viv:
        return env_viv
    on_path = shutil.which("viv")
    if on_path:
        return on_path
    release = ROOT / "target/release/viv"
    if release.exists():
        return str(release)
    raise SystemExit(
        "site/build.py: no viv binary found -- set $VIV, put viv on PATH, or "
        "build ./target/release/viv (devbox run -- cargo build --release)"
    )


def viv_help(cmd):
    """`$VIV <cmd> --help` output, ready to drop into a fenced code block. An
    empty cmd means bare `viv --help`, the global options and command list."""
    binary = find_viv_binary()
    args = [binary, "--help"] if not cmd else [binary, *cmd.split(), "--help"]
    try:
        result = subprocess.run(args, capture_output=True, text=True, check=True)
    except FileNotFoundError:
        raise SystemExit(f"site/build.py: $VIV points at a missing binary: {binary}")
    return result.stdout.strip("\n")


def wrap_tables(html):
    """Wrap each <table> in a scrollable div so phone width never scrolls the page."""
    return re.sub(r"<table>", '<div class="table-wrap"><table>', html).replace(
        "</table>", "</table></div>"
    )


def latest_corpus_section(text):
    """Return (heading, body) for the newest `## <timestamp>` section of a
    corpus.md-shaped file."""
    sections = re.split(r"^## (\S+)$", text, flags=re.MULTILINE)[1:]
    return max(zip(sections[0::2], sections[1::2]))


CORPUS_TOOLS = ("composer", "riff", "viv", "vivacity")
CORPUS_SCENARIOS = ("Cold", "Warm", "No-op", "Update-warm")
CORPUS_ROW_RE = re.compile(
    r"^\| (\S.*?) \| \d+ \| (composer|riff|viv|vivacity) \|"
    r" (\S+) \| (\S+) \| (\S+) \| (\S+) \|$",
    re.MULTILINE,
)


def corpus_row_values(body):
    """Return {project: {tool: {scenario: value_or_None}}} for every row in
    a corpus.md section body."""
    projects = {}
    for project, tool, *values in CORPUS_ROW_RE.findall(body):
        projects.setdefault(project, {})[tool] = {
            scenario: (None if v == "n/a" else float(v))
            for scenario, v in zip(CORPUS_SCENARIOS, values)
        }
    return projects


def corpus_speed_table(body):
    """Return {tool: {scenario: (median, included, total) or None}}, the
    median across projects with a numeric value plus coverage, since a tool
    that didn't run everywhere shouldn't average its gaps in as zero."""
    projects = corpus_row_values(body)
    total = len(projects)
    table = {}
    for tool in CORPUS_TOOLS:
        table[tool] = {}
        for scenario in CORPUS_SCENARIOS:
            values = [
                p[tool][scenario]
                for p in projects.values()
                if tool in p and p[tool][scenario] is not None
            ]
            table[tool][scenario] = (
                (statistics.median(values), len(values), total) if values else None
            )
    return table


def format_speed_cell(entry):
    """Median plus a small coverage count when a tool ran on fewer projects
    than the total, "n/a" style coverage when it ran on none at all."""
    if entry is None:
        return "n/a"
    median, included, total = entry
    cell = f"{median:.2f}s"
    if included < total:
        cell += f" ({included}/{total})"
    return cell


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


def plugin_adapter_count():
    """Count viv's native adapters in docs/plugin-strategy.md's inventory
    table (rows marked "Native", not the "Known inert" ones)."""
    text = (ROOT / "docs/plugin-strategy.md").read_text()
    section = text.split("## Inventory", 1)[1].split("\n## ", 1)[0]
    return section.count("| Native (")


def github_anchor(heading):
    """GitHub's Markdown heading slug: lowercase, spaces to hyphens, drop
    anything that isn't alphanumeric, space or hyphen."""
    slug = re.sub(r"[^\w\s-]", "", heading.lower())
    return re.sub(r"\s+", "-", slug.strip())


def rewrite_relative_links(text, current_rel_path):
    """Point a doc's relative and #anchor links at the file on GitHub, so a
    snippet lifted onto the site keeps working outside the repo."""
    current_dir = Path(current_rel_path).parent

    def replace(m):
        label, target = m.group(1), m.group(2)
        if target.startswith(("http://", "https://")):
            return m.group(0)
        if target.startswith("#"):
            return f"[{label}]({GITHUB_BLOB}{current_rel_path}{target})"
        path_part, _, anchor = target.partition("#")
        resolved = (ROOT / current_dir / path_part).resolve().relative_to(ROOT).as_posix()
        url = f"{GITHUB_BLOB}{resolved}"
        if anchor:
            url += f"#{anchor}"
        return f"[{label}]({url})"

    return re.sub(r"\[([^\]]*)\]\(([^)]+)\)", replace, text)


def demote_relative(text, offset):
    """Shift every Markdown heading in text by offset levels (clamped to
    h1..h6), so a section lifted from a README/doc nests under whatever
    page heading precedes it rather than always one level down."""
    if offset == 0:
        return text

    def shift(match):
        level = max(1, min(6, len(match.group(1)) + offset))
        return "#" * level + " "

    return re.sub(r"^(#{1,6}) ", shift, text, flags=re.MULTILINE)


def section_from_file(rel_path, heading):
    """Return (level, body) for a `## <heading>` or `### <heading>` section
    in a file at rel_path (up to the next heading of that level or
    shallower): level is that heading's own depth, and body is the section
    text below it, links rewritten to point at GitHub. Headings inside body
    are left at their source depth -- callers demote them relative to
    wherever the section lands."""
    text = (ROOT / rel_path).read_text()
    start = re.search(rf"^(#{{2,3}}) {re.escape(heading)}\n", text, re.MULTILINE)
    if not start:
        raise SystemExit(f"site/build.py: heading '{heading}' not found in {rel_path}")
    level = len(start.group(1))
    rest = text[start.end():]
    end = re.search(rf"^#{{1,{level}}} ", rest, re.MULTILINE)
    body = (rest[: end.start()] if end else rest).strip("\n")
    # Footnote markers ([^12]) point at definitions outside this section;
    # the "From the README" link above each section is where to find them.
    body = re.sub(r"\[\^\d+\]", "", body)
    return level, rewrite_relative_links(body, rel_path)


def doc_section(path, heading, offset=1):
    level, body = section_from_file(path, heading)
    return demote_relative(body, offset)


GENERIC_PLACEHOLDER_RE = re.compile(r"\{\{(help|include):([^}]*)\}\}")


def preceding_heading_level(text, pos):
    """The level of the last Markdown heading before pos in text, or 1 (the
    page's own `#` title) when nothing precedes it."""
    heads = list(re.finditer(r"^(#{1,6}) ", text[:pos], re.MULTILINE))
    return len(heads[-1].group(1)) if heads else 1


def include_whole_file(rel_path):
    """{{include:<path>}} with no #heading: the file's whole body, its own
    first H1 dropped and every remaining heading shifted down one level so
    it nests under the page's own H1 instead of colliding with it."""
    text = (ROOT / rel_path).read_text()
    text = re.sub(r"\A# [^\n]*\n+", "", text, count=1)
    return demote_relative(rewrite_relative_links(text, rel_path), 1)


def resolve_generic_placeholder(match):
    kind, arg = match.group(1), match.group(2)
    if kind == "help":
        return viv_help(arg)
    path, _, heading = arg.partition("#")
    if not heading:
        return include_whole_file(path)
    # Demote so the section's own top heading lands one level below whatever
    # page heading precedes this placeholder, not always one level down.
    page_level = preceding_heading_level(match.string, match.start())
    level, body = section_from_file(path, heading)
    return demote_relative(body, page_level - level)


def stability_summary():
    """The first paragraph under docs/stability.md's first heading, links
    rewritten the same way as section_from_file."""
    text = (ROOT / "docs/stability.md").read_text()
    para = re.search(r"^## [^\n]*\n\n(.*?)\n\n", text, re.MULTILINE | re.DOTALL).group(1)
    return rewrite_relative_links(para, "docs/stability.md")


def compare_placeholders():
    corpus_path = "bench/results/corpus.md"
    text = (ROOT / corpus_path).read_text()
    heading, body = latest_corpus_section(text)
    date = heading.split("T")[0]
    version_line = body.strip().splitlines()[0]
    skip_notes = re.findall(r"^- \S.*$", body, re.MULTILINE)
    speed = corpus_speed_table(body)

    tool_labels = {"composer": "Composer", "riff": "riff", "viv": "viv", "vivacity": "vivacity"}
    speed_lines = [
        "| Tool | Cold | Warm | No-op | Update-warm |",
        "|---|---|---|---|---|",
    ]
    for tool in CORPUS_TOOLS:
        cells = " | ".join(format_speed_cell(speed[tool][s]) for s in CORPUS_SCENARIOS)
        speed_lines.append(f"| {tool_labels[tool]} | {cells} |")

    installed = {
        tool: (speed[tool]["Cold"][1] if speed[tool]["Cold"] else 0) for tool in CORPUS_TOOLS
    }
    total_projects = len(corpus_row_values(body))

    compat_path = newest_compat_file()
    identical, compat_total = compat_identical_count(compat_path)
    compat_rel = compat_path.relative_to(ROOT).as_posix()
    adapters = plugin_adapter_count()

    capability_lines = [
        "| | [Composer](https://getcomposer.org) | [riff](https://github.com/shyim/riff) | [vivacity](https://github.com/Adelagric/vivacity) | [viv](https://github.com/svandragt/vivace) |",
        "|---|---|---|---|---|",
        f"| Byte-identical `vendor/` | is the reference "
        f"| no — 4 differences (autoloader-suffix, `provide` order, "
        f"`NULL` vs `null`, a newer `InstalledVersions.php`) "
        f"([JOURNAL.md, 2026-09-06]({GITHUB_BLOB}JOURNAL.md)) "
        f"| not measured here "
        f"| yes, {identical}/{compat_total} pinned projects "
        f"([{compat_rel}]({GITHUB_BLOB}{compat_rel})) |",
        f"| Corpus projects installed (of {total_projects}) | {installed['composer']} "
        f"| {installed['riff']} | {installed['vivacity']} | {installed['viv']} |",
        f"| Plugins handled natively | all (runs PHP) | not measured here "
        f"| its known list ([JOURNAL.md, 2026-09-15]({GITHUB_BLOB}JOURNAL.md)) "
        f"| {adapters} adapters "
        f"([docs/plugin-strategy.md]({GITHUB_BLOB}docs/plugin-strategy.md)) |",
        "| Needs PHP to install | yes | no | no | no |",
        "| Drop-in `composer` shim | — | — | — | yes |",
        f"| Publishes its failures | — | — | NOTICE, changelog "
        f"| [compat/results/]({GITHUB_BLOB}compat/results), "
        f"[bench/results/corpus.md]({GITHUB_BLOB}{corpus_path}) footnotes, "
        f"[JOURNAL.md]({GITHUB_BLOB}JOURNAL.md) |",
    ]

    return {
        "speed_table": "\n".join(speed_lines),
        "speed_source": (
            f"{version_line}\n<span class=\"source\">source: "
            f'<a href="{GITHUB_BLOB}{corpus_path}">{corpus_path}</a>, '
            f'section <code>{heading}</code> ({date})</span>'
        ),
        "speed_skips": "\n".join(skip_notes),
        "capability_table": "\n".join(capability_lines),
    }


def readme_source_line(heading):
    anchor = github_anchor(heading)
    return (
        f'<span class="source">From the README: '
        f'<a href="{GITHUB_BLOB}README.md#{anchor}">{heading}</a></span>'
    )


def site_page_body(rel_path):
    """A site page's markdown body: front matter and the H1 removed, so the
    generated framework pages reuse the getting-started text as one source
    (the README no longer carries it)."""
    text = (ROOT / "site" / "pages" / rel_path).read_text()
    _, body = parse_front_matter(text)
    lines = body.lstrip("\n").split("\n")
    if lines and lines[0].startswith("# "):
        lines = lines[1:]
    return "\n".join(lines).strip("\n")


def site_source_line(rel_path, title):
    slug = rel_path[: -len(".md")]
    return f'<span class="source">source: <a href="/{slug}.html">{title}</a></span>'


def shim_section():
    return site_page_body("getting-started/shim.md")


def ci_snippet():
    return site_page_body("getting-started/ci.md")


def dockerfile_section():
    return site_page_body("getting-started/docker.md")


def migrate_placeholders():
    return {
        "shim_from_readme": site_source_line("getting-started/shim.md", "Using viv as composer"),
        "shim_section": shim_section(),
        "dockerfile_from_readme": site_source_line("getting-started/docker.md", "In a Dockerfile"),
        "dockerfile_section": dockerfile_section(),
        "ci_from_readme": site_source_line("getting-started/ci.md", "In CI"),
        "ci_snippet": ci_snippet(),
        "stability_summary": (
            f"{stability_summary()}\n\n"
            f'<span class="source">source: <a href="{GITHUB_BLOB}docs/stability.md">docs/stability.md</a></span>'
        ),
    }


# One entry per framework page (#265): slug, display name, the corpus/compat
# project name, and the Composer plugin packages that framework's starter
# usually enables (see docs/plugin-strategy.md's Inventory table).
FRAMEWORKS = [
    {"slug": "for-laravel", "name": "Laravel", "project": "laravel/laravel", "plugins": []},
    {
        "slug": "for-symfony",
        "name": "Symfony",
        "project": "symfony/demo",
        "plugins": ["symfony/flex", "symfony/runtime"],
    },
    {
        "slug": "for-drupal",
        "name": "Drupal",
        "project": "drupal/recommended-project",
        "plugins": [
            "drupal/core-composer-scaffold",
            "drupal/core-project-message",
            "drupal/core-recipe-unpack",
            "composer/installers",
        ],
    },
    {
        "slug": "for-wordpress",
        "name": "WordPress (Bedrock)",
        "project": "roots/bedrock",
        "plugins": ["composer/installers", "johnpbloch/wordpress-core-installer"],
    },
    {
        "slug": "for-craft",
        "name": "Craft CMS",
        "project": "craftcms/craft",
        "plugins": ["craftcms/plugin-installer", "yiisoft/yii2-composer"],
    },
    {"slug": "for-statamic", "name": "Statamic", "project": "statamic/statamic", "plugins": []},
]


def fmt_time(value):
    return f"{value:.2f}s" if value is not None else "n/a"


def framework_corpus_times(project):
    """Return (composer_cold, composer_warm, viv_cold, viv_warm, heading,
    body) for project's row in the latest bench/results/corpus.md section,
    values float or None (a `n/a` cell)."""
    text = (ROOT / "bench/results/corpus.md").read_text()
    heading, body = latest_corpus_section(text)
    row = corpus_row_values(body).get(project, {})
    composer, viv = row.get("composer", {}), row.get("viv", {})
    return composer.get("Cold"), composer.get("Warm"), viv.get("Cold"), viv.get("Warm"), heading, body


def plugin_portable_text(text, plugin):
    """The "Portable?" text for plugin from docs/plugin-strategy.md's
    Inventory section: its table cell, or -- for a plugin only named in the
    prose below the table, like symfony/flex -- the parenthetical after its
    name. Raises if the plugin is in neither, so a framework page's plugin
    list can't drift from the inventory (#265)."""
    section = text.split("## Inventory", 1)[1].split("\n## ", 1)[0]
    row = re.search(rf"^\| {re.escape(plugin)} \| [^|]*\| (.*) \|$", section, re.MULTILINE)
    if row:
        cell = row.group(1).strip()
    else:
        prose = re.search(rf"{re.escape(plugin)}\s*\n?\(([^)]*)\)", section)
        if not prose:
            raise SystemExit(
                f"site/build.py: plugin '{plugin}' not found in "
                "docs/plugin-strategy.md's Inventory section"
            )
        cell = prose.group(1).strip()
    return rewrite_relative_links(cell, "docs/plugin-strategy.md")


def compat_project_rows(path, project):
    """Return [(mode, result, details), ...] for project's dev/no-dev rows
    in path's Pinned corpus table."""
    section = path.read_text().split("## Pinned corpus", 1)[1].split("\n## ", 1)[0]
    row_re = re.compile(
        rf"^\| {re.escape(project)} \| (dev|no-dev) \| (\S.*?) \| \S+ \| (.*) \|$",
        re.MULTILINE,
    )
    return row_re.findall(section)


def framework_page(entry):
    """The full Markdown for one framework page, or None if its project's
    Composer/viv Cold time in the latest corpus section is missing -- a
    framework with no data gets no page and no hub listing (#265)."""
    corpus_path = "bench/results/corpus.md"
    composer_cold, composer_warm, viv_cold, viv_warm, heading, body = framework_corpus_times(
        entry["project"]
    )
    if composer_cold is None or viv_cold is None:
        return None
    date = heading.split("T")[0]
    version_line = body.strip().splitlines()[0]
    times_source = (
        f"{version_line}\n"
        f'<span class="source">source: <a href="{GITHUB_BLOB}{corpus_path}">{corpus_path}</a>, '
        f'section <code>{heading}</code> ({date})</span>'
    )

    if entry["plugins"]:
        strategy_text = (ROOT / "docs/plugin-strategy.md").read_text()
        plugin_rows = "\n".join(
            f"| {p} | {plugin_portable_text(strategy_text, p)} |" for p in entry["plugins"]
        )
        plugins_md = (
            "| Plugin | viv |\n"
            "|---|---|\n"
            f"{plugin_rows}\n\n"
            f'<span class="source">source: <a href="{GITHUB_BLOB}docs/plugin-strategy.md">docs/plugin-strategy.md</a></span>'
        )
    else:
        plugins_md = (
            f"{entry['project']} enables no Composer plugins, so `viv install` "
            "runs without `--no-plugins`."
        )

    compat_path = newest_compat_file()
    compat_rel = compat_path.relative_to(ROOT).as_posix()
    compat_rows = compat_project_rows(compat_path, entry["project"])
    compat_lines = ["| Mode | Result | Details |", "|---|---|---|"]
    compat_lines += [f"| {mode} | {result} | {details} |" for mode, result, details in compat_rows]
    compat_md = (
        "\n".join(compat_lines)
        + f'\n\n<span class="source">source: <a href="{GITHUB_BLOB}{compat_rel}">{compat_rel}</a></span>'
    )

    return f"""# {entry['name']}

Install times for {entry['project']}, the {entry['name']} starter in viv's benchmark corpus.

| | Composer | viv |
|---|---|---|
| Cold | {fmt_time(composer_cold)} | {fmt_time(viv_cold)} |
| Warm | {fmt_time(composer_warm)} | {fmt_time(viv_warm)} |

{times_source}

## Plugins

{plugins_md}

## Local

{site_source_line("getting-started/shim.md", "Using viv as composer")}

{shim_section()}

## Dockerfile

{site_source_line("getting-started/docker.md", "In a Dockerfile")}

{dockerfile_section()}

## CI

{site_source_line("getting-started/ci.md", "In CI")}

{ci_snippet()}

## Compatibility sweep

{compat_md}
"""


def excerpt(html, limit=80):
    text = html_lib.unescape(re.sub(r"<[^>]+>", " ", html))
    text = " ".join(text.split())
    return text if len(text) <= limit else text[:limit].rsplit(" ", 1)[0] + "…"


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
        # JSON Feed makes `title` optional; a status post has none, so use
        # the first line of its text instead.
        title = item.get("title") or excerpt(item.get("content_text") or item.get("content_html", ""))
        lines.append(f"- [{title}]({item['url']}) <span class=\"source\">{date}</span>")
    return "\n".join(lines)


def documentation_links():
    """The homepage's "Documentation" block: one line per site/nav.toml
    section, title and summary from that section's own index.md front
    matter, so this list can't drift from the sidebar it mirrors."""
    lines = []
    for sec in load_nav():
        slug = sec["slug"]
        url = "/" if slug == "getting-started" else page_url(slug, "index")
        meta, _ = parse_front_matter((SITE / "pages" / slug / "index.md").read_text())
        lines.append(f"- [{meta['title']}]({url}) -- {meta['summary']}")
    return "\n".join(lines)


def index_placeholders():
    return {
        "getting_started_body": site_page_body("getting-started/index.md"),
        "doc_sections": documentation_links(),
    }


def news_placeholders():
    return {"posts": latest_posts()}


CHANGELOG_RE = re.compile(r"^## \[(.+?)\](?: - (\S+))?$", re.MULTILINE)


def changelog_releases():
    """One page per CHANGELOG.md version, newest first (the file's own
    order), skipping Unreleased when it has no entries yet."""
    text = (ROOT / "CHANGELOG.md").read_text()
    heads = list(CHANGELOG_RE.finditer(text))
    pages = []
    for i, head in enumerate(heads):
        version, date = head.group(1), head.group(2)
        end = heads[i + 1].start() if i + 1 < len(heads) else len(text)
        body = text[head.end():end].strip("\n")
        if version == "Unreleased" and not body.strip():
            continue
        dated = f"Released {date}" if date else "Not yet released"
        page_md = f"# {version}\n\n{dated}\n\n{body}\n"
        slug = version if version != "Unreleased" else "unreleased"
        pages.append(make_page("releases", slug, version, len(pages), dated, page_md))
    return pages


PAGE_PLACEHOLDERS = {
    "index": index_placeholders,
    "compare": compare_placeholders,
    "news": news_placeholders,
    "guides/migrate": migrate_placeholders,
}


def markdown_to_html(text):
    return wrap_tables(markdown.markdown(text, extensions=["tables", "fenced_code", "toc", "footnotes"]))


def page_title_tag(title):
    return title if title.lower() == "viv" else f"{title} - viv"


def apply_custom_placeholders(body, custom_fn):
    text = body
    if custom_fn:
        for key, value in custom_fn().items():
            text = text.replace(f"{{{{{key}}}}}", value)
    return text


def resolve_body(body, custom_fn):
    text = apply_custom_placeholders(body, custom_fn)
    return GENERIC_PLACEHOLDER_RE.sub(resolve_generic_placeholder, text)


H2_RE = re.compile(r'<h2 id="([^"]+)">(.*?)</h2>', re.DOTALL)


def index_sections(url, title, content_html):
    """One search record for the page's intro (before its first h2) plus one
    per h2 section, {url, page, heading, text}; ids come from the toc
    extension's own <h2 id="..."> output rather than recomputing a slug."""
    title = title.removesuffix(" - viv")
    matches = list(H2_RE.finditer(content_html))
    records = []
    intro_end = matches[0].start() if matches else len(content_html)
    intro_text = excerpt(content_html[:intro_end], 400)
    if intro_text:
        records.append({"url": url, "page": title, "heading": title, "text": intro_text})
    for i, match in enumerate(matches):
        start = match.end()
        end = matches[i + 1].start() if i + 1 < len(matches) else len(content_html)
        records.append({
            "url": f"{url}#{match.group(1)}",
            "page": title,
            "heading": excerpt(match.group(2), 200),
            "text": excerpt(content_html[start:end], 400),
        })
    return records


def render_doc_page(page, sections, pages_by_section, sections_by_slug, search_index,
                     sidebar_current=None):
    """A page inside the sidebar/breadcrumb/TOC layout: every section page,
    the generated framework and release pages, and the top-level compare.md,
    news.md and index.md. sidebar_current overrides which section/slug the
    sidebar marks current, for the homepage (index.md) which reuses the
    getting-started section's content but isn't part of any section's own
    page sequence. Returns the rendered HTML; the caller decides where it
    lands on disk."""
    key = f"{page['section']}/{page['slug']}" if page["section"] else page["slug"]
    custom_fn = PAGE_PLACEHOLDERS.get(key)
    content_html = markdown_to_html(resolve_body(page["body"], custom_fn))
    title_tag = page_title_tag(page["title"])
    search_index.extend(index_sections(page["url"], title_tag, content_html))

    prev_page = next_page = None
    if page["section"] and page["in_sequence"]:
        seq = section_sequence(pages_by_section, page["section"])
        idx = next(i for i, p in enumerate(seq) if p["slug"] == page["slug"])
        prev_page = seq[idx - 1] if idx > 0 else None
        next_page = seq[idx + 1] if idx + 1 < len(seq) else None

    current_section, current_slug = sidebar_current or (page["section"], page["nav_slug"])
    layout = (
        '<div class="layout">\n'
        '<button type="button" id="sidebar-toggle" class="sidebar-toggle" '
        'aria-expanded="false" aria-controls="sidebar">Menu</button>\n'
        f'<nav class="sidebar" id="sidebar" aria-label="Sections">\n'
        f'{render_sidebar(sections, pages_by_section, current_section, current_slug, page["url"])}\n'
        "</nav>\n"
        '<div class="content">\n'
        f'{render_breadcrumbs(sections_by_slug, page)}\n'
        f'<main id="main-content">\n{content_html}\n'
        f'{render_prevnext(prev_page, next_page)}\n'
        f'{render_edit_link(page["src_rel"])}\n'
        "</main>\n"
        "</div>\n"
        f'<aside class="page-toc" aria-label="On this page">\n{render_toc(content_html)}\n</aside>\n'
        "</div>"
    )
    return SHELL.format(title=title_tag, layout=layout)


def main():
    if DIST.exists():
        shutil.rmtree(DIST)
    DIST.mkdir(parents=True)

    search_index = []
    sections = load_nav()
    sections_by_slug = {s["slug"]: s for s in sections}
    pages_by_section = {s["slug"]: [] for s in sections}

    for slug in pages_by_section:
        for md_path in sorted((SITE / "pages" / slug).glob("*.md")):
            pages_by_section[slug].append(read_section_page(md_path, slug))
        if not any(p["slug"] == "index" for p in pages_by_section[slug]):
            raise SystemExit(f"site/build.py: section '{slug}' has no index.md")

    # Generated pages, not files under site/pages/ (#265): built before the
    # main render loop so guides/frameworks.md's {{framework_list}} can list
    # only the ones with data, and none of them takes part in guides' own
    # prev/next sequence or sidebar listing -- they're reached only from
    # that hub page, same as before this restructure.
    frameworks_generated = []
    framework_pages = []
    for entry in FRAMEWORKS:
        md_text = framework_page(entry)
        if md_text is None:
            continue
        page = make_page("guides", entry["slug"], entry["name"], 999, "", md_text,
                          nav_slug="frameworks", in_sequence=False)
        framework_pages.append(page)
        viv_cold = framework_corpus_times(entry["project"])[2]
        frameworks_generated.append((entry["slug"], entry["name"], fmt_time(viv_cold)))

    PAGE_PLACEHOLDERS["guides/frameworks"] = lambda: {
        "framework_list": "\n".join(
            f"- [{name}](/guides/{slug}.html) ({cold} cold)"
            for slug, name, cold in frameworks_generated
        )
    }

    releases = changelog_releases()
    pages_by_section["releases"].extend(releases)
    PAGE_PLACEHOLDERS["releases/index"] = lambda: {
        "changelog": "\n".join(f"- [{p['title']}]({p['url']}) -- {p['summary']}" for p in releases)
    }

    for slug in pages_by_section:
        for page in section_sequence(pages_by_section, slug):
            html = render_doc_page(page, sections, pages_by_section, sections_by_slug, search_index)
            page_dist_path(page).write_text(html)
    for page in framework_pages:
        html = render_doc_page(page, sections, pages_by_section, sections_by_slug, search_index)
        page_dist_path(page).write_text(html)

    # Top-level pages, not part of any section's own sequence: index.md
    # reuses the getting-started section's content as the docs entry point
    # (#site-vertical-nav) -- sidebar_current marks that section's own entry
    # current there too, since index_page.url overrides its own /index.html
    # to / and isn't reachable through section_sequence. compare.md and
    # news.md get the same doc layout, minus a section.
    _, index_body = parse_front_matter((SITE / "pages/index.md").read_text())
    index_page = make_page(None, "index", "viv", 0, "", index_body,
                            src_rel="site/pages/getting-started/index.md")
    index_page["url"] = "/"
    index_html = render_doc_page(index_page, sections, pages_by_section, sections_by_slug, search_index,
                                  sidebar_current=("getting-started", "index"))
    page_dist_path(index_page).write_text(index_html)

    _, news_body = parse_front_matter((SITE / "pages/news.md").read_text())
    news_page = make_page(None, "news", "News", 999, "", news_body, src_rel="site/pages/news.md")
    news_html = render_doc_page(news_page, sections, pages_by_section, sections_by_slug, search_index)
    page_dist_path(news_page).write_text(news_html)

    compare_meta, compare_body = parse_front_matter((SITE / "pages/compare.md").read_text())
    compare_page = make_page(None, "compare", "Compare", 999, "", compare_body,
                              src_rel="site/pages/compare.md")
    compare_html = render_doc_page(compare_page, sections, pages_by_section, sections_by_slug, search_index)
    page_dist_path(compare_page).write_text(compare_html)

    for old_name, url in REDIRECTS.items():
        (DIST / old_name).write_text(REDIRECT_SHELL.format(url=url))

    shutil.copytree(SITE / "static", DIST, dirs_exist_ok=True)
    (DIST / "search.json").write_text(json.dumps(search_index))
    (DIST / "CNAME").write_text("vivace.vandragt.com\n")
    (DIST / ".nojekyll").write_text("")

    for html_path in sorted(DIST.rglob("*.html")):
        if "{{" in html_path.read_text():
            rel = html_path.relative_to(DIST)
            print(f"site/build.py: unfilled {{{{placeholder}}}} in {rel}", file=sys.stderr)
            return 1
    print(f"site/build.py: wrote {DIST}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
