#!/usr/bin/env python3
"""Compare hyperfine JSON results for `viv` against a stored baseline.

Usage:
    bench/compare.py <hyperfine-json>... --baseline bench/results/baseline.json [--tolerance 0.15] [--write-baseline]
    bench/compare.py --self-test

Fails (exit 1) when `viv`'s warm or noop mean regresses past
baseline * (1 + tolerance). Cold and update-warm are informational only: both
wait on the network (downloads, and 304 revalidations of every metadata file),
so their variance is not ours (see bench/results/README.md). With
--write-baseline, writes the measured means as the new baseline instead of
comparing.
"""
import argparse
import json
import sys

TOLERANCE_DEFAULT = 0.15
SCENARIOS = ("cold", "warm", "noop", "update-warm")
CHECKED_SCENARIOS = ("warm", "noop")


def means_from_hyperfine(paths):
    """Return {scenario: mean_seconds} for `viv` commands across the given hyperfine JSON files."""
    means = {}
    for path in paths:
        with open(path) as f:
            data = json.load(f)
        for result in data["results"]:
            command = result["command"]
            parts = command.split()
            if len(parts) != 2 or parts[0] != "viv" or parts[1] not in SCENARIOS:
                continue
            means[parts[1]] = result["mean"]
    return means


def compare(means, baseline, tolerance):
    """Return (ok, rows) where rows is a list of (scenario, current, baseline, status)."""
    ok = True
    rows = []
    for scenario in SCENARIOS:
        current = means.get(scenario)
        if current is None:
            continue
        base = baseline.get(scenario)
        if scenario not in CHECKED_SCENARIOS:
            rows.append((scenario, current, base, "info"))
            continue
        if base is None:
            rows.append((scenario, current, base, "skip"))
            continue
        limit = base * (1 + tolerance)
        if current > limit:
            ok = False
            rows.append((scenario, current, base, "FAIL"))
        else:
            rows.append((scenario, current, base, "ok"))
    return ok, rows


def print_table(project, rows, tolerance):
    print(f"bench/compare.py: {project} (tolerance {tolerance:.0%})")
    print(f"{'scenario':<8} {'current':>10} {'baseline':>10} {'status':>6}")
    for scenario, current, base, status in rows:
        base_str = f"{base:.3f}s" if base is not None else "-"
        print(f"{scenario:<8} {current:>9.3f}s {base_str:>10} {status:>6}")


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

    means = means_from_hyperfine(args.json_paths)
    if not means:
        print("bench/compare.py: no `viv` results found in input", file=sys.stderr)
        return 1

    with open(args.baseline) as f:
        all_baselines = json.load(f)
    project_baseline = all_baselines.get(args.project, {})

    if args.write_baseline:
        all_baselines[args.project] = {
            "warm": means.get("warm"),
            "noop": means.get("noop"),
            "cold": means.get("cold"),
            "update-warm": means.get("update-warm"),
        }
        with open(args.baseline, "w") as f:
            json.dump(all_baselines, f, indent=2, sort_keys=True)
            f.write("\n")
        print(f"bench/compare.py: wrote baseline for {args.project} to {args.baseline}")
        return 0

    if not project_baseline:
        print(f"bench/compare.py: no baseline for '{args.project}' yet, skipping regression check")
        return 0

    ok, rows = compare(means, project_baseline, args.tolerance)
    print_table(args.project, rows, args.tolerance)
    if not ok:
        print(f"bench/compare.py: regression beyond {args.tolerance:.0%} tolerance", file=sys.stderr)
        return 1
    return 0


def self_test():
    baseline = {"warm": 1.0, "noop": 0.1}

    ok, rows = compare({"warm": 1.05, "noop": 0.10}, baseline, 0.15)
    assert ok, "expected pass within tolerance"

    ok, rows = compare({"warm": 1.20, "noop": 0.10}, baseline, 0.15)
    assert not ok, "expected fail beyond tolerance"
    statuses = {r[0]: r[3] for r in rows}
    assert statuses["warm"] == "FAIL"
    assert statuses["noop"] == "ok"

    ok, rows = compare({"warm": 1.0, "noop": 0.5, "cold": 2.0}, baseline, 0.15)
    statuses = {r[0]: r[3] for r in rows}
    assert statuses["cold"] == "info", "cold must be informational only"

    ok, rows = compare({"warm": 5.0}, {}, 0.15)
    assert ok, "missing baseline entries must skip, not fail"
    assert rows[0][3] == "skip"

    print("bench/compare.py: self-test ok")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
