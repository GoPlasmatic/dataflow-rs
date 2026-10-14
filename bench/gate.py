#!/usr/bin/env python3
"""The v4 performance gate's runner: the 13 benchmarks of design 06, measured by
CLAUDE.md's interleaved method. Every gate benchmark prints one line,
`GATE {"bench": ..., "metrics": [{"name", "value", "unit", "better"}]}`, and
this reads only that line.

    gate.py build [--v4] --out DIR
        Build every example in release from a clean tree, copy the 13 gate
        binaries out of target/, and write DIR/build.json (commit, rustc, date).
        With --v4, build `cargo build --release -p dataflow-rs-v4 --examples`
        instead and copy the eight sync ports, target/release/examples/v4_<name>,
        to DIR/<name>: a v4 directory then holds the names a v3 one does, and
        `compare` reads the two alike.

    gate.py baseline --bins DIR --rounds N --out FILE [--only a,b]
        Run the binaries round-robin for N + 1 rounds, discard round 0 as
        cold, and write every run, each metric's mean, median and spread, with
        the build, the machine and the method, to FILE.

    gate.py compare --a DIR --b DIR --rounds N [--only a,b] [--out FILE]
        Run the binaries of both directories round-robin for N + 1 rounds
        (the order within a pair alternates), discard round 0 as cold, and judge
        each metric on medians. B is worse when it is worse than A by more than
        the noise, max(3%, A's spread, B's spread), spread being
        (max - min) / mean over the kept rounds; better when it is better by
        more than the noise; "not slower" otherwise. A row whose gate is
        "Faster" (FASTER_ROWS) reads faster / NOT FASTER instead of
        better / not slower / WORSE. A metric only one side reports is listed
        as such and not judged. Exit status is 1 if any verdict is WORSE or
        NOT FASTER.
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


# The eight sync benchmarks v4 ports in phase 4 (`v4/examples/v4_<name>.rs`),
# in BENCHES order. The other five need host handlers, which v4 gets later.
V4_BENCHES = [
    "realistic_benchmark",
    "benchmark",
    "micro_cond_bench",
    "micro_aggregate_bench",
    "micro_subtree_write_bench",
    "micro_multiworkflow_bench",
    "map_performance_test",
    "loop_scaling",
]

# The rows whose gate is "Faster" rather than "Not slower" (design 06).
FASTER_ROWS = {"loop_scaling"}

# The smallest noise `compare` allows, in percent.
NOISE_FLOOR_PCT = 3.0


def sh(*cmd, cwd=None):
    return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def repo_root():
    return sh("git", "rev-parse", "--show-toplevel")


def build(args):
    root = repo_root()
    if sh("git", "status", "--porcelain", "--untracked-files=no", cwd=root):
        sys.exit("refusing to build: the tree has uncommitted changes")
    if args.v4:
        subprocess.run(["cargo", "build", "--release", "-p", "dataflow-rs-v4", "--examples"], cwd=root, check=True)
        names, prefix = V4_BENCHES, "v4_"
    else:
        subprocess.run(["cargo", "build", "--release", "--examples"], cwd=root, check=True)
        names, prefix = BENCHES, ""
    os.makedirs(args.out, exist_ok=True)
    for name in names:
        shutil.copy2(
            os.path.join(root, "target", "release", "examples", prefix + name),
            os.path.join(args.out, name),
        )
    info = {
        "commit": sh("git", "rev-parse", "HEAD", cwd=root),
        "describe": sh("git", "describe", "--tags", "--always", cwd=root),
        "rustc": sh("rustc", "-V"),
        "date": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "engine": "v4" if args.v4 else "v3",
    }
    with open(os.path.join(args.out, "build.json"), "w") as f:
        json.dump(info, f, indent=2)
        f.write("\n")
    print(f"{len(names)} binaries at {info['commit']} in {args.out}")


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


def spread_pct(values):
    mean = statistics.mean(values)
    return (max(values) - min(values)) / abs(mean) * 100 if mean else 0.0


def verdict(better, faster_row, a_median, b_median, noise_pct):
    """B against A: (verdict, delta %). `better` is the metric's direction."""
    delta = (b_median - a_median) / abs(a_median) * 100 if a_median else 0.0
    gain = -delta if better == "lower" else delta  # positive when B is better
    if faster_row:
        return ("faster" if gain > noise_pct else "NOT FASTER"), delta
    if gain > noise_pct:
        return "better", delta
    if gain < -noise_pct:
        return "WORSE", delta
    return "not slower", delta


