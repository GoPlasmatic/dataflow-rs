#!/usr/bin/env python3
"""The performance gate's runner: an A/B comparison of two builds of the
engine (a base commit against a candidate) over the 13 gate benchmarks,
measured by CLAUDE.md's interleaved method. Every gate benchmark prints one
line, `GATE {"bench": ..., "metrics": [{"name", "value", "unit", "better"}]}`,
and this reads only that line.

    gate.py build --out DIR
        Build the root's examples in release from a clean tree at HEAD, copy
        the 13 BENCHES, target/release/examples/<name>, to DIR/<name>, and
        write DIR/build.json (commit, describe, rustc, date). To build the
        base, check its commit out and run `build` again into another
        directory. The build passes cargo `--target-dir <root>/target` and
        copies from there, so neither the environment nor a cargo config can
        move the binaries away from where they are copied from.

    gate.py baseline --bins DIR --rounds N --out FILE [--only a,b]
        Run the gate binaries DIR holds (or --only's), round-robin for N + 1
        rounds, discard round 0 as cold, and write every run, each metric's
        mean, median and spread, with the build, the machine and the method,
        to FILE.

    gate.py compare --a DIR --b DIR --rounds N [--only a,b] [--out FILE]
        Run the binaries of both directories round-robin for N + 1 rounds
        (the order within a pair alternates), discard round 0 as cold, and judge
        each metric on its paired per-round ratios: in each kept round the
        ratio B/A, inverted for a metric where higher is better, so that a
        ratio above 1 always means B is worse. The noise is max(3%, half the
        inter-quartile range of the ratios). B is WORSE when the median ratio
        is above 1 + noise, better when it is below 1 - noise, and
        "not slower" otherwise. A bench may judge its metrics apart
        (METRIC_RULES): a metric takes the rule of the first prefix of its
        name listed there, else "not slower", and an "info" metric prints its
        medians and ratio with no verdict and never fails the gate
        (`large_context_handler.write_only` and
        `async_handler_benchmark.marginal`). A metric only one side reports is
        listed as such and not judged. Exit status is 1 if any verdict is
        WORSE.

    gate.py smoke --bins DIR [--timeout SECONDS]
        Run every example (`examples/*.rs`) built in DIR once, with
        DATAFLOW_BENCH_SMOKE=1: each benchmark runs its startup check in full,
        a few iterations, and prints its GATE line marked `"smoke": true`. A
        gate benchmark must exit 0 and print exactly that, with its own name
        and at least one metric; any other example must exit 0 and print no
        GATE line, so a benchmark missing from BENCHES fails too. CI runs it
        over the debug build: it checks the workloads, never the numbers.

A smoke number is never a measurement: `baseline` and `compare` refuse to
start with DATAFLOW_BENCH_SMOKE set, run every binary without it, and refuse a
GATE line that says `"smoke"`.

`baseline` and `compare` run every binary under `/usr/bin/time -l` and keep,
for every run, the instructions retired and cycles elapsed it prints (macOS). `compare`
reports each benchmark's median per-round ratio of instructions retired, B/A,
beside its verdicts: the work the code does, which neither load nor code
layout moves. It is informational, never a verdict.
"""
import argparse
import datetime
import json
import math
import os
import platform
import shutil
import statistics
import subprocess
import sys

# The gate benchmarks (`examples/<name>.rs`), in the order they run.
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


# Per-metric rules, for a bench whose metrics are not all judged alike: a
# metric takes the rule of the first prefix of its name listed under its
# bench, else "not slower"; "info" reports the metric without a verdict.
# `large_context_handler`'s write-only floor and
# `async_handler_benchmark.marginal`, a difference of two latency means under
# a spawn burst whose noise ran 29% to 66%, are reported without a verdict.
METRIC_RULES = {
    "async_handler_benchmark": {"marginal": "info"},
    "large_context_handler": {"write_only": "info"},
}

