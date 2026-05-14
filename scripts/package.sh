#!/usr/bin/env bash
# MM bundle packager - Linux x86_64 zip distribution, built strictly from
# the pinned upstream gdx-rust-sdk commit recorded in sdk/UPSTREAM_REF.
#
# What this script does:
#   1. Reads the pinned upstream ref from sdk/UPSTREAM_REF.
#   2. Resolves the upstream source tree:
#        - If $UPSTREAM_SRC is set, use that directory (CI / explicit local
#          checkout).
#        - Else if a sibling ../gdx-rust-sdk exists, use that.
#        - Else clone gq-godark/gdx-rust-sdk@<pinned-ref> into a temp dir
#          (requires `gh` or `git`, plus auth for the private repo).
#   3. Verifies the resolved upstream is at exactly the pinned ref.
#   4. Parity check: vendored sdk/src/ must match $UPSTREAM_SRC/src/
#      (excluding files refresh_sdk.sh deliberately drops: market_data.rs,
#      rest_client.rs, rest_transport.rs). Drift here means somebody
#      hand-edited the vendored copy or forgot to bump UPSTREAM_REF after a
#      refresh - fail loudly.
#   5. Builds release binaries via `cargo build --release --examples`. The
#      parity check above guarantees vendored sdk/ is bit-equal to upstream/
#      src/ for every file actually compiled, so the resulting binaries are
#      reproducible from the public upstream pin.
#   6. Stages the binaries + recipient docs from bundle/ and zips them.
#
# Output layout:
#   <DIST_NAME>/
#   |-- quickstart
#   |-- full_trader_example
#   |-- .env.example
#   |-- README.md             (from bundle/README.md)
#   `-- SDK_REFERENCE.md      (from bundle/SDK_REFERENCE.md)
#
# Usage:
#   bash scripts/package.sh
#   bash scripts/package.sh my-release-name
#   UPSTREAM_SRC=/path/to/gdx-rust-sdk bash scripts/package.sh
set -euo pipefail

UPSTREAM_REPO="gq-godark/gdx-rust-sdk"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_NAME="${1:-gdx-rust-sdk-examples-linux-x86_64}"

cd "$REPO_ROOT"

# ---- pre-flight ------------------------------------------------------------
if [[ ! -f "${REPO_ROOT}/sdk/UPSTREAM_REF" ]]; then
  echo "error: sdk/UPSTREAM_REF missing - run scripts/refresh_sdk.sh first" >&2
  exit 1
fi
PINNED_REF="$(tr -d '[:space:]' < "${REPO_ROOT}/sdk/UPSTREAM_REF")"
if [[ -z "$PINNED_REF" ]]; then
  echo "error: sdk/UPSTREAM_REF is empty" >&2
  exit 1
fi

for required in bundle/README.md bundle/SDK_REFERENCE.md .env.example \
                examples/quickstart.rs examples/full_trader_example.rs examples/dotenv.rs; do
  if [[ ! -f "${REPO_ROOT}/${required}" ]]; then
    echo "error: required source file missing: ${required}" >&2
    exit 1
  fi
done
if ! command -v zip >/dev/null 2>&1; then
  echo "error: 'zip' not found in PATH (apt-get install zip)" >&2
  exit 1
fi
if ! command -v cargo >/dev/null 2>&1; then
  echo "error: 'cargo' not found in PATH (install rustup + stable toolchain)" >&2
  exit 1
fi

# ---- resolve upstream source tree -----------------------------------------
CLEANUP_UPSTREAM=false

if [[ -n "${UPSTREAM_SRC:-}" ]]; then
  echo "Using UPSTREAM_SRC=${UPSTREAM_SRC}"
elif [[ -d "${REPO_ROOT}/../gdx-rust-sdk/.git" ]]; then
  UPSTREAM_SRC="$(cd "${REPO_ROOT}/../gdx-rust-sdk" && pwd)"
  echo "Using sibling upstream checkout: $UPSTREAM_SRC"
