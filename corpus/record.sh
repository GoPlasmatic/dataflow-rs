#!/usr/bin/env bash
# Records the golden corpus from the published v3 that corpus/v3/Cargo.toml pins
# (crates.io dataflow-rs =3.15.0): corpus/record.sh [filter]
#
# The default build writes every case that needs no feature on; the
# all-features build writes the cases that need one and checks that the rest
# record identically with everything on. Read each new or changed recording
# before committing it: it is what v3 did, and v4 will be held to it.
set -euo pipefail
cd "$(dirname "$0")/.."

cargo run -q -p dataflow-corpus-v3 --bin record -- "$@"
cargo run -q -p dataflow-corpus-v3 --all-features --bin record -- "$@"

version=$(sed -n 's/^dataflow-rs = "=\(.*\)"/\1/p' corpus/v3/Cargo.toml)
checksum=$(awk -v v="$version" '/^\[\[package\]\]$/{n="";ver="";next} /^name = /{n=$3} /^version = /{ver=$3} /^checksum = /{if(n=="\"dataflow-rs\""&&ver=="\""v"\""){gsub(/"/,"",$3);print $3}}' Cargo.lock)
datalogic=$(awk '/^name = "datalogic-rs"$/{getline; print}' Cargo.lock | sed 's/version = "\(.*\)"/\1/' | head -1)
cat > corpus/RECORDED.json <<JSON
{
  "engine": "dataflow-rs $version",
  "source": "crates.io dataflow-rs $version, checksum $checksum",
  "datalogic_rs": "$datalogic",
  "rustc": "$(rustc -V)",
  "date": "$(date -u +%Y-%m-%d)"
}
JSON
