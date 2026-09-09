#!/usr/bin/env python3
"""Compare hyperfine JSON results for `viv` against `composer`, gated on a stored baseline ratio.

Usage:
    bench/compare.py <hyperfine-json>... --baseline bench/results/baseline.json [--tolerance 0.15] [--write-baseline]
    bench/compare.py --self-test

GitHub runners vary 30-45% run to run on identical code (Laravel warm: 64 ms
one run, 93 ms the next, confirmed by a local A/B) so a raw-seconds baseline
chases runner noise, not regressions. Instead each checked scenario is graded
as `ratio = viv_mean / composer_mean` for the same scenario, measured in the
same job on the same runner in the same minute: the runner's speed cancels
out of the ratio even though it doesn't cancel out of either mean alone.
Composer has no `update-offline` scenario, so that ratio uses composer's
`update-warm` mean as the denominator instead -- it only needs to be a stable
same-runner, same-minute number, not the same scenario.

Fails (exit 1) when `viv`'s warm, noop or update-offline ratio regresses past
baseline_ratio * (1 + tolerance) AND the absolute cost of that regression is
at least 5 ms (viv_mean - baseline_ratio * composer_mean) -- differences
under 5 ms are never a failure, even past tolerance, because a fast scenario
(a 4 ms noop) can swing well past a relative tolerance on a runner-clock
wobble too small to matter in real seconds. Cold and update-warm are
informational only: both wait on the network (downloads, and 304
revalidations of every metadata file), so their variance is not ours (see
bench/results/README.md). A scenario whose composer denominator is missing
from the input is skipped, not failed. With --write-baseline, writes the
measured ratios as the new baseline instead of comparing.
"""
import argparse
import json
import sys

TOLERANCE_DEFAULT = 0.15
# A ratio beyond tolerance still passes if what it costs in this run's actual
# seconds is under 5 ms: viv_mean - baseline_ratio * composer_mean, the gap
# between what happened and what the baseline ratio predicted for this run's
# composer_mean. Catches a real regression on a slow scenario without failing
# a fast one (noop, warm) over a ratio wobble that's a few microseconds in
# absolute terms.
ABSOLUTE_SLACK_S = 0.005
SCENARIOS = ("cold", "warm", "noop", "update-warm", "update-offline")
CHECKED_SCENARIOS = ("warm", "noop", "update-offline")
# Which composer scenario stands in the denominator for each viv scenario.
# update-offline has no composer equivalent, so it borrows update-warm (same
# runner, same minute; the denominator only has to be stable, not identical
# in kind).
DENOMINATOR_SCENARIO = {
    "cold": "cold",
    "warm": "warm",
    "noop": "noop",
    "update-warm": "update-warm",
    "update-offline": "update-warm",
}


def means_from_hyperfine(paths, tool):
    """Return {scenario: mean_seconds} for `tool`'s commands across the given hyperfine JSON files."""
    means = {}
    for path in paths:
        with open(path) as f:
            data = json.load(f)
        for result in data["results"]:
            parts = result["command"].split()
            if len(parts) != 2 or parts[0] != tool or parts[1] not in SCENARIOS:
                continue
            means[parts[1]] = result["mean"]
    return means


def compare(viv_means, composer_means, baseline, tolerance):
    """Return (ok, rows) where rows is a list of (scenario, viv, composer, ratio, baseline_ratio, status)."""
    ok = True
    rows = []
    for scenario in SCENARIOS:
        viv_mean = viv_means.get(scenario)
        if viv_mean is None:
            continue
        composer_mean = composer_means.get(DENOMINATOR_SCENARIO[scenario])
        ratio = viv_mean / composer_mean if composer_mean else None
        if scenario not in CHECKED_SCENARIOS:
            rows.append((scenario, viv_mean, composer_mean, ratio, None, "info"))
            continue
        base_ratio = baseline.get(scenario)
        if ratio is None or base_ratio is None:
            rows.append((scenario, viv_mean, composer_mean, ratio, base_ratio, "skip"))
            continue
        limit = base_ratio * (1 + tolerance)
        excess = viv_mean - base_ratio * composer_mean
        if ratio > limit and excess >= ABSOLUTE_SLACK_S:
            ok = False
            rows.append((scenario, viv_mean, composer_mean, ratio, base_ratio, "FAIL"))
        else:
            rows.append((scenario, viv_mean, composer_mean, ratio, base_ratio, "ok"))
    return ok, rows


