#!/usr/bin/env bash
# Records the golden corpus from this checkout's v3: corpus/record.sh [filter]
#
# The default build writes every case that needs no feature on; the
# all-features build writes the cases that need one and checks that the rest
# record identically with everything on. Read each new or changed recording
# before committing it: it is what v3 did, and v4 will be held to it.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo run -q -p dataflow-corpus-v3 --bin record -- "$@"
cargo run -q -p dataflow-corpus-v3 --all-features --bin record -- "$@"

engine=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
datalogic=$(awk '/^name = "datalogic-rs"$/{getline; print}' Cargo.lock | sed 's/version = "\(.*\)"/\1/' | head -1)
cat > corpus/RECORDED.json <<JSON
{
  "engine": "dataflow-rs $engine",
  "src_tree": "$(git rev-parse HEAD:src)",
  "datalogic_rs": "$datalogic",
  "rustc": "$(rustc -V)",
  "date": "$(date -u +%Y-%m-%d)"
}
JSON