# The variable that runs a benchmark in smoke mode (`examples/shared/gate.rs`).
SMOKE = "DATAFLOW_BENCH_SMOKE"

# The smallest noise `compare` allows, in percent.
NOISE_FLOOR_PCT = 3.0

# What `/usr/bin/time -l` prints about the run's process, by the key a run
# keeps it under. macOS only: elsewhere, or where it prints none, a run has no
# counters.
TIME = "/usr/bin/time"
COUNTERS = {"instructions retired": "instructions", "cycles elapsed": "cycles"}


def sh(*cmd, cwd=None):
    return subprocess.run(cmd, cwd=cwd, check=True, capture_output=True, text=True).stdout.strip()


def repo_root():
    return sh("git", "rev-parse", "--show-toplevel")


def build(args):
    root = repo_root()
    if sh("git", "status", "--porcelain", "--untracked-files=no", cwd=root):
        sys.exit("refusing to build: the tree has uncommitted changes")
    env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
    commit = sh("git", "rev-parse", "HEAD", cwd=root)
    # An explicit target directory: `CARGO_BUILD_TARGET_DIR` or a
    # `build.target-dir` in a cargo config would otherwise move the binaries.
    target = os.path.join(root, "target")
    subprocess.run(
        ["cargo", "build", "--release", "--examples", "--target-dir", target],
        cwd=root,
        env=env,
        check=True,
    )
    os.makedirs(args.out, exist_ok=True)
    for name in BENCHES:
        shutil.copy2(
            os.path.join(target, "release", "examples", name),
            os.path.join(args.out, name),
        )
    info = {
        "commit": commit,
        "describe": sh("git", "describe", "--tags", "--always", cwd=root),
        "rustc": sh("rustc", "-V"),
        "date": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
    }
    with open(os.path.join(args.out, "build.json"), "w") as f:
        json.dump(info, f, indent=2)
        f.write("\n")
    print(f"{len(BENCHES)} binaries at {info['commit']} in {args.out}")


def gate_lines(stdout):
    """Every GATE line's object, in order."""
    return [json.loads(line[5:]) for line in stdout.splitlines() if line.startswith("GATE ")]


def gate_line(name, stdout):
    lines = gate_lines(stdout)
    if not lines:
        sys.exit(f"{name} printed no GATE line")
    gate = lines[-1]
    if gate["bench"] != name:
        sys.exit(f"{name} printed a GATE line for {gate['bench']}")
    if gate.get("smoke"):
        sys.exit(f"{name} printed a smoke GATE line: a smoke run is never a measurement")
    return gate["metrics"]


def without_smoke():
    """The environment, without the smoke switch: a measured run is a full run."""
    return {k: v for k, v in os.environ.items() if k != SMOKE}


def refuse_smoke():
    if SMOKE in os.environ:
        sys.exit(f"refusing to measure with {SMOKE} set: unset it")


def counters(stderr):
    """The counters `/usr/bin/time -l` printed, the last of each: it prints
    them after anything the benchmark wrote to stderr."""
    found = {}
    for line in stderr.splitlines():
        value, _, label = line.strip().partition(" ")
        key = COUNTERS.get(label.strip())
        if key and value.isdigit():
            found[key] = int(value)
    return found


def run_once(bins, name):
    """One run: its GATE metrics and its counters (empty where unavailable)."""
    path = os.path.join(bins, name)
    timed = sys.platform == "darwin" and os.access(TIME, os.X_OK)
    done = subprocess.run(
        [TIME, "-l", path] if timed else [path],
        capture_output=True,
        text=True,
        env=without_smoke(),
    )
    if done.returncode != 0:
        sys.exit(f"{name} exited {done.returncode}:\n{done.stderr[-2000:]}")
    return gate_line(name, done.stdout), (counters(done.stderr) if timed else {})


