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
import urllib.request
from pathlib import Path

import markdown

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
DIST = SITE / "dist"
FEED = "https://vandragt.com/tag/vivace/feed.json"
GITHUB_BLOB = "https://github.com/svandragt/vivace/blob/main/"

NAV = [("Home", "index.html"), ("Manual", "manual.html"), ("Compare", "compare.html"),
       ("GitHub", "https://github.com/svandragt/vivace")]

# Drives both the sidebar every non-index page renders and manual.html's own
# list; order here is the order readers see.
MANUAL = [
    ("getting-started", "Getting started"),
    ("cheatsheet", "Cheat sheet"),
    ("install", "Install and upgrade"),
    ("commands", "Commands"),
    ("shim", "Using viv as composer"),
    ("migrate", "Migrating from Composer"),
    ("plugins", "Plugins"),
    ("cache", "Cache and offline use"),
    ("compatibility", "Compatibility and scope"),
    ("reference", "Reference"),
    ("troubleshooting", "Troubleshooting"),
    ("compare", "Compare"),
]

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
{nav}
</header>
{layout}
<footer class="site-footer">
<a href="https://github.com/svandragt/vivace">svandragt/vivace</a> on GitHub
</footer>
</body>
</html>
"""


def render_nav():
    return "\n".join(f'<a href="{href}">{label}</a>' for label, href in NAV)


def render_sidebar(current_slug):
    links = []
    for slug, title in MANUAL:
        current = ' class="current"' if slug == current_slug else ""
        links.append(f'<a href="{slug}.html"{current}>{title}</a>')
    return "\n".join(links)


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
    args = [binary, "--help"] if not cmd else [binary, cmd, "--help"]
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


def corpus_cold_times():
    """Return (viv_cold, composer_cold, section_heading) for laravel/laravel's
    Cold column in the latest section of bench/results/corpus.md."""
    text = (ROOT / "bench/results/corpus.md").read_text()
    heading, body = latest_corpus_section(text)
    row_re = re.compile(
        r"^\| laravel/laravel \| \d+ \| (composer|viv) \| (\S+) \|", re.MULTILINE
    )
    times = dict(row_re.findall(body))
    return times["viv"], times["composer"], heading


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


def readme_section(heading, offset=1):
    level, body = section_from_file("README.md", heading)
    return demote_relative(body, offset)


def readme_fenced_block(heading):
    """The first fenced code block inside a README.md section, so a page
    can show just the command without pulling in the surrounding prose."""
    body = readme_section(heading)
    match = re.search(r"```.*?```", body, re.DOTALL)
    if not match:
        raise SystemExit(f"site/build.py: no fenced block in README section '{heading}'")
    return match.group(0)


def doc_section(path, heading, offset=1):
    level, body = section_from_file(path, heading)
    return demote_relative(body, offset)


GENERIC_PLACEHOLDER_RE = re.compile(r"\{\{(help|readme|doc):([^}]*)\}\}")


def preceding_heading_level(text, pos):
    """The level of the last Markdown heading before pos in text, or 1 (the
    page's own `#` title) when nothing precedes it."""
    heads = list(re.finditer(r"^(#{1,6}) ", text[:pos], re.MULTILINE))
    return len(heads[-1].group(1)) if heads else 1


def resolve_generic_placeholder(match):
    kind, arg = match.group(1), match.group(2)
    if kind == "help":
        return viv_help(arg)
    # Demote so the section's own top heading lands one level below whatever
    # page heading precedes this placeholder, not always one level down.
    page_level = preceding_heading_level(match.string, match.start())
    if kind == "readme":
        level, body = section_from_file("README.md", arg)
    else:
        path, _, heading = arg.partition("#")
        level, body = section_from_file(path, heading)
    return demote_relative(body, page_level - level)


def stability_summary():
    """The first paragraph under docs/stability.md's first heading, links
    rewritten the same way as readme_section."""
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


def migrate_placeholders():
    return {
        "shim_from_readme": readme_source_line("Using viv as composer"),
        "shim_section": readme_section("Using viv as composer"),
        "dockerfile_from_readme": readme_source_line("In a Dockerfile"),
        "dockerfile_section": readme_section("In a Dockerfile"),
        "ci_from_readme": readme_source_line("Using viv as composer"),
        "stability_summary": (
            f"{stability_summary()}\n\n"
            f'<span class="source">source: <a href="{GITHUB_BLOB}docs/stability.md">docs/stability.md</a></span>'
        ),
    }


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
        "install_block": readme_fenced_block("Try it"),
        "posts": latest_posts(),
    }


PAGE_PLACEHOLDERS = {
    "index": index_placeholders,
    "compare": compare_placeholders,
    "migrate": migrate_placeholders,
}


def render_page(md_path, placeholders=None):
    text = md_path.read_text()
    if placeholders:
        for key, value in placeholders.items():
            text = text.replace(f"{{{{{key}}}}}", value)
    text = GENERIC_PLACEHOLDER_RE.sub(resolve_generic_placeholder, text)
    title = text.splitlines()[0].lstrip("# ").strip()
    if title.lower() != "viv":
        title = f"{title} - viv"
    body_html = markdown.markdown(text, extensions=["tables", "fenced_code", "toc"])
    return title, wrap_tables(body_html)


def main():
    if DIST.exists():
        shutil.rmtree(DIST)
    DIST.mkdir(parents=True)

    for md_path in sorted((SITE / "pages").glob("*.md")):
        placeholder_fn = PAGE_PLACEHOLDERS.get(md_path.stem)
        placeholders = placeholder_fn() if placeholder_fn else None
        title, content_html = render_page(md_path, placeholders)
        if md_path.stem == "index":
            nav = f"<nav>{render_nav()}</nav>"
            layout = f"<main>\n{content_html}\n</main>"
        else:
            nav = ""
            layout = (
                '<div class="layout">\n'
                f'<aside class="sidebar">\n{render_sidebar(md_path.stem)}\n</aside>\n'
                f"<main>\n{content_html}\n</main>\n"
                "</div>"
            )
        html = SHELL.format(title=title, nav=nav, layout=layout)
        (DIST / f"{md_path.stem}.html").write_text(html)

    shutil.copytree(SITE / "static", DIST, dirs_exist_ok=True)
    (DIST / "CNAME").write_text("vivace.vandragt.com\n")
    (DIST / ".nojekyll").write_text("")

    for html_path in sorted(DIST.glob("*.html")):
        if "{{" in html_path.read_text():
            print(f"site/build.py: unfilled {{{{placeholder}}}} in {html_path.name}", file=sys.stderr)
            return 1
    print(f"site/build.py: wrote {DIST}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
