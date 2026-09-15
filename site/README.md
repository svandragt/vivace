# site

Source for vivace.vandragt.com: `site/pages/*.md` render through `site/build.py`
(stdlib plus the `markdown` package) into `site/dist/`, with `site/static/`
copied alongside. Build with `python3 site/build.py`; output goes to
`site/dist/`, gitignored. CI deploys `site/dist/` to GitHub Pages (#253).
