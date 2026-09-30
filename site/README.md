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
`/<section>/<page>.html`, appears in the sidebar under that section in
`order`, and gets a breadcrumb, "On this page" list, prev/next and an "Edit
this page on GitHub" link for free. `site/pages/<section>/index.md` is that
section's own landing page. Use `{{help:<cmd>}}` for `viv <cmd> --help`
output, `{{include:<path>}}` for another file's body (its own H1 dropped),
and `{{include:<path>#<Heading>}}` for just one of its sections.

## Layout

One vertical nav: the sidebar (`render_sidebar` in `site/build.py`) opens
with a top group -- Compare, News, GitHub -- above the five sections from
`site/nav.toml`, all sharing the same current-page highlighting. The header
keeps only the `viv` wordmark (linking home) and the search box.

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
