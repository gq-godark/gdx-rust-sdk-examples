#!/usr/bin/env bash
# Build release binaries and package them with the shipping docs into a
# single linux-x86_64 tarball. The vendored SDK under `sdk/` means the
# binaries link statically against `godark` and depend at runtime only on
# system OpenSSL + glibc.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_NAME="${1:-godark-rust-examples-linux-x86_64}"

cd "$REPO_ROOT"

echo "Building release binaries..."
cargo build --release --examples

STAGING_DIR="$(mktemp -d)"
DEST="$STAGING_DIR/$DIST_NAME"
mkdir -p "$DEST"

cp "$REPO_ROOT/target/release/examples/quickstart"           "$DEST/"
cp "$REPO_ROOT/target/release/examples/full_trader_example"  "$DEST/"
cp "$REPO_ROOT/.env.example"                                 "$DEST/"
cp "$REPO_ROOT/README.md"                                    "$DEST/"
cp "$REPO_ROOT/SDK_REFERENCE.md"                             "$DEST/"

ARCHIVE="$REPO_ROOT/${DIST_NAME}.tar.gz"
tar -czf "$ARCHIVE" -C "$STAGING_DIR" "$DIST_NAME"
rm -rf "$STAGING_DIR"

echo "Package created: $ARCHIVE"
echo "Contents:"
tar -tzf "$ARCHIVE" | head -30
