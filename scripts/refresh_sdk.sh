#!/usr/bin/env bash
# Refresh the vendored SDK source under `sdk/` from a sibling `gdx-rust-sdk`
# checkout. Pre-generated protobuf bindings under `sdk/src/generated/` are
# included so the distribution does not require `protoc` or `prost-build`.
#
# Usage:
#   ./scripts/refresh_sdk.sh /path/to/gdx-rust-sdk
#
# The source checkout MUST have already been built at least once (so that
# `src/generated/*.rs` is up to date). Run `cargo build` inside the source
# checkout if in doubt.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 /path/to/gdx-rust-sdk" >&2
  exit 1
fi

SRC="$1"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEST="$REPO_ROOT/sdk"

if [[ ! -d "$SRC" ]]; then
  echo "error: source directory '$SRC' does not exist" >&2
  exit 1
fi

if [[ ! -f "$SRC/src/generated/mod.rs" ]]; then
  echo "error: '$SRC/src/generated/mod.rs' missing - build the SDK source first" >&2
  echo "       (cd '$SRC' && cargo build)" >&2
  exit 1
fi

echo "Refreshing $DEST from $SRC ..."
rm -rf "$DEST"
mkdir -p "$DEST"

# Copy SDK source. We deliberately drop:
#   - target/, .git/, .github/, Cargo.lock      (build / VCS artifacts)
#   - examples/, tests/, scripts/, deny.toml    (SDK-internal tooling)
#   - CHANGELOG.md, .gitignore, .gitkeep        (not relevant to consumers)
#   - build.rs, gdx-proto/                      (consumer uses pre-generated bindings)
rsync -a \
  --exclude='target/' \
  --exclude='.git/' \
  --exclude='.github/' \
  --exclude='examples/' \
  --exclude='tests/' \
  --exclude='scripts/' \
  --exclude='Cargo.lock' \
  --exclude='CHANGELOG.md' \
  --exclude='deny.toml' \
  --exclude='.gitignore' \
  --exclude='.gitkeep' \
  --exclude='gdx-proto' \
  --exclude='build.rs' \
  "$SRC/" "$DEST/"

# Strip the SDK's own [[example]] / [dev-dependencies] / [build-dependencies]
# blocks from the vendored Cargo.toml and add the autoexamples/autotests/
# autobenches = false trio under [package]. This matches the layout shipped
# in this repo.
python3 - "$DEST/Cargo.toml" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
text = p.read_text()
text = re.sub(r"\n\[build-dependencies\][\s\S]*?(?=\n\[|$)", "", text)
text = re.sub(r"\n\[dev-dependencies\][\s\S]*?(?=\n\[|$)", "", text)
text = re.sub(r"\n\[\[example\]\][\s\S]*?(?=\n\[|$)", "", text)
text = text.rstrip() + "\n"
inject = (
    "# Pre-generated protobuf bindings are committed under `src/generated/`, so\n"
    "# this vendored copy of `godark` does NOT regenerate them at build time -\n"
    "# consumers do not need `protoc` or `prost-build`. Ship only the library\n"
    "# target; the SDK's own examples and integration tests are excluded from\n"
    "# this distribution.\n"
    "autoexamples = false\n"
    "autotests = false\n"
    "autobenches = false\n"
)
text = re.sub(r"(\[package\][\s\S]*?)(\n\[)", lambda m: m.group(1).rstrip() + "\n" + inject + m.group(2), text, count=1)
p.write_text(text)
PY

echo "Vendored size: $(du -sh "$DEST" | cut -f1)"
echo "Done. Review with: cd '$REPO_ROOT' && git status sdk/"
