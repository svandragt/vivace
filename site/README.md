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
