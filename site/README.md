# site

Source for vivace.vandragt.com: `site/pages/*.md` render through `site/build.py`
(stdlib plus the `markdown` package) into `site/dist/`, with `site/static/`
copied alongside. Build with `python3 site/build.py`; check the result with
`python3 site/check.py`. Output goes to `site/dist/`, gitignored. CI runs
both and deploys `site/dist/` to GitHub Pages (#253).

## Adding a page

Add `site/pages/<section>/<page>.md` (section is one of `getting-started`,
`guides`, `reference`, `architecture`, `releases` -- `site/nav.toml`), with
front matter (`title` required, `order` an int, default 999, and an optional
one-line `summary` shown in the sidebar tooltip). It builds to
`/<section>/<page>.html`, appears in the sidebar under that section, and
gets a breadcrumb, "On this page" list, prev/next and an "Edit this page on
GitHub" link for free. `site/pages/<section>/index.md` is that section's own
landing page and always sorts first in the sidebar. Use `{{help:<cmd>}}` for
`viv <cmd> --help` output, `{{include:<path>}}` for another file's body (its
own H1 dropped), and `{{include:<path>#<Heading>}}` for just one of its
sections.

Each section in `site/nav.toml` sets `sort`: `"order"` sorts that section's
other pages by front-matter `order` then slug (getting-started, releases);
`"alpha"` sorts them by title, case-insensitive and natural, with a leading
`"viv <cmd>"` stripped first so reference pages titled "viv <cmd>" sort by
`<cmd>` (guides, reference, architecture). See `section_sequence` in
`site/build.py`.

## Layout

One vertical nav: the sidebar (`render_sidebar` in `site/build.py`) holds
only the five sections from `site/nav.toml`, all sharing the same
current-page highlighting. The header holds the `viv` wordmark (linking
home), a small nav for News and GitHub (`render_header_nav`, same
current-page highlighting), and the search box.

`/` (`site/pages/index.md`) is the docs entry point: it reuses
`getting-started/index.md`'s body (`site_page_body`, front matter and H1
dropped) followed by a "Documentation" block generated from each section's
own front matter (`documentation_links`), so that list can't drift from the
sidebar it mirrors. `getting-started/index.md` stays the section's own
landing page at `/getting-started/` too -- same source, so the two can't
diverge -- which is why its own links to sibling pages are root-relative
(`/getting-started/install.html`, not `install.html`): the same body renders
at two different URL depths. The sidebar's own entry for it always points at
`/`, not `/getting-started/`.