else
  CLEANUP_UPSTREAM=true
  UPSTREAM_SRC="$(mktemp -d)/gdx-rust-sdk"
  echo "Cloning ${UPSTREAM_REPO}@${PINNED_REF} -> $UPSTREAM_SRC ..."
  if command -v gh >/dev/null 2>&1; then
    gh repo clone "${UPSTREAM_REPO}" "$UPSTREAM_SRC" -- --quiet --filter=blob:none
  else
    git clone --quiet --filter=blob:none "https://github.com/${UPSTREAM_REPO}.git" "$UPSTREAM_SRC"
  fi
  git -C "$UPSTREAM_SRC" checkout --quiet "$PINNED_REF"
fi

cleanup() {
  if [[ "$CLEANUP_UPSTREAM" == true && -n "${UPSTREAM_SRC:-}" ]]; then
    rm -rf "$(dirname "$UPSTREAM_SRC")"
  fi
}
trap cleanup EXIT

# ---- verify upstream is at the pinned ref ---------------------------------
if [[ ! -d "$UPSTREAM_SRC/.git" ]]; then
  echo "error: '$UPSTREAM_SRC' is not a git checkout - cannot verify pin" >&2
  exit 1
fi
upstream_head_sha="$(git -C "$UPSTREAM_SRC" rev-parse HEAD)"
upstream_pin_sha="$(git -C "$UPSTREAM_SRC" rev-parse "$PINNED_REF" 2>/dev/null || true)"
if [[ -z "$upstream_pin_sha" ]]; then
  echo "error: pinned ref '$PINNED_REF' does not resolve in $UPSTREAM_SRC" >&2
  echo "       (try: git -C $UPSTREAM_SRC fetch --tags origin)" >&2
  exit 1
fi
if [[ "$upstream_head_sha" != "$upstream_pin_sha" ]]; then
  echo "error: upstream HEAD ($upstream_head_sha) does not match pinned ref" >&2
  echo "       sdk/UPSTREAM_REF=$PINNED_REF -> $upstream_pin_sha" >&2
  echo "       checkout the pinned ref before packaging:" >&2
  echo "         git -C $UPSTREAM_SRC checkout $PINNED_REF" >&2
  exit 1
fi
echo "Upstream verified at pin: $PINNED_REF ($upstream_head_sha)"

# ---- parity check: vendored sdk/src must match upstream src ---------------
# Exclusions match the deliberate drops in scripts/refresh_sdk.sh - the SDK's
# REST + standalone market-data surfaces are not used by either MM example
# and are intentionally excluded from the vendored copy.
#
# lib.rs is also excluded from the recursive bit-equality check because the
# vendored copy has its `pub mod market_data;` / `pub mod rest_client;` /
# `pub mod rest_transport;` declarations stripped (otherwise the crate
# wouldn't compile - the matching .rs files are excluded above). We
# re-derive that trim from upstream's lib.rs separately and compare,
# so any non-trim drift in lib.rs is still caught.
PARITY_EXCLUDES=(
  --exclude='market_data.rs'
  --exclude='rest_client.rs'
  --exclude='rest_transport.rs'
  --exclude='lib.rs'
)
if ! diff -r --brief "${PARITY_EXCLUDES[@]}" \
       "$UPSTREAM_SRC/src" "$REPO_ROOT/sdk/src" >/dev/null; then
  echo
  echo "error: vendored sdk/src/ has drifted from upstream $PINNED_REF:" >&2
  diff -r --brief "${PARITY_EXCLUDES[@]}" \
       "$UPSTREAM_SRC/src" "$REPO_ROOT/sdk/src" >&2 || true
  echo >&2
  echo "  fix: bash scripts/refresh_sdk.sh $UPSTREAM_SRC && git add sdk/ && git commit" >&2
  exit 1
fi

# lib.rs parity check: trim upstream's lib.rs the same way refresh_sdk.sh
# does, then bit-compare against the vendored copy. This guards against
# silent edits to vendored lib.rs (e.g. adding a `pub mod evil;` referring
# to a smuggled-in source file) that the recursive diff above would miss.
EXPECTED_LIB_RS="$(mktemp)"
python3 - "$UPSTREAM_SRC/src/lib.rs" > "$EXPECTED_LIB_RS" <<'PY'
import re, sys, pathlib
text = pathlib.Path(sys.argv[1]).read_text()
for mod in ("market_data", "rest_client", "rest_transport"):
    text = re.sub(rf"^pub mod {mod};\s*\n", "", text, flags=re.M)
    text = re.sub(rf"^pub use {mod}::[^;]+;\s*\n", "", text, flags=re.M)
