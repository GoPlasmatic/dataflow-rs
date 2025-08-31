#!/bin/bash

echo "=== Dataflow-rs Performance Analysis ==="
echo ""

# 1. Run benchmark
echo "1. Running benchmark..."
cargo run --release --example benchmark > benchmark_output.txt 2>&1

# 2. Analyze results
echo "2. Analyzing results..."
python3 - <<EOF
import json
import sys

with open('benchmark_results.json', 'r') as f:
    data = json.load(f)

# Find latest version results
versions = {}
for key, value in data.items():
    version = key.rsplit('_', 1)[0]
    bench_type = key.rsplit('_', 1)[1]
    if version not in versions:
        versions[version] = {}
    versions[version][bench_type] = value

# Get latest version
latest = sorted(versions.keys())[-1]
print(f"Latest version: {latest}")
print("")

# Analyze performance characteristics
results = versions[latest]
if 'seq' in results and 'x1' in results:
    seq_results = results.get('seq', {}).get('x1', results.get('x1', {}))
    seq_avg = seq_results.get('avg_time_ns', 0)
    
    if seq_avg > 0:
        print("Performance Analysis:")
        print(f"  Sequential baseline: {seq_avg/1000:.2f} μs")
        
        for conc in ['x16', 'x32', 'x64', 'x128']:
            if conc in results:
                con_avg = results[conc]['avg_time_ns']
                speedup = seq_avg / con_avg if con_avg > 0 else 0
                print(f"  Concurrency {conc[1:]}: {con_avg/1000:.2f} μs (speedup: {speedup:.2f}x)")

print("")
print("Bottleneck Indicators:")

# Check for diminishing returns
if 'x32' in results and 'x64' in results:
    seq_results = results.get('seq', {}).get('x1', results.get('x1', {}))
    seq_avg = seq_results.get('avg_time_ns', 0)
    if seq_avg > 0:
        speedup_32 = seq_avg / results['x32']['avg_time_ns']
        speedup_64 = seq_avg / results['x64']['avg_time_ns']
        if speedup_64 < speedup_32 * 1.5:
            print("  ⚠️  Diminishing returns at high concurrency")
            print("     Possible causes: lock contention, resource exhaustion")

# Check P99 latency spikes
for bench_type, data in results.items():
    p99 = data['p99_ns']
    avg = data['avg_time_ns']
    if p99 > avg * 3:
        print(f"  ⚠️  High P99 latency in {bench_type}: {p99/1000:.2f} μs vs avg {avg/1000:.2f} μs")
        print("     Possible causes: GC pauses, OS scheduling, lock contention")

# Performance comparison between versions
print("")
print("Version Comparison:")
sorted_versions = sorted(versions.keys())
if len(sorted_versions) > 1:
    prev = sorted_versions[-2]
    latest = sorted_versions[-1]
    
    print(f"  Comparing {prev} → {latest}")
    
    for bench_type in ['x1', 'x16', 'x32', 'x64', 'x128']:
        if bench_type in versions[prev] and bench_type in versions[latest]:
            prev_avg = versions[prev][bench_type]['avg_time_ns']
            latest_avg = versions[latest][bench_type]['avg_time_ns']
            improvement = ((prev_avg - latest_avg) / prev_avg) * 100
            if improvement > 0:
                print(f"    {bench_type}: {improvement:.1f}% faster")
            else:
                print(f"    {bench_type}: {abs(improvement):.1f}% slower")
EOF

# 3. Generate flamegraph (optional)
echo ""
echo "3. Generate flamegraph? (requires sudo on macOS) [y/N]"
read -r response
if [[ "$response" =~ ^[Yy]$ ]]; then
    echo "Generating flamegraph..."
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sudo cargo flamegraph --release --example benchmark
    else
        cargo flamegraph --release --example benchmark
    fi
    echo "Flamegraph saved to flamegraph.svg"
fi

# 4. Quick system resource check
echo ""
echo "4. System Resource Analysis:"
echo "   CPU cores: $(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo 'unknown')"
echo "   Memory: $(sysctl -n hw.memsize 2>/dev/null | awk '{print $1/1024/1024/1024 " GB"}' || free -h 2>/dev/null | grep Mem | awk '{print $2}' || echo 'unknown')"

echo ""
echo "=== Analysis Complete ==="
echo ""
echo "Next steps:"
echo "  1. Review bottleneck indicators above"
echo "  2. Open flamegraph.svg to identify hot paths"
echo "  3. Run 'cargo run --example benchmark_traced' for detailed tracing"
echo "  4. Use 'tokio-console' for async runtime analysis"