def compare(args):
    names = args.only.split(",") if args.only else None
    available = [n for n in BENCHES if all(os.path.exists(os.path.join(d, n)) for d in (args.a, args.b))]
    if names is None:
        names = available
    unknown = [n for n in names if n not in BENCHES]
    if unknown:
        sys.exit(f"not gate benchmarks: {unknown}")
    missing = [n for n in names if n not in available]
    if missing:
        sys.exit(f"not built in both directories: {missing}")
    builds = {}
    for side, d in (("a", args.a), ("b", args.b)):
        with open(os.path.join(d, "build.json")) as f:
            builds[side] = json.load(f)

    load_before = os.getloadavg()
    dirs = {"a": args.a, "b": args.b}
    runs = {name: {"a": [], "b": []} for name in names}
    for round_no in range(args.rounds + 1):
        # Which side goes first alternates, so neither owns the cooler slot.
        order = ("a", "b") if round_no % 2 == 0 else ("b", "a")
        for name in names:
            for side in order:
                metrics = run_once(dirs[side], name)
                if round_no > 0:
                    runs[name][side].append(metrics)
            label = "cold, discarded" if round_no == 0 else f"round {round_no}/{args.rounds}"
            print(f"{label:>16}  {name}", file=sys.stderr, flush=True)

    def series(side_runs):
        out = {}
        for run in side_runs:
            for m in run:
                out.setdefault(m["name"], {"unit": m["unit"], "better": m["better"], "runs": []})["runs"].append(m["value"])
        return out

    rows = []
    for name in names:
        a, b = series(runs[name]["a"]), series(runs[name]["b"])
        for metric in list(a) + [m for m in b if m not in a]:
            row = {"bench": name, "metric": metric}
            ra, rb = a.get(metric), b.get(metric)
            if ra is None or rb is None:
                side, only = ("B", rb) if ra is None else ("A", ra)
                row.update(verdict=f"only in {side}", unit=only["unit"], better=only["better"],
                           **{("a" if ra else "b"): {"runs": only["runs"], "median": statistics.median(only["runs"])}})
                rows.append(row)
                continue
            am, bm = statistics.median(ra["runs"]), statistics.median(rb["runs"])
            sa, sb = spread_pct(ra["runs"]), spread_pct(rb["runs"])
            noise = max(NOISE_FLOOR_PCT, sa, sb)
            v, delta = verdict(ra["better"], name in FASTER_ROWS, am, bm, noise)
            row.update(
                unit=ra["unit"], better=ra["better"], verdict=v, delta_pct=delta, noise_pct=noise,
                a={"runs": ra["runs"], "median": am, "spread_pct": sa},
                b={"runs": rb["runs"], "median": bm, "spread_pct": sb},
            )
            rows.append(row)

    print(f"{'bench':<26} {'metric':<28} {'A median':>13} {'B median':>13} {'delta':>8} {'noise':>7}  verdict")
    for row in rows:
        a, b = row.get("a"), row.get("b")
        cell = lambda side: f"{side['median']:>13.2f}" if side else f"{'-':>13}"
        if "delta_pct" in row:
            tail = f"{row['delta_pct']:>+7.1f}% {row['noise_pct']:>6.1f}%"
        else:
            tail = f"{'-':>8} {'-':>7}"
        print(f"{row['bench']:<26} {row['metric']:<28} {cell(a)} {cell(b)} {tail}  {row['verdict']}")

    if args.out:
        out = {
            "a": {"dir": args.a, "build": builds["a"]},
            "b": {"dir": args.b, "build": builds["b"]},
            "machine": machine(load_before),
            "method": {
                "rounds": args.rounds,
                "discarded_rounds": 1,
                "order": names,
                "statistic": "median",
                "noise": f"max({NOISE_FLOOR_PCT:g}%, spread A, spread B), spread = (max - min) / mean",
            },
            "rows": rows,
        }
        with open(args.out, "w") as f:
            json.dump(out, f, indent=2, ensure_ascii=False)
            f.write("\n")
    if any(r["verdict"] in ("WORSE", "NOT FASTER") for r in rows):
        sys.exit(1)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    b = sub.add_parser("build")
    b.add_argument("--out", required=True)
    b.add_argument("--v4", action="store_true", help="build v4's ports instead of v3's examples")
    m = sub.add_parser("baseline")
    m.add_argument("--bins", required=True)
    m.add_argument("--rounds", type=int, default=5)
    m.add_argument("--out", required=True)
    m.add_argument("--only")
    c = sub.add_parser("compare")
    c.add_argument("--a", required=True, help="the reference build directory (v3)")
    c.add_argument("--b", required=True, help="the build directory judged against it (v4)")
    c.add_argument("--rounds", type=int, default=5)
    c.add_argument("--only")
    c.add_argument("--out")
    args = p.parse_args()
    {"build": build, "baseline": baseline, "compare": compare}[args.command](args)


if __name__ == "__main__":
    main()