sys.stdout.write(text)
PY
if ! diff -u "$EXPECTED_LIB_RS" "$REPO_ROOT/sdk/src/lib.rs" >/dev/null; then
  echo
  echo "error: vendored sdk/src/lib.rs has drifted from the expected trim of upstream $PINNED_REF:" >&2
  diff -u "$EXPECTED_LIB_RS" "$REPO_ROOT/sdk/src/lib.rs" >&2 || true
  echo >&2
  echo "  fix: bash scripts/refresh_sdk.sh $UPSTREAM_SRC && git add sdk/ && git commit" >&2
  rm -f "$EXPECTED_LIB_RS"
  exit 1
fi
rm -f "$EXPECTED_LIB_RS"

echo "Parity check passed: sdk/src/ matches $UPSTREAM_SRC/src/ (lib.rs trim verified)"

# ---- build release binaries ----------------------------------------------
# Build from the vendored sdk/ tree (parity check above guarantees it is
# byte-identical to $UPSTREAM_SRC/src for every file the workspace
# actually compiles). This keeps the build hermetic - no `protoc` required,
# no `build.rs` invocation - because the vendored Cargo.toml has its
# [build-dependencies] stripped by refresh_sdk.sh.
echo "Building release binaries (quickstart + full_trader_example)..."
cargo build --release --examples --quiet

QUICKSTART_BIN="$REPO_ROOT/target/release/examples/quickstart"
FULL_TRADER_BIN="$REPO_ROOT/target/release/examples/full_trader_example"

for bin in "$QUICKSTART_BIN" "$FULL_TRADER_BIN"; do
  if [[ ! -x "$bin" ]]; then
    echo "error: expected binary missing or non-executable: $bin" >&2
    exit 1
  fi
done

# ---- stage ----------------------------------------------------------------
STAGING_DIR="$(mktemp -d)"
DEST="$STAGING_DIR/$DIST_NAME"
mkdir -p "$DEST"

echo "Staging distribution at $DEST ..."
cp "$QUICKSTART_BIN"                          "$DEST/quickstart"
cp "$FULL_TRADER_BIN"                         "$DEST/full_trader_example"
cp "${REPO_ROOT}/.env.example"                "$DEST/.env.example"
cp "${REPO_ROOT}/bundle/README.md"            "$DEST/README.md"
cp "${REPO_ROOT}/bundle/SDK_REFERENCE.md"     "$DEST/SDK_REFERENCE.md"

# ---- zip ------------------------------------------------------------------
ARCHIVE="$REPO_ROOT/${DIST_NAME}.zip"
rm -f "$ARCHIVE"
( cd "$STAGING_DIR" && zip -qr "$ARCHIVE" "$DIST_NAME" )
rm -rf "$STAGING_DIR"

# ---- post-flight assertions ----------------------------------------------
echo
echo "Package created: $ARCHIVE"
LISTING="$(unzip -l "$ARCHIVE")"
echo "$LISTING"

# Recipient-only contract: no internal directories must leak.
if echo "$LISTING" | grep -E "${DIST_NAME}/(sdk|scripts|target|examples|bundle)/" >/dev/null; then
  echo "error: bundle contains forbidden internal directory - binary-only contract violated" >&2
  exit 1
fi
for required in \
  "${DIST_NAME}/quickstart" \
  "${DIST_NAME}/full_trader_example" \
  "${DIST_NAME}/README\\.md" \
  "${DIST_NAME}/SDK_REFERENCE\\.md" \
  "${DIST_NAME}/\\.env\\.example"; do
  if ! echo "$LISTING" | grep -E "${required}" >/dev/null; then
    echo "error: bundle missing required entry: ${required}" >&2
    exit 1
  fi
done

echo
echo "binary-only assertion: PASSED"
echo "built from upstream:    ${UPSTREAM_REPO}@${PINNED_REF} (${upstream_head_sha})"
