#!/usr/bin/env python3
"""The v4 performance gate's runner: the 13 benchmarks of design 06, measured by
CLAUDE.md's interleaved method. Every gate benchmark prints one line,
`GATE {"bench": ..., "metrics": [{"name", "value", "unit", "better"}]}`, and
this reads only that line.

    gate.py build [--v3 [--v3-ref REF]] --out DIR
        Build the root's examples in release (v4) from a clean tree, copy the
        ports V4_BENCHES names (thirteen, every bench, and the self-judged
        `host_call_floor`), target/release/examples/<name>, to DIR/<name>, and
        write DIR/build.json (commit, rustc, date, engine "v4").
        With --v3, build v3 instead, from a git worktree of REF (V3_REF, the
        freeze commit, by default) at target/gate-v3, reused while it is at
        REF: `cargo build --release --examples` there, with its own target,
        and copy the 13 BENCHES from it. build.json then says engine "v3" and
        holds REF's full sha. A v4 directory holds the names a v3 one does,
        and `compare` reads the two alike.

    gate.py baseline --bins DIR --rounds N --out FILE [--only a,b]
        Run the gate binaries DIR holds (or --only's), as `compare` finds
        them, so a `build` directory runs its thirteen ports and
        `host_call_floor`, round-robin for
        N + 1 rounds, discard round 0 as cold, and write every run, each
        metric's mean, median and spread, with the build, the machine and the
        method, to FILE.

    gate.py compare --a DIR --b DIR --rounds N [--only a,b] [--out FILE]
        Run the binaries of both directories round-robin for N + 1 rounds
        (the order within a pair alternates), discard round 0 as cold, and judge
        each metric on its paired per-round ratios (I80, amending I70): in each
        kept round the ratio B/A, inverted for a metric where higher is
        better, so that a ratio above 1 always means B is worse. The noise is
        max(3%, half the inter-quartile range of the ratios). B is WORSE when
        the median ratio is above 1 + noise, better when it is below
        1 - noise, and "not slower" otherwise. A row whose gate is "Faster"
        (FASTER_ROWS: every metric of `loop_scaling`, its `map` and `handler`
        rows alike, and of `fan_out`) reads faster when the median ratio is
        below 1 - noise and NOT FASTER otherwise, but only when A's build.json
        says v3 and B's says v4; between two builds of one engine every row is
        judged better / not slower / WORSE. A bench may judge its metrics
        apart (METRIC_RULES): a metric takes the rule of the first prefix of
        its name listed there, else its bench's, and an "info" metric prints
        its medians and ratio with no verdict and never fails the gate
        (`large_context_handler`'s `with_view` and `per_field` are "faster",
        its `write_only` and `async_handler_benchmark.marginal` info). Every
        other bench, `orion_connector` among them, is "not slower". A metric
        only one side reports is listed as such and not judged. Exit status is
        1 if any verdict is WORSE or NOT FASTER.

        A self-judged bench (SELF_JUDGED) compares v3 and v4 inside its own
        process and has no v3 binary: `build` copies it, and `compare`
        runs it from B only, once per round, and lists its rows in the same
        table. Its judged metric is already a ratio, v4 over v3, so the
        verdict reads that value directly: over the kept rounds, WORSE when
        its median exceeds 1 + noise and better below 1 - noise, the noise
        being max(3%, half the inter-quartile range of the values). Its
        other metrics are info.

Both commands run every binary under `/usr/bin/time -l` and keep, for every
run, the instructions retired and cycles elapsed it prints (macOS). `compare`
reports each benchmark's median per-round ratio of instructions retired, B/A,
beside its verdicts: the work the code does, which neither load nor code
layout moves. It is informational, never a verdict.
"""
import argparse
import datetime
import json
import math
import os
import shutil
import statistics
import subprocess
import sys

# The freeze commit: v3 as the baseline measured it, whose examples `build --v3`
# builds in a worktree (v3's source left `main` in phase 7b).
V3_REF = "2d3d8e9"

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


# The benchmarks v4 ports (`examples/<name>.rs`), in BENCHES order: the
# eight sync ones of phase 4, phase 5's four host-call rows and phase 6's
# `observed_run`, thirteen ports,
# with the self-judged `host_call_floor` beside the row whose floor it gates.
V4_BENCHES = [
    "realistic_benchmark",
    "benchmark",
    "micro_cond_bench",
    "micro_aggregate_bench",
    "micro_subtree_write_bench",
    "micro_multiworkflow_bench",
    "async_handler_benchmark",
    "host_call_floor",
    "map_performance_test",
    "loop_scaling",
    "large_context_handler",
    "fan_out",
    "orion_connector",
    "observed_run",
]