def print_table(project, rows, tolerance):
    print(f"bench/compare.py: {project} (tolerance {tolerance:.0%})")
    print(f"{'scenario':<15} {'viv':>10} {'composer':>10} {'ratio':>8} {'baseline':>10} {'status':>6}")
    for scenario, viv_mean, composer_mean, ratio, base_ratio, status in rows:
        composer_str = f"{composer_mean:.3f}s" if composer_mean is not None else "-"
        ratio_str = f"{ratio:.3f}" if ratio is not None else "-"
        base_str = f"{base_ratio:.3f}" if base_ratio is not None else "-"
        print(
            f"{scenario:<15} {viv_mean:>9.3f}s {composer_str:>10} {ratio_str:>8} {base_str:>10} {status:>6}"
        )


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("json_paths", nargs="*")
    parser.add_argument("--baseline", default="bench/results/baseline.json")
    parser.add_argument("--project", default="monolog", help="baseline key (default: monolog)")
    parser.add_argument("--tolerance", type=float, default=TOLERANCE_DEFAULT)
    parser.add_argument("--write-baseline", action="store_true")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    if not args.json_paths:
        parser.error("no hyperfine JSON paths given")

    viv_means = means_from_hyperfine(args.json_paths, "viv")
    composer_means = means_from_hyperfine(args.json_paths, "composer")
    if not viv_means:
        print("bench/compare.py: no `viv` results found in input", file=sys.stderr)
        return 1

    with open(args.baseline) as f:
        all_baselines = json.load(f)
    project_baseline = all_baselines.get(args.project, {})

    if args.write_baseline:
        ratios = {}
        for scenario in SCENARIOS:
            viv_mean = viv_means.get(scenario)
            composer_mean = composer_means.get(DENOMINATOR_SCENARIO[scenario])
            ratios[scenario] = viv_mean / composer_mean if viv_mean is not None and composer_mean else None
        all_baselines[args.project] = ratios
        with open(args.baseline, "w") as f:
            json.dump(all_baselines, f, indent=2, sort_keys=True)
            f.write("\n")
        print(f"bench/compare.py: wrote baseline for {args.project} to {args.baseline}")
        return 0

    if not project_baseline:
        print(f"bench/compare.py: no baseline for '{args.project}' yet, skipping regression check")
        return 0

    ok, rows = compare(viv_means, composer_means, project_baseline, args.tolerance)
    print_table(args.project, rows, args.tolerance)
    if not ok:
        print(f"bench/compare.py: regression beyond {args.tolerance:.0%} tolerance", file=sys.stderr)
        return 1
    return 0


def self_test():
    baseline = {"warm": 0.5, "noop": 0.1, "update-offline": 0.4}

    # Pass within tolerance.
    ok, rows = compare(
        {"warm": 0.52, "noop": 0.105, "update-offline": 0.42},
        {"warm": 1.0, "noop": 1.0, "update-warm": 1.0},
        baseline,
        0.15,
    )
    assert ok, "expected pass within tolerance"

    # Fail beyond tolerance.
    ok, rows = compare(
        {"warm": 0.7, "noop": 0.1, "update-offline": 0.4},
        {"warm": 1.0, "noop": 1.0, "update-warm": 1.0},
        baseline,
        0.15,
    )
    assert not ok, "expected fail beyond tolerance"
    statuses = {r[0]: r[5] for r in rows}
    assert statuses["warm"] == "FAIL"

    # A ratio beyond tolerance still passes if the absolute cost this run is
    # under 5 ms slack: noop's composer_mean is 1 ms, so even a doubled ratio
    # is under a millisecond of real regression.
    ok, rows = compare(
        {"noop": 0.001},
        {"noop": 0.001},
        {"noop": 0.1},
        0.15,
    )
    assert ok, "expected absolute slack to absorb a sub-millisecond wobble"
    statuses = {r[0]: r[5] for r in rows}
    assert statuses["noop"] == "ok"

    # The same ratio, scaled up so the absolute excess crosses 5 ms, fails.
    ok, rows = compare(
        {"noop": 0.008},
        {"noop": 0.001},
        {"noop": 0.1},
        0.15,
    )
    assert not ok, "expected a >=5ms excess at the same ratio to fail"
    statuses = {r[0]: r[5] for r in rows}
    assert statuses["noop"] == "FAIL"

    # Skip when the composer denominator is missing.
    ok, rows = compare(
        {"warm": 0.7, "update-offline": 0.4},
        {},
        baseline,
        0.15,
    )
    assert ok, "missing composer denominator must skip, not fail"
    statuses = {r[0]: r[5] for r in rows}
    assert statuses["warm"] == "skip"
    assert statuses["update-offline"] == "skip"

    # A runner twice as slow on both tools passes unchanged: the ratio cancels.
    ok, rows = compare(
        {"warm": 1.0, "noop": 0.2, "update-offline": 0.8},
        {"warm": 2.0, "noop": 2.0, "update-warm": 2.0},
        baseline,
        0.15,
    )
    assert ok, "a runner twice as slow on both tools must not regress the ratio"

    # update-offline borrows composer's update-warm mean as its denominator.
    ok, rows = compare(
        {"update-offline": 0.42},
        {"update-warm": 1.0},
        baseline,
        0.15,
    )
    assert ok
    statuses = {r[0]: r[5] for r in rows}
    assert statuses["update-offline"] == "ok"

    # Cold and update-warm are informational only, never gated.
    ok, rows = compare(
        {"warm": 0.5, "noop": 0.1, "cold": 5.0, "update-warm": 6.0},
        {"warm": 1.0, "noop": 1.0, "cold": 1.0, "update-warm": 1.0},
        baseline,
        0.15,
    )
    statuses = {r[0]: r[5] for r in rows}
    assert statuses["cold"] == "info", "cold must be informational only"
    assert statuses["update-warm"] == "info", "update-warm must be informational only"

    # Missing baseline entry must skip, not fail.
    ok, rows = compare({"warm": 5.0}, {"warm": 1.0}, {}, 0.15)
    assert ok, "missing baseline entries must skip, not fail"
    assert rows[0][5] == "skip"

    print("bench/compare.py: self-test ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
