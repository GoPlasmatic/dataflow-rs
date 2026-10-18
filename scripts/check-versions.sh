#!/usr/bin/env bash
#
# The four artifacts are version-locked: the engine (`dataflow-rs`), its
# derive, the wasm npm package and the UI. The engine's version is authored
# once, in `[workspace.package] version`, and every other place must agree
# with it. This runs on every PR (ci.yml) and again at tag time (release.yml),
# where the tag must equal it and the changelog must carry the release's
# section. Before this, agreement was hand-checked at tag time only, with
# inline `grep | head -1 | sed`, so a miss shipped a mismatched release.
#
#   scripts/check-versions.sh            # the workspace version, no tag
#   scripts/check-versions.sh 4.0.0      # check a tag's version (must equal it)
#   scripts/check-versions.sh --print    # print the workspace version only
#
# Exits non-zero, with a GitHub `::error::` annotation per mismatch, on any
# disagreement.
set -euo pipefail
cd "$(dirname "$0")/.."

# The engine's version, read from the `[workspace.package]` section: the one
# place it is authored. A targeted read, so no cargo/network is needed.
workspace_version() {
  awk '
    /^\[workspace\.package\]/ { s = 1; next }
    /^\[/ { s = 0 }
    s && /^version[[:space:]]*=/ {
      sub(/^version[[:space:]]*=[[:space:]]*"/, "")
      sub(/".*/, "")
      print
      exit
    }
  ' Cargo.toml
}

VERSION="$(workspace_version)"
if [ -z "$VERSION" ]; then
  echo "::error::Cargo.toml has no [workspace.package] version"
  exit 1
fi

if [ "${1:-}" = "--print" ]; then
  printf '%s\n' "$VERSION"
  exit 0
fi

TAG="${1:-}"
if [ -n "$TAG" ] && [ "$TAG" != "$VERSION" ]; then
  echo "::error::tag $TAG does not match [workspace.package] version $VERSION"
  exit 1
fi

FAILED=0
check() { # label, actual
  if [ "${2:-}" != "$VERSION" ]; then
    echo "::error::$1 is ${2:-<missing>}, expected $VERSION"
    FAILED=1
  else
    echo "  ok  $1 = $2"
  fi
}

# The internal crates are named through `[workspace.dependencies]`; each
# member's own version is inherited, so the exact pin here is what can drift.
# A requirement carries its operator (`=` for an exact pin); strip it first.
pin() { # crate-name
  grep -E "^$1 = .*version = " Cargo.toml |
    sed -n "s/.*version = \"[=^~]*\([^\"]*\)\".*/\1/p" | head -1
}
check "Cargo.toml dataflow-rs pin" "$(pin dataflow-rs)"
check "Cargo.toml dataflow-rs-derive pin" "$(pin dataflow-rs-derive)"

# Members that inherit the version say so; a literal would be a second source.
for member in derive/Cargo.toml wasm/Cargo.toml; do
  if grep -qE '^version\.workspace *= *true' "$member"; then
    echo "  ok  $member inherits the version"
  else
    check "$member" "$(sed -n 's/^version = "\(.*\)"/\1/p' "$member" | head -1)"
  fi
done

# The npm artifacts keep their versions in JSON; the release stamps them from
# the engine's, so an in-repo value can go stale invisibly.
check "ui/package.json" "$(python3 -c 'import json; print(json.load(open("ui/package.json"))["version"])')"
check "ui/package-lock.json" "$(python3 -c 'import json; print(json.load(open("ui/package-lock.json"))["version"])')"
check "ui/package-lock.json packages.\"\"" \
  "$(python3 -c 'import json; print(json.load(open("ui/package-lock.json"))["packages"][""]["version"])')"

# The docs pin the minor only (`dataflow-rs = "4.0"`), so compare that.
MINOR="${VERSION%.*}"
while IFS= read -r hit; do
  [ -n "$hit" ] || continue
  FOUND=$(printf '%s' "$hit" | grep -oE '"[0-9]+\.[0-9]+' | head -1 | tr -d '"')
  if [ "$FOUND" != "$MINOR" ]; then
    echo "::error::stale version in $hit (expected $MINOR)"
    FAILED=1
  else
    echo "  ok  ${hit%%:*} pins $FOUND"
  fi
done < <(grep -rnE 'dataflow-rs = (\{ *version = )?"[0-9]+\.[0-9]+' \
           README.md docs/src/ 2>/dev/null || true)

# A release's notes are the changelog's section for that version (see
# release.yml's github-release). A `-dev` version is not released.
if [[ "$VERSION" != *-dev ]]; then
  if grep -q "^## \[$VERSION\]" CHANGELOG.md; then
    echo "  ok  CHANGELOG.md has a [$VERSION] section"
  else
    echo "::error::CHANGELOG.md has no '## [$VERSION]' section"
    FAILED=1
  fi
fi

[ "$FAILED" -eq 0 ] || exit 1
echo "All artifact and documentation versions agree on $VERSION"
