#!/usr/bin/env python3
"""Judge an Orion bench run (bench/orion.sh's orion.json) against a baseline.

    orion_compare.py --a A1.json [A2.json ...] --b B1.json [B2.json ...]
                     [--floor 0.95] [--rss-mib 6.25] [--out FILE]

A row's ratio is B's mean rps over A's. With several files on each side they
are paired by position (round i of A against round i of B) and the ratio is the
median of the per-round ratios. Row "A" (the health check, no engine) is the
machine's control and is printed first: when it reads under --floor the session
is slower than the baseline's, so no row can be judged and every row goes to an
A/B run in one session. Otherwise a row reads "not slower" at --floor or above
and WORSE under it. A row with errors in any B run (or a pass that exited
non-zero) reads "errors". Peak RSS (local scenarios only; cluster runs in
containers and has none) is B's largest over A's largest, and a delta above
--rss-mib is a miss. A scenario present in A but absent from B (cluster in 7c)
is reported as "not run". Exits 1 on any WORSE, errors, control-slow or RSS
miss. Standard library only.
"""
import argparse
import json
import statistics
import sys


def load(paths):
    out = []
    for p in paths:
        with open(p) as f:
            out.append(json.load(f))
    return out


def rows(doc):
    """{(scenario, row): row_dict} for one summary."""
    return {(sn, rn): r for sn, s in doc.get("scenarios", {}).items() for rn, r in s["rows"].items()}


def is_control(row):
    return row.split(":")[0].strip() == "A"


def rss_max(docs, scenario):
    vals = [d["scenarios"][scenario]["peak_rss_mib_max"] for d in docs
            if scenario in d.get("scenarios", {}) and "peak_rss_mib_max" in d["scenarios"][scenario]]
    return max(vals) if vals else None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--a", nargs="+", required=True, help="baseline orion.json file(s)")
    ap.add_argument("--b", nargs="+", required=True, help="candidate orion.json file(s)")
    ap.add_argument("--floor", type=float, default=0.95)
    ap.add_argument("--rss-mib", type=float, default=6.25)
    ap.add_argument("--out", help="also write the verdicts as JSON")
    args = ap.parse_args()
    if len(args.a) != len(args.b) and 1 not in (len(args.a), len(args.b)):
        ap.error("--a and --b take the same number of files (or one side takes one)")
    A, B = load(args.a), load(args.b)
    n = max(len(A), len(B))
    pairs = [(A[i % len(A)], B[i % len(B)]) for i in range(n)]

    keys = []
    for d in A + B:
        for k in rows(d):
            if k not in keys:
                keys.append(k)
    keys = sorted(keys, key=lambda k: not is_control(k[1]))  # stable: control first

    results = []
    for sn, rn in keys:
        ra = [rows(a).get((sn, rn)) for a, _ in pairs]
        rb = [rows(b).get((sn, rn)) for _, b in pairs]
        res = {"scenario": sn, "row": rn}
        if any(r is None for r in rb) or any(r is None for r in ra):
            res.update(verdict="not run", ratio=None, errors=None)
            results.append(res)
            continue
        ratios = [y["mean_rps"] / x["mean_rps"] for x, y in zip(ra, rb)]
        res["ratio"] = statistics.median(ratios)
        res["errors"] = sum(r["errors"] for r in rb)
        bad_exit = any(any(d["scenarios"][sn]["exit_status"]) for _, d in pairs)
        res["bad_exit"] = bad_exit
        results.append(res)

    control = [r for r in results if is_control(r["row"]) and r["ratio"] is not None]
    slow_session = any(r["ratio"] < args.floor for r in control)
    failed = False
    for r in results:
        if r.get("verdict") == "not run":
            continue
        if r["errors"] or r["bad_exit"]:
            r["verdict"] = "errors"
        elif slow_session:
            r["verdict"] = "A/B needed"
        elif r["ratio"] >= args.floor:
            r["verdict"] = "not slower"
        else:
            r["verdict"] = "WORSE"
        failed |= r["verdict"] in ("errors", "WORSE", "A/B needed")

    print(f"{'scenario':<12} {'row':<44} {'ratio':>6} {'errors':>6}  verdict")
    for r in results:
        if r["ratio"] is None:
            print(f"{r['scenario']:<12} {r['row']:<44} {'-':>6} {'-':>6}  not run")
            continue
        flag = "  (control)" if is_control(r["row"]) else ""
        print(f"{r['scenario']:<12} {r['row']:<44} {r['ratio']:>6.3f} {r['errors']:>6}  {r['verdict']}{flag}")
    if slow_session:
        print(f"\ncontrol row under {args.floor}: the session is slower than the baseline's; "
              "re-measure every row A/B in one session")

    print(f"\n{'scenario':<12} {'A MiB':>8} {'B MiB':>8} {'delta':>8}  rss (limit +{args.rss_mib} MiB)")
    rss = []
    for sn in dict.fromkeys(k[0] for k in keys):
        a, b = rss_max(A, sn), rss_max(B, sn)
        if a is None and b is None:
            print(f"{sn:<12} {'-':>8} {'-':>8} {'-':>8}  not sampled")
            continue
        if a is None or b is None:
            print(f"{sn:<12} {a or '-':>8} {b or '-':>8} {'-':>8}  not run" if b is None else
                  f"{sn:<12} {'-':>8} {b:>8} {'-':>8}  no baseline")
            continue
        d = b - a
        ok = d <= args.rss_mib
        failed |= not ok
        rss.append({"scenario": sn, "a_mib": a, "b_mib": b, "delta_mib": round(d, 1), "ok": ok})
        print(f"{sn:<12} {a:>8.1f} {b:>8.1f} {d:>+8.1f}  {'ok' if ok else 'OVER'}")

    if args.out:
        with open(args.out, "w") as f:
            json.dump({"floor": args.floor, "rss_mib": args.rss_mib, "slow_session": slow_session,
                       "rows": results, "rss": rss}, f, indent=2)
            f.write("\n")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