# The rows whose gate is "Faster" rather than "Not slower" (design 06), every
# metric of each: `loop_scaling`'s `handler` rows with its `map` ones (I95),
# and `fan_out`. `orion_connector` is "not slower", as every bench not listed
# here or in METRIC_RULES.
FASTER_ROWS = {"loop_scaling", "fan_out"}

# Per-metric rules, for a bench whose metrics are not all judged alike (I95):
# a metric takes the rule of the first prefix of its name listed under its
# bench, else its bench's. "faster" is FASTER_ROWS' rule for one metric;
# "info" reports the metric without a verdict. A host call's own floor is
# judged by `host_call_floor` (SELF_JUDGED): `large_context_handler`'s
# write-only floor and `async_handler_benchmark.marginal`, a difference of two
# latency means under a spawn burst whose noise ran 29% to 66%, are reported
# without a verdict.
METRIC_RULES = {
    "async_handler_benchmark": {"marginal": "info"},
    "large_context_handler": {"with_view": "faster", "per_field": "faster", "write_only": "info"},
}

# Benches that compare v3 and v4 inside one process (v4 binaries only), with
# the one metric each judges and its rule; their other metrics are info.
# `host_call_floor` runs `async_handler_benchmark`'s two messages on both
# engines on one thread, in interleaved rounds: `call.ratio` is v4 over v3 for
# a message with the host call.
SELF_JUDGED = {
    "host_call_floor": {"call.ratio": "not slower"},
}

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
    if args.v3:
        ref = sh("git", "rev-parse", args.v3_ref, cwd=root)
        tree = os.path.join(root, "target", "gate-v3")
        sh("git", "worktree", "prune", cwd=root)
        head = None
        if os.path.isdir(tree):
            try:
                head = sh("git", "-C", tree, "rev-parse", "HEAD")
            except subprocess.CalledProcessError:
                head = None
        if head != ref:
            if os.path.isdir(tree):
                sh("git", "worktree", "remove", "--force", tree, cwd=root)
            sh("git", "worktree", "add", "--detach", tree, ref, cwd=root)
        subprocess.run(["cargo", "build", "--release", "--examples"], cwd=tree, env=env, check=True)
        names, src, info_root, commit = BENCHES, tree, tree, ref
    else:
        subprocess.run(["cargo", "build", "--release", "--examples"], cwd=root, env=env, check=True)
        names, src, info_root = V4_BENCHES, root, root
        commit = sh("git", "rev-parse", "HEAD", cwd=root)
    os.makedirs(args.out, exist_ok=True)
    for name in names:
        shutil.copy2(
            os.path.join(src, "target", "release", "examples", name),
            os.path.join(args.out, name),
        )
    info = {
        "commit": commit,
        "describe": sh("git", "describe", "--tags", "--always", cwd=info_root),
        "rustc": sh("rustc", "-V"),
        "date": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        "engine": "v3" if args.v3 else "v4",
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
    done = subprocess.run([TIME, "-l", path] if timed else [path], capture_output=True, text=True)
    if done.returncode != 0:
        sys.exit(f"{name} exited {done.returncode}:\n{done.stderr[-2000:]}")
    return gate_line(name, done.stdout), (counters(done.stderr) if timed else {})


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
    available = [n for n in BENCHES + list(SELF_JUDGED) if os.path.exists(os.path.join(args.bins, n))]
    names = args.only.split(",") if args.only else available
    unknown = [n for n in names if n not in BENCHES and n not in SELF_JUDGED]
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
    """The rule `metric` of bench `name` is judged by: "faster", "not slower"
    or "info" (METRIC_RULES, else the bench's)."""
    for prefix, judged in METRIC_RULES.get(name, {}).items():
        if metric.startswith(prefix):
            return judged
    return "faster" if name in FASTER_ROWS else "not slower"


def verdict(faster_row, median_ratio, noise_pct):
    """B against A from the median of the per-round ratios (I80)."""
    noise = noise_pct / 100
    if faster_row:
        return "faster" if median_ratio < 1 - noise else "NOT FASTER"
    if median_ratio > 1 + noise:
        return "WORSE"
    if median_ratio < 1 - noise:
        return "better"
    return "not slower"


def sides(name):
    """The directories a bench runs from: both, or B alone when it is
    self-judged."""
    return ("b",) if name in SELF_JUDGED else ("a", "b")


def self_judged_row(name, metric, values, unit, better):
    """A self-judged bench's row: its judged metric's verdict read off the
    values themselves (ratios, v4 over v3), any other metric info."""
    median = statistics.median(values)
    noise = max(NOISE_FLOOR_PCT, half_iqr_pct(values))
    judged = SELF_JUDGED[name].get(metric)
    row = {"bench": name, "metric": metric, "unit": unit, "better": better,
           "b": {"runs": values, "median": median}, "self_judged": True}
    if unit == "ratio":
        row.update(median_ratio=median, half_iqr_pct=half_iqr_pct(values), noise_pct=noise)
    row["verdict"] = verdict(False, median, noise) if judged == "not slower" else "info"
    return row


def compare(args):
    names = args.only.split(",") if args.only else None
    dirs = {"a": args.a, "b": args.b}
    available = [n for n in BENCHES + list(SELF_JUDGED)
                 if all(os.path.exists(os.path.join(dirs[s], n)) for s in sides(n))]
    if names is None:
        names = available
    unknown = [n for n in names if n not in BENCHES and n not in SELF_JUDGED]
    if unknown:
        sys.exit(f"not gate benchmarks: {unknown}")
    missing = [n for n in names if n not in available]
    if missing:
        sys.exit(f"not built where they run (both directories, or B for a self-judged bench): {missing}")
    builds = {}
    for side, d in (("a", args.a), ("b", args.b)):
        with open(os.path.join(d, "build.json")) as f:
            builds[side] = json.load(f)

    # The "Faster" rule is v3 against v4; between two builds of one engine
    # (a tuning comparison) every row is judged better / not slower / WORSE.
    faster_rules = builds["a"].get("engine") == "v3" and builds["b"].get("engine") == "v4"

    load_before = os.getloadavg()
    # Per bench and side, one entry per kept round, in round order, so the
    # i-th runs of A and B are a pair.
    runs = {name: {"a": [], "b": []} for name in names}
    for round_no in range(args.rounds + 1):
        # Which side goes first alternates, so neither owns the cooler slot.
        order = ("a", "b") if round_no % 2 == 0 else ("b", "a")
        for name in names:
            for side in (s for s in order if s in sides(name)):
                metrics, counted = run_once(dirs[side], name)
                if round_no > 0:
                    runs[name][side].append({"metrics": {m["name"]: m for m in metrics}, "counters": counted})
            label = "cold, discarded" if round_no == 0 else f"round {round_no}/{args.rounds}"
            print(f"{label:>16}  {name}", file=sys.stderr, flush=True)

    rows, work = [], {}
    for name in names:
        if name in SELF_JUDGED:
            for metric in dict.fromkeys(m for r in runs[name]["b"] for m in r["metrics"]):
                found = [r["metrics"][metric] for r in runs[name]["b"] if metric in r["metrics"]]
                rows.append(self_judged_row(name, metric, [m["value"] for m in found],
                                            found[0]["unit"], found[0]["better"]))
            work[name] = {key: {"b": [r["counters"].get(key) for r in runs[name]["b"]]}
                          for key in COUNTERS.values()}
            continue
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
            judged = rule(name, metric)
            if judged == "info":
                said = "info"
            else:
                said = verdict(faster_rules and judged == "faster", median_ratio, noise)
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
        if name in SELF_JUDGED:
            continue
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
                "statistic": "median of the per-round ratios B/A, inverted where higher is better (I80)",
                "noise": f"max({NOISE_FLOOR_PCT:g}%, half the inter-quartile range of the ratios)",
                "rules": {"faster_rows": sorted(FASTER_ROWS), "metric_rules": METRIC_RULES,
                          "self_judged": SELF_JUDGED},
                "counters": f"{TIME} -l, informational",
            },
            "rows": rows,
            "work": work,
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
    b.add_argument("--v3", action="store_true", help="build v3's examples from a worktree of --v3-ref instead of the root's")
    b.add_argument("--v3-ref", default=V3_REF, help="the commit --v3 builds (default: the freeze commit)")
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
