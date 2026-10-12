#!/usr/bin/env bash
# Orion's baseline for the v4 gate: its own bench.sh scenarios, one at a time,
# each pass on a fresh server, with the server's peak RSS sampled beside it.
# Phase 1 records it on Orion 1.12.0 over dataflow-rs 3.15; phase 7 runs it
# again on Orion's v4 branch and compares.
#
#   bench/orion.sh ORION_DIR OUT_DIR [PASSES]
#
#   ORION_TAG=v1.12.0     the tag ORION_DIR must be checked out at
#   ORION_CLUSTER=1       also run `cluster` against Orion's HA compose stack
#                         (docker-compose.ha.yml), brought up and down here as
#                         the project dataflow-gate-orion on a free port; its
#                         RSS is not sampled, since it runs in containers
#   ORION_SCENARIOS="a b"  the local scenarios to run (default: all eight)
#   BENCH_DURATION, BENCH_CONCURRENCY   passed through to bench.sh (10s, 50)
#
# Writes OUT_DIR/<scenario>-<pass>/ (bench.sh's log, hey's raw output, the peak
# RSS) and OUT_DIR/orion.json, the summary.
set -euo pipefail

[[ $# -ge 2 ]] || { echo "usage: $0 ORION_DIR OUT_DIR [PASSES]" >&2; exit 2; }
ORION_DIR=$(cd "$1" && pwd)
mkdir -p "$2"
OUT=$(cd "$2" && pwd)
PASSES=${3:-3}
TAG=${ORION_TAG:-v1.12.0}
BENCH="$ORION_DIR/crates/orion-server/tests/benchmark/bench.sh"
read -r -a SCENARIOS <<< "${ORION_SCENARIOS:-baseline simple complex multi concurrency reload plugin model}"

if [[ -n "$(git -C "$ORION_DIR" status --porcelain --untracked-files=no)" ]]; then
    echo "$ORION_DIR has local changes" >&2
    exit 1
fi
if [[ "$(git -C "$ORION_DIR" describe --tags --exact-match 2>/dev/null || true)" != "$TAG" ]]; then
    echo "$ORION_DIR is not at $TAG" >&2
    exit 1
fi

echo "building Orion $TAG (release)" >&2
LOAD_BEFORE=$(sysctl -n vm.loadavg)
export LOAD_BEFORE
cargo build --release --manifest-path "$ORION_DIR/crates/orion-server/Cargo.toml" >&2

# The largest RSS, in KiB, of any process running the server binary, sampled
# every 200 ms until the stop file appears. bench.sh starts it through
# `crates/orion-server/../../target/...`, so the match is on this checkout and
# the binary's tail, not on the normalised path.
sample_rss() {
    local stop=$1 out=$2 max=0 pid rss
    while [[ ! -e "$stop" ]]; do
        for pid in $(pgrep -f "^$ORION_DIR/.*target/release/orion-server --config" || true); do
            rss=$(ps -o rss= -p "$pid" 2>/dev/null | tr -d ' ' || true)
            if [[ -n "$rss" && "$rss" -gt "$max" ]]; then
                max=$rss
            fi
        done
        sleep 0.2
    done
    echo "$max" > "$out"
}

run_pass() {
    local scenario=$1 pass=$2 sample=$3
    local dir="$OUT/$scenario-$pass"
    rm -rf "$dir"
    mkdir -p "$dir"
    echo "  $scenario, pass $pass/$PASSES" >&2
    local sampler=""
    if [[ "$sample" == 1 ]]; then
        sample_rss "$dir/.stop" "$dir/peak_rss_kib" &
        sampler=$!
    fi
    local status=0
    BENCH_RELEASE=1 BENCH_SKIP_BUILD=1 NO_COLOR=1 BENCH_OUTPUT_DIR="$dir" \
        bash "$BENCH" "$scenario" > "$dir/bench.log" 2>&1 || status=$?
    if [[ -n "$sampler" ]]; then
        touch "$dir/.stop"
        wait "$sampler"
        rm -f "$dir/.stop"
    fi
    echo "$status" > "$dir/exit_status"
}

for scenario in "${SCENARIOS[@]}"; do
    for pass in $(seq 1 "$PASSES"); do
        run_pass "$scenario" "$pass" 1
    done
done

# The stack runs as its own compose project with its load balancer on a free
# loopback port, never the file's 8080: `cluster` clears every workflow at the
# URL it is given, and 8080 is a common port for something else to hold.
if [[ "${ORION_CLUSTER:-}" == 1 ]]; then
    ORION_ADMIN_API_KEYS="$(openssl rand -hex 32)"
    export ORION_ADMIN_API_KEYS
    LB_PORT=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1",0)); print(s.getsockname()[1]); s.close()')
    export BENCH_CLUSTER_URL="http://127.0.0.1:$LB_PORT"
    override="$OUT/cluster.override.yml"
    printf 'services:\n  lb:\n    ports: !override\n      - "127.0.0.1:%s:8080"\n' "$LB_PORT" > "$override"
    compose=(docker compose -p dataflow-gate-orion --project-directory "$ORION_DIR"
             -f "$ORION_DIR/docker-compose.ha.yml" -f "$override")
    not_run=""
    if curl -s -o /dev/null --max-time 2 "$BENCH_CLUSTER_URL"; then
        not_run="something already answered at $BENCH_CLUSTER_URL"
    else
        echo "bringing up Orion's HA stack at $BENCH_CLUSTER_URL" >&2
        if "${compose[@]}" up -d --wait >&2; then
            for pass in $(seq 1 "$PASSES"); do
                run_pass cluster "$pass" 0
            done
        else
            not_run="the HA stack did not come up"
        fi
        "${compose[@]}" down -v >&2 || true
    fi
    if [[ -n "$not_run" ]]; then
        echo "cluster not run: $not_run" >&2
        mkdir -p "$OUT/cluster-1"
        echo "$not_run" > "$OUT/cluster-1/bench.log"
        echo 1 > "$OUT/cluster-1/exit_status"
    fi
fi

python3 - "$ORION_DIR" "$OUT" "$TAG" "$PASSES" <<'PY'
import json, os, re, statistics, subprocess, sys

orion, out, tag, passes = sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4])
ROW = re.compile(r"^\s*(.+?)\s+([\d.]+) req/s\s+avg=([\d.]+) ms\s+p99=([\d.]+) ms\s+errors=(\d+)", re.M)