def machine(load_before):
    if sys.platform == "darwin":
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
    u = platform.uname()
    return {
        "model": u.machine,
        "cpu": u.processor or u.machine,
        "cores": os.cpu_count() or 1,
        "memory_gb": 0,
        "os": f"{u.system} {u.release}",
        "power": "unknown",
        "load_avg_before": [round(x, 2) for x in load_before],
    }


def baseline(args):
    refuse_smoke()
    available = [n for n in BENCHES if os.path.exists(os.path.join(args.bins, n))]
    names = args.only.split(",") if args.only else available
    unknown = [n for n in names if n not in BENCHES]
    if unknown:
        sys.exit(f"not gate benchmarks: {unknown}")
    missing = [n for n in names if n not in available]
    if missing:
        sys.exit(f"not built in {args.bins}: {missing}")
    if not names:
        sys.exit(f"no gate binary in {args.bins}")
    with open(os.path.join(args.bins, "build.json")) as f:
        build_info = json.load(f)

    load_before = os.getloadavg()
    runs = {name: [] for name in names}
    work = {name: [] for name in names}
    for round_no in range(args.rounds + 1):
        for name in names:
            metrics, counted = run_once(args.bins, name)
            label = "cold, discarded" if round_no == 0 else f"round {round_no}/{args.rounds}"
            print(f"{label:>16}  {name}", file=sys.stderr, flush=True)
            if round_no > 0:
                runs[name].append(metrics)
                work[name].append(counted)

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
                "spread_pct": spread_pct(values),
            }
        benches[name] = table

    out = {
        "build": build_info,
        "machine": machine(load_before),
        "method": {"rounds": args.rounds, "discarded_rounds": 1, "order": names, "counters": f"{TIME} -l"},
        "benches": benches,
        "counters": {name: {key: [c.get(key) for c in work[name]] for key in COUNTERS.values()} for name in names},
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


def ratio(better, a, b):
    """One round's B against A, oriented so that above 1 means B is worse:
    B/A where lower is better, A/B where higher is."""
    num, den = (b, a) if better == "lower" else (a, b)
    if den == 0:
        return 1.0 if num == 0 else math.inf
    return num / den


def half_iqr_pct(ratios):
    """Half the inter-quartile range of `ratios`, in percent."""
    if len(ratios) < 2:
        return 0.0
    q1, _, q3 = statistics.quantiles(ratios, n=4, method="inclusive")
    return (q3 - q1) / 2 * 100


def rule(name, metric):
    """The rule `metric` of bench `name` is judged by: "not slower" or
    "info" (METRIC_RULES)."""
    for prefix, judged in METRIC_RULES.get(name, {}).items():
        if metric.startswith(prefix):
            return judged
    return "not slower"


def verdict(median_ratio, noise_pct):
    """B against A from the median of the per-round ratios."""
    noise = noise_pct / 100
    if median_ratio > 1 + noise:
        return "WORSE"
    if median_ratio < 1 - noise:
        return "better"
    return "not slower"


def compare(args):
    refuse_smoke()
    names = args.only.split(",") if args.only else None
    dirs = {"a": args.a, "b": args.b}
    available = [n for n in BENCHES if all(os.path.exists(os.path.join(d, n)) for d in dirs.values())]
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
    # Per bench and side, one entry per kept round, in round order, so the
    # i-th runs of A and B are a pair.
    runs = {name: {"a": [], "b": []} for name in names}
    for round_no in range(args.rounds + 1):
        # Which side goes first alternates, so neither owns the cooler slot.
        order = ("a", "b") if round_no % 2 == 0 else ("b", "a")
        for name in names:
            for side in order:
                metrics, counted = run_once(dirs[side], name)
                if round_no > 0:
                    runs[name][side].append({"metrics": {m["name"]: m for m in metrics}, "counters": counted})
            label = "cold, discarded" if round_no == 0 else f"round {round_no}/{args.rounds}"
            print(f"{label:>16}  {name}", file=sys.stderr, flush=True)

    rows, work = [], {}
    for name in names:
        pairs = list(zip(runs[name]["a"], runs[name]["b"]))
        a_names = [m for r in runs[name]["a"] for m in r["metrics"]]
        b_names = [m for r in runs[name]["b"] for m in r["metrics"]]
        for metric in dict.fromkeys(a_names + b_names):
            row = {"bench": name, "metric": metric}
            in_a, in_b = metric in a_names, metric in b_names
            if not (in_a and in_b):
                side, key = ("A", "a") if in_a else ("B", "b")
                found = [r["metrics"][metric] for r in runs[name][key] if metric in r["metrics"]]
                values = [m["value"] for m in found]
                row.update(verdict=f"only in {side}", unit=found[0]["unit"], better=found[0]["better"],
                           **{key: {"runs": values, "median": statistics.median(values)}})
                rows.append(row)
                continue
            paired = [(ra["metrics"][metric], rb["metrics"][metric]) for ra, rb in pairs
                      if metric in ra["metrics"] and metric in rb["metrics"]]
            ma, mb = paired[0]
            if (ma["better"], ma["unit"]) != (mb["better"], mb["unit"]):
                sys.exit(
                    f"{name} {metric}: A reports {ma['better']}/{ma['unit']} and B "
                    f"{mb['better']}/{mb['unit']}; a metric's name is a contract"
                )
            va, vb = [a["value"] for a, _ in paired], [b["value"] for _, b in paired]
            ratios = [ratio(ma["better"], a, b) for a, b in zip(va, vb)]
            median_ratio = statistics.median(ratios)
            half_iqr = half_iqr_pct(ratios)
            noise = max(NOISE_FLOOR_PCT, half_iqr)
            said = "info" if rule(name, metric) == "info" else verdict(median_ratio, noise)
            row.update(
                unit=ma["unit"], better=ma["better"], verdict=said,
                ratios=ratios, median_ratio=median_ratio, half_iqr_pct=half_iqr, noise_pct=noise,
                a={"runs": va, "median": statistics.median(va)},
                b={"runs": vb, "median": statistics.median(vb)},
            )
            rows.append(row)
        # The work each side did: every counter of every run, and for
        # instructions retired the median per-round ratio B/A.
        work[name] = {
            key: {side: [r["counters"].get(key) for r in runs[name][side]] for side in ("a", "b")}
            for key in COUNTERS.values()
        }
        counted = [(ra["counters"]["instructions"], rb["counters"]["instructions"]) for ra, rb in pairs
                   if "instructions" in ra["counters"] and "instructions" in rb["counters"]]
        if counted:
            ratios = [b / a for a, b in counted]
            work[name]["instructions"].update(ratios=ratios, median_ratio=statistics.median(ratios))

    def cell(v, whole=False):
        return f"{v:>14.0f}" if whole or abs(v) >= 1e9 else f"{v:>14.2f}"

    print(f"{'bench':<26} {'metric':<28} {'A median':>14} {'B median':>14} {'ratio':>7} {'noise':>6}  verdict")
    for name in names:
        for row in (r for r in rows if r["bench"] == name):
            a, b = row.get("a"), row.get("b")
            ca = cell(a["median"]) if a else f"{'-':>14}"
            cb = cell(b["median"]) if b else f"{'-':>14}"
            if "median_ratio" in row:
                tail = f"{row['median_ratio']:>7.3f} {row['noise_pct']:>5.1f}%"
            else:
                tail = f"{'-':>7} {'-':>6}"
            print(f"{row['bench']:<26} {row['metric']:<28} {ca} {cb} {tail}  {row['verdict']}")
        instructions = work[name]["instructions"]
        if "median_ratio" in instructions:
            a = statistics.median(v for v in instructions["a"] if v is not None)
            b = statistics.median(v for v in instructions["b"] if v is not None)
            tail = f"{instructions['median_ratio']:>7.3f} {'-':>6}"
            print(f"{name:<26} {'instructions retired':<28} {cell(a, True)} {cell(b, True)} {tail}  (informational)")

    if args.out:
        out = {
            "a": {"dir": args.a, "build": builds["a"]},
            "b": {"dir": args.b, "build": builds["b"]},
            "machine": machine(load_before),
            "method": {
                "rounds": args.rounds,
                "discarded_rounds": 1,
                "order": names,
                "statistic": "median of the per-round ratios B/A, inverted where higher is better",
                "noise": f"max({NOISE_FLOOR_PCT:g}%, half the inter-quartile range of the ratios)",
                "rules": {"metric_rules": METRIC_RULES},
                "counters": f"{TIME} -l, informational",
            },
            "rows": rows,
            "work": work,
        }
        with open(args.out, "w") as f:
            json.dump(out, f, indent=2, ensure_ascii=False)
            f.write("\n")
    if any(r["verdict"] == "WORSE" for r in rows):
        sys.exit(1)


def examples(root):
    """Every example's name: each `examples/*.rs`."""
    folder = os.path.join(root, "examples")
    return sorted(f[:-3] for f in os.listdir(folder) if f.endswith(".rs"))


def smoke(args):
    env = dict(os.environ, **{SMOKE: "1"})
    failures = []
    names = examples(repo_root())
    for name in names:
        path = os.path.join(args.bins, name)
        if not os.access(path, os.X_OK):
            failures.append(f"{name}: not built in {args.bins}")
            continue
        try:
            done = subprocess.run([path], capture_output=True, text=True, env=env, timeout=args.timeout)
        except subprocess.TimeoutExpired:
            failures.append(f"{name}: still running after {args.timeout}s; does it read smoke mode?")
            continue
        if done.returncode != 0:
            failures.append(f"{name}: exited {done.returncode}:\n{done.stderr[-2000:]}")
            continue
        lines = gate_lines(done.stdout)
        if name not in BENCHES:
            if lines:
                failures.append(f"{name}: printed a GATE line but is not in BENCHES")
            else:
                print(f"ok    {name}")
            continue
        if len(lines) != 1:
            failures.append(f"{name}: printed {len(lines)} GATE lines, not one")
        elif lines[0].get("bench") != name:
            failures.append(f"{name}: its GATE line names {lines[0].get('bench')}")
        elif lines[0].get("smoke") is not True:
            failures.append(f"{name}: its GATE line is not marked smoke; does it read smoke mode?")
        elif not lines[0].get("metrics"):
            failures.append(f"{name}: its GATE line has no metric")
        else:
            print(f"ok    {name} ({len(lines[0]['metrics'])} metrics)")
    missing = [b for b in BENCHES if b not in names]
    if missing:
        failures.append(f"BENCHES names no example: {missing}")
    if failures:
        sys.exit("smoke failures:\n" + "\n".join(failures))
    print(f"{len(names)} examples ran smoke")


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = p.add_subparsers(dest="command", required=True)
    b = sub.add_parser("build")
    b.add_argument("--out", required=True, help="the directory the binaries and build.json go to")
    m = sub.add_parser("baseline")
    m.add_argument("--bins", required=True)
    m.add_argument("--rounds", type=int, default=5)
    m.add_argument("--out", required=True)
    m.add_argument("--only")
    c = sub.add_parser("compare")
    c.add_argument("--a", required=True, help="the reference build directory (the base)")
    c.add_argument("--b", required=True, help="the build directory judged against it (the candidate)")
    c.add_argument("--rounds", type=int, default=5)
    c.add_argument("--only")
    c.add_argument("--out")
    k = sub.add_parser("smoke")
    k.add_argument("--bins", required=True, help="the directory the examples are built in")
    k.add_argument("--timeout", type=int, default=300, help="seconds one example may take")
    args = p.parse_args()
    {"build": build, "baseline": baseline, "compare": compare, "smoke": smoke}[args.command](args)


if __name__ == "__main__":
    main()
