#!/usr/bin/env python3
"""Aggregate `bench/corpus.sh` results into per-scenario speedup ranges.

Usage:
    bench/aggregate.py <corpus.md> [--section <heading>]
    bench/aggregate.py --self-test

Parses the latest (or a named) `## <timestamp>` section's table and computes,
per scenario (cold, warm, no-op, update-warm), the per-project ratio of
composer/viv and riff/viv. Reports the geometric mean and the min/max of each
ratio, since these are speedups (ratios), not additive quantities: a project
where composer is 10x slower and one where it's 0.1x slower should average to
1x, not 5x. A project/tool cell that is `n/a` or missing (a known failure,
see bench/skips.txt) is skipped and counted, not treated as a zero.
"""
import argparse
import math
import re
import sys

SCENARIOS = ("Cold", "Warm", "No-op", "Update-warm")
ROW_RE = re.compile(
    r"^\|\s*(\S.*?)\s*\|\s*\d+\s*\|\s*(composer|riff|viv)\s*\|"
    r"\s*(\S+)\s*\|\s*(\S+)\s*\|\s*(\S+)\s*\|\s*(\S+)\s*\|$"
)


def parse_sections(text):
    """Return {heading: [(project, tool, {scenario: value_or_None}), ...]}."""
    sections = {}
    heading = None
    rows = []
    for line in text.splitlines():
        m = re.match(r"^## (\S+)", line)
        if m:
            if heading is not None:
                sections[heading] = rows
            heading = m.group(1)
            rows = []
            continue
        m = ROW_RE.match(line)
        if m and heading is not None:
            project, tool, *values = m.groups()
            scenario_values = {}
            for scenario, value in zip(SCENARIOS, values):
                scenario_values[scenario] = None if value == "n/a" else float(value)
            rows.append((project, tool, scenario_values))
    if heading is not None:
        sections[heading] = rows
    return sections


def latest_section(sections, name=None):
    if name is not None:
        return sections[name]
    return sections[max(sections)]


def by_project(rows):
    """Return {project: {tool: {scenario: value_or_None}}}."""
    projects = {}
    for project, tool, values in rows:
        projects.setdefault(project, {})[tool] = values
    return projects


def aggregate(rows):
    """Return {scenario: {"composer": stats, "riff": stats}} where stats is
    (geomean, min, max, included, skipped) or None if nothing to compare."""
    projects = by_project(rows)
    result = {}
    for scenario in SCENARIOS:
        result[scenario] = {}
        for other in ("composer", "riff"):
            ratios = []
            skipped = 0
            for tools in projects.values():
                viv = tools.get("viv", {}).get(scenario)
                base = tools.get(other, {}).get(scenario)
                if viv is None or base is None:
                    skipped += 1
                    continue
                ratios.append(base / viv)
            if not ratios:
                result[scenario][other] = None
                continue
            geomean = math.exp(sum(math.log(r) for r in ratios) / len(ratios))
            result[scenario][other] = (geomean, min(ratios), max(ratios), len(ratios), skipped)
    return result


def format_cell(stats):
    if stats is None:
        return "n/a", False
    geomean, lo, hi, _included, _skipped = stats
    spans_noise = lo < 1.0 < hi
    # Always a plain ratio: below 1x means viv is slower, said once in the
    # README rather than mixing "slower" prose with a ratio range.
    return f"{geomean:.1f}× ({lo:.1f} to {hi:.1f})", spans_noise


def format_table(result):
    lines = [
        "| Scenario | viv vs Composer | viv vs riff |",
        "|---|---|---|",
    ]
    footnote = False
    for scenario in SCENARIOS:
        composer_cell, composer_noise = format_cell(result[scenario]["composer"])
        riff_cell, riff_noise = format_cell(result[scenario]["riff"])
        if composer_noise or riff_noise:
            footnote = True
        marker = lambda flag: "[^noise]" if flag else ""
        lines.append(
            f"| {scenario} | {composer_cell}{marker(composer_noise)} | {riff_cell}{marker(riff_noise)} |"
        )
    if footnote:
        lines.append("")
        lines.append("[^noise]: within noise on some projects")
    return "\n".join(lines)


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("corpus_md", nargs="?")
    parser.add_argument("--section", help="heading (e.g. a timestamp) to use instead of the latest")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    if not args.corpus_md:
        parser.error("no corpus.md path given")

    with open(args.corpus_md) as f:
        text = f.read()
    sections = parse_sections(text)
    rows = latest_section(sections, args.section)
    result = aggregate(rows)
    print(format_table(result))
    return 0


def self_test():
    text = """
## 2026-01-01T00:00:00Z

| Project | Packages | Tool | Cold | Warm | No-op | Update-warm |
|---|---|---|---|---|---|---|
| a/a | 10 | composer | 2.0 | 2.0 | 1.0 | n/a |
| a/a | 10 | riff | 0.5 | 1.0 | 1.0 | n/a |
| a/a | 10 | viv | 1.0 | 1.0 | 1.0 | 0.5 |
| b/b | 20 | composer | 4.0 | 4.0 | 1.0 | 1.0 |
| b/b | 20 | riff | n/a | n/a | n/a | n/a |
| b/b | 20 | viv | 2.0 | 2.0 | 1.0 | 2.0 |
"""
    sections = parse_sections(text)
    assert list(sections) == ["2026-01-01T00:00:00Z"]
    rows = latest_section(sections)
    assert len(rows) == 6

    result = aggregate(rows)
    cold_composer = result["Cold"]["composer"]
    assert cold_composer[3] == 2 and cold_composer[4] == 0
    assert abs(cold_composer[0] - math.sqrt(2.0 * 2.0)) < 1e-9

    cold_riff = result["Cold"]["riff"]
    # b/b's riff cold is n/a: one project included, one skipped.
    assert cold_riff[3] == 1 and cold_riff[4] == 1
    assert cold_riff[0] == 0.5

    noop = result["No-op"]["composer"]
    assert noop[0] == 1.0 and noop[1] == 1.0 and noop[2] == 1.0

    update_warm_composer = result["Update-warm"]["composer"]
    # a/a's composer update-warm is n/a: one skipped, one included.
    assert update_warm_composer[3] == 1 and update_warm_composer[4] == 1

    cell, noisy = format_cell(cold_riff)
    assert cell.startswith("0."), cell
    assert not noisy

    table = format_table(result)
    assert "| Cold | 2.0× (2.0 to 2.0) |" in table
    assert "within noise" not in table  # no ratio here spans 1.0

    print("bench/aggregate.py: self-test ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
