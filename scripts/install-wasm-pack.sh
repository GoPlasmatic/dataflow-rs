#!/usr/bin/env bash
#
# Installs the pinned wasm-pack prebuilt release binary into ~/.cargo/bin so
# CI, docs.yml and release.yml all run the exact same wasm-pack version
# without compiling it from source or tracking an unpinned `| sh` installer.
set -euo pipefail

VERSION="v0.14.0"
ARCH="$(uname -m)"
OS="$(uname -s)"
case "${OS}-${ARCH}" in
  Linux-x86_64)  TARGET="x86_64-unknown-linux-musl" ;;
  Linux-aarch64) TARGET="aarch64-unknown-linux-musl" ;;
  Darwin-x86_64) TARGET="x86_64-apple-darwin" ;;
  Darwin-arm64)  TARGET="aarch64-apple-darwin" ;;
  *)
    echo "::error::unsupported platform for prebuilt wasm-pack: ${OS}-${ARCH}"
    exit 1
    ;;
esac

ARCHIVE="wasm-pack-${VERSION}-${TARGET}"
URL="https://github.com/drager/wasm-pack/releases/download/${VERSION}/${ARCHIVE}.tar.gz"
DEST="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$DEST"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
curl -fsSL "$URL" | tar -xz -C "$TMP"
install -m 0755 "$TMP/$ARCHIVE/wasm-pack" "$DEST/wasm-pack"
"$DEST/wasm-pack" --version
