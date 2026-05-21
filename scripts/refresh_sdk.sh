#!/usr/bin/env bash
# Refresh the vendored SDK source under `sdk/` from a sibling `gdx-rust-sdk`
# checkout AND record the upstream commit in `sdk/UPSTREAM_REF` so the
# release pipeline (scripts/package.sh + .github/workflows/release.yml) can
# verify the vendored copy hasn't drifted from upstream.
#
# Pre-generated protobuf bindings under `src/generated/` are included so the
# distribution does not require `protoc` or `prost-build`.
#
# Usage:
#   ./scripts/refresh_sdk.sh /path/to/gdx-rust-sdk
#
# The source checkout MUST:
#   1. be a git checkout (`.git/` present) so the pin can be recorded
#   2. have a clean worktree (no uncommitted changes); otherwise the recorded
#      SHA wouldn't faithfully describe what was vendored
#   3. have already been built at least once (so `src/generated/*.rs` exists)
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

if [[ ! -d "$SRC/.git" ]]; then
  echo "error: '$SRC' is not a git checkout - pin cannot be recorded" >&2
  exit 1
fi

if [[ ! -f "$SRC/src/generated/mod.rs" ]]; then
  echo "error: '$SRC/src/generated/mod.rs' missing - build the SDK source first" >&2
  echo "       (cd '$SRC' && cargo build)" >&2
  exit 1
fi

# Refuse to refresh from a dirty upstream worktree. The pin would not be
# reproducible and the CI parity check would fail in confusing ways.
if ! git -C "$SRC" diff --quiet || ! git -C "$SRC" diff --cached --quiet; then
  echo "error: upstream '$SRC' has uncommitted changes; commit or stash first" >&2
  exit 1
fi

UPSTREAM_SHA="$(git -C "$SRC" rev-parse HEAD)"
UPSTREAM_TAG="$(git -C "$SRC" describe --tags --exact-match HEAD 2>/dev/null || true)"

echo "Refreshing $DEST from $SRC ..."
echo "  upstream HEAD: $UPSTREAM_SHA${UPSTREAM_TAG:+ (tag $UPSTREAM_TAG)}"

# Preserve UPSTREAM_REF across the wipe; it gets rewritten below.
rm -rf "$DEST"
mkdir -p "$DEST"

# Copy SDK source. Deliberately drop:
#   - target/, .git/, .github/, Cargo.lock      (build / VCS artifacts)
#   - examples/, tests/, scripts/, deny.toml    (SDK-internal tooling)
#   - CHANGELOG.md, .gitignore, .gitkeep        (not relevant to consumers)
#   - build.rs, gdx-proto/                      (consumer uses pre-generated bindings)
#   - src/market_data.rs, src/rest_client.rs,   (REST + gomarket WS surfaces are
#     src/rest_transport.rs                     not used by either MM example)
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
  --exclude='src/market_data.rs' \
  --exclude='src/rest_client.rs' \
  --exclude='src/rest_transport.rs' \
  "$SRC/" "$DEST/"

# Strip the SDK's own [[example]] / [dev-dependencies] / [build-dependencies]
# blocks from the vendored Cargo.toml, drop the `reqwest` line (only the
# trimmed REST modules used it), and add the autoexamples/autotests/
# autobenches = false trio under [package]. This matches the layout shipped
# in this repo.
python3 - "$DEST/Cargo.toml" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
text = p.read_text()
text = re.sub(r"\n\[build-dependencies\][\s\S]*?(?=\n\[|$)", "", text)
text = re.sub(r"\n\[dev-dependencies\][\s\S]*?(?=\n\[|$)", "", text)
text = re.sub(r"\n\[\[example\]\][\s\S]*?(?=\n\[|$)", "", text)
text = re.sub(r"\nreqwest\s*=.*\n", "\n", text)
text = text.rstrip() + "\n"
inject = (
    "# Pre-generated protobuf bindings are committed under `src/generated/`, so\n"
    "# this bundled copy of `godark` does NOT regenerate them at build time -\n"
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

# Drop `pub mod market_data/rest_client/rest_transport;` (and their
# `pub use` re-exports) from lib.rs — the matching .rs files are excluded
# from the rsync above; leaving the decls in would fail to compile.
# Idempotent.
python3 - "$DEST/src/lib.rs" <<'PY'
import re, sys, pathlib
p = pathlib.Path(sys.argv[1])
text = p.read_text()
for mod in ("market_data", "rest_client", "rest_transport"):
    text = re.sub(rf"^pub mod {mod};\s*\n", "", text, flags=re.M)
    text = re.sub(rf"^pub use {mod}::[^;]+;\s*\n", "", text, flags=re.M)
p.write_text(text)
PY

# Pin the commit (prefer tag for human readability if HEAD is on one).
if [[ -n "$UPSTREAM_TAG" ]]; then
  echo "$UPSTREAM_TAG" > "$DEST/UPSTREAM_REF"
else
  echo "$UPSTREAM_SHA" > "$DEST/UPSTREAM_REF"
fi
echo "  wrote pin: $(cat "$DEST/UPSTREAM_REF")  -> sdk/UPSTREAM_REF"

echo "Vendored size: $(du -sh "$DEST" | cut -f1)"
echo "Done. Review with: cd '$REPO_ROOT' && git status sdk/"
