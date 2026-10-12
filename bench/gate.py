#!/usr/bin/env python3
"""The v4 performance gate's runner: the 13 benchmarks of design 06, measured by
CLAUDE.md's interleaved method. Every gate benchmark prints one line,
`GATE {"bench": ..., "metrics": [{"name", "value", "unit", "better"}]}`, and
this reads only that line.

    gate.py build --out DIR
        Build every example in release from a clean tree, copy the 13 gate
        binaries out of target/, and write DIR/build.json (commit, rustc, date).

    gate.py baseline --bins DIR --rounds N --out FILE [--only a,b]
        Run the binaries round-robin for N + 1 rounds, discard round 0 as
        cold, and write every run, each metric's mean, median and spread, with
        the build, the machine and the method, to FILE.

Phase 4 adds `compare`, which interleaves two build directories.
"""
import argparse
import datetime
import json
import os
import shutil
import statistics
import subprocess
import sys

BENCHES = [
    "realistic_benchmark",
    "benchmark",
    "micro_cond_bench",
    "micro_aggregate_bench",
    "micro_subtree_write_bench",
    "micro_multiworkflow_bench",
    "async_handler_benchmark",
    "map_performance_test",
    "loop_scaling",
    "large_context_handler",
    "fan_out",
    "orion_connector",
    "observed_run",
]


def sh(*cmd, cwd=None):
    return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def repo_root():
    return sh("git", "rev-parse", "--show-toplevel")


def build(args):
    root = repo_root()
    if sh("git", "status", "--porcelain", "--untracked-files=no", cwd=root):
        sys.exit("refusing to build: the tree has uncommitted changes")
    subprocess.run(["cargo", "build", "--release", "--examples"], cwd=root, check=True)
    os.makedirs(args.out, exist_ok=True)
    for name in BENCHES:
        shutil.copy2(os.path.join(root, "target", "release", "examples", name), os.path.join(args.out, name))
    info = {
        "commit": sh("git", "rev-parse", "HEAD", cwd=root),
        "describe": sh("git", "describe", "--tags", "--always", cwd=root),
        "rustc": sh("rustc", "-V"),
        "date": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
    }
    with open(os.path.join(args.out, "build.json"), "w") as f:
        json.dump(info, f, indent=2)
        f.write("\n")
    print(f"{len(BENCHES)} binaries at {info['commit']} in {args.out}")


def gate_line(name, stdout):
    for line in reversed(stdout.splitlines()):
        if line.startswith("GATE "):
            gate = json.loads(line[5:])
            if gate["bench"] != name:
                sys.exit(f"{name} printed a GATE line for {gate['bench']}")
            return gate["metrics"]
    sys.exit(f"{name} printed no GATE line")


def run_once(bins, name):
    done = subprocess.run([os.path.join(bins, name)], capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"{name} exited {done.returncode}:\n{done.stderr[-2000:]}")
    return gate_line(name, done.stdout)


def machine(load_before):
    def ctl(key):
        return sh("sysctl", "-n", key)

    power = sh("pmset", "-g", "batt").splitlines()
    return {
        "model": ctl("hw.model"),
        "cpu": ctl("machdep.cpu.brand_string"),
        "cores": int(ctl("hw.ncpu")),
        "memory_gb": round(int(ctl("hw.memsize")) / 2**30),
        "os": "macOS " + sh("sw_vers", "-productVersion"),
        "power": power[0] if power else "unknown",
        "load_avg_before": [round(x, 2) for x in load_before],
    }


def baseline(args):
    names = args.only.split(",") if args.only else BENCHES
    unknown = [n for n in names if n not in BENCHES]
    if unknown:
        sys.exit(f"not gate benchmarks: {unknown}")
    with open(os.path.join(args.bins, "build.json")) as f:
        build_info = json.load(f)

    load_before = os.getloadavg()
    runs = {name: [] for name in names}
    for round_no in range(args.rounds + 1):
        for name in names:
            metrics = run_once(args.bins, name)
            label = "cold, discarded" if round_no == 0 else f"round {round_no}/{args.rounds}"
            print(f"{label:>16}  {name}", file=sys.stderr, flush=True)
            if round_no > 0:
                runs[name].append(metrics)

    benches = {}
    for name in names:
        table = {}
        for metric in runs[name][0]:
            values = [next(m["value"] for m in run if m["name"] == metric["name"]) for run in runs[name]]
            mean = statistics.mean(values)
            table[metric["name"]] = {
                "unit": metric["unit"],
                "better": metric["better"],
                "runs": values,
                "mean": mean,
                "median": statistics.median(values),
                "spread_pct": (max(values) - min(values)) / abs(mean) * 100 if mean else 0.0,
            }
        benches[name] = table

    out = {
        "build": build_info,
        "machine": machine(load_before),
        "method": {"rounds": args.rounds, "discarded_rounds": 1, "order": names},
        "benches": benches,
    }
    with open(args.out, "w") as f:
        json.dump(out, f, indent=2, ensure_ascii=False)
        f.write("\n")
    for name, table in benches.items():
        for metric, row in table.items():
            print(f"{name:<28} {metric:<34} {row['mean']:>14.2f} {row['median']:>14.2f} {row['unit']:<11} ±{row['spread_pct']:.1f}%")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    b = sub.add_parser("build")
    b.add_argument("--out", required=True)
    m = sub.add_parser("baseline")
    m.add_argument("--bins", required=True)
    m.add_argument("--rounds", type=int, default=5)
    m.add_argument("--out", required=True)
    m.add_argument("--only")
    args = p.parse_args()
    {"build": build, "baseline": baseline}[args.command](args)


if __name__ == "__main__":
    main()