def sh(*cmd, cwd=None):
    return subprocess.run(cmd, cwd=cwd, capture_output=True, text=True).stdout.strip()

def lock_version(name):
    text = open(os.path.join(orion, "Cargo.lock")).read()
    m = re.search(r'name = "%s"\nversion = "([^"]+)"' % re.escape(name), text)
    return m.group(1) if m else None

scenarios = {}
for entry in sorted(os.listdir(out)):
    path = os.path.join(out, entry)
    if not os.path.isdir(path) or "-" not in entry:
        continue
    scenario, pass_no = entry.rsplit("-", 1)
    log = open(os.path.join(path, "bench.log")).read()
    status = int(open(os.path.join(path, "exit_status")).read().strip() or 1)
    s = scenarios.setdefault(scenario, {"rows": {}, "peak_rss_kib": [], "exit_status": []})
    s["exit_status"].append(status)
    rss_file = os.path.join(path, "peak_rss_kib")
    if os.path.exists(rss_file):
        rss = int(open(rss_file).read().strip() or 0)
        if rss > 0:
            s["peak_rss_kib"].append(rss)
        else:
            s["rss_not_sampled"] = s.get("rss_not_sampled", 0) + 1
    for m in ROW.finditer(log):
        row = s["rows"].setdefault(m.group(1).strip(), {"runs": []})
        row["runs"].append({"rps": float(m.group(2)), "avg_ms": float(m.group(3)),
                            "p99_ms": float(m.group(4)), "errors": int(m.group(5))})

for s in scenarios.values():
    for row in s["rows"].values():
        runs = row["runs"]
        row["mean_rps"] = statistics.mean(r["rps"] for r in runs)
        row["mean_avg_ms"] = statistics.mean(r["avg_ms"] for r in runs)
        row["mean_p99_ms"] = statistics.mean(r["p99_ms"] for r in runs)
        row["errors"] = sum(r["errors"] for r in runs)
    if s["peak_rss_kib"]:
        s["peak_rss_mib_max"] = round(max(s["peak_rss_kib"]) / 1024, 1)

def ctl(key):
    return sh("sysctl", "-n", key)

summary = {
    "orion": {"tag": tag, "commit": sh("git", "rev-parse", "HEAD", cwd=orion),
              "dataflow_rs": lock_version("dataflow-rs"), "datalogic_rs": lock_version("datalogic-rs")},
    "machine": {"model": ctl("hw.model"), "cpu": ctl("machdep.cpu.brand_string"), "cores": int(ctl("hw.ncpu")),
                "memory_gb": round(int(ctl("hw.memsize")) / 2**30), "os": "macOS " + sh("sw_vers", "-productVersion"),
                "load_avg_before": [float(x) for x in os.environ["LOAD_BEFORE"].strip("{} ").split()]},
    "rustc": sh("rustc", "-V"),
    "method": {"passes": passes, "duration": os.environ.get("BENCH_DURATION", "10s"),
               "concurrency": int(os.environ.get("BENCH_CONCURRENCY", "50")),
               "rss": "max over 200 ms samples of the local server process; not sampled for cluster"},
    "scenarios": scenarios,
}
with open(os.path.join(out, "orion.json"), "w") as f:
    json.dump(summary, f, indent=2)
    f.write("\n")
for name, s in scenarios.items():
    for row, r in s["rows"].items():
        print(f"{name:<12} {row:<44} {r['mean_rps']:>10.1f} req/s  avg {r['mean_avg_ms']:.2f} ms  p99 {r['mean_p99_ms']:.2f} ms  errors {r['errors']}")
    if "peak_rss_mib_max" in s:
        print(f"{name:<12} peak RSS {s['peak_rss_mib_max']} MiB")
    if any(s["exit_status"]):
        print(f"{name:<12} exit statuses {s['exit_status']}")
PY
