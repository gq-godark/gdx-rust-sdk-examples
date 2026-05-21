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
#   6. Stages the binaries + example sources + vendored sdk/ + a top-level
#      Cargo.toml + recipient docs from bundle/, then zips them. Recipients
#      can either run the prebuilt binaries directly (no toolchain needed)
#      or `cargo build --release --examples` from the unzipped bundle.
#
# Output layout:
#   <DIST_NAME>/
#   |-- quickstart                 (prebuilt static-ish ELF, x86_64 Linux)
#   |-- full_trader_example        (prebuilt)
#   |-- Cargo.toml                 (workspace manifest; godark = { path = "sdk" })
#   |-- .env.example
#   |-- README.md                  (from bundle/README.md)
#   |-- SDK_REFERENCE.md           (from bundle/SDK_REFERENCE.md)
#   |-- examples/
#   |   |-- quickstart.rs
#   |   |-- full_trader_example.rs
#   |   `-- dotenv.rs
#   `-- sdk/
#       |-- Cargo.toml             (godark crate manifest)
#       |-- README.md
#       |-- shared/symbols.json
#       `-- src/                   (godark crate source incl. pre-generated proto)
#
# Usage:
#   bash scripts/package.sh                              # default: godark-rust-sdk-linux-x86_64.zip
#   bash scripts/package.sh my-release-name
#   UPSTREAM_SRC=/path/to/gdx-rust-sdk bash scripts/package.sh
set -euo pipefail

UPSTREAM_REPO="gq-godark/gdx-rust-sdk"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DIST_NAME="${1:-godark-rust-sdk-linux-x86_64}"

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

for required in bundle/README.md bundle/SDK_REFERENCE.md bundle/Cargo.toml \
                bundle/sdk/README.md .env.example \
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
# Excludes mirror the rsync drops in refresh_sdk.sh, plus lib.rs (which has
# its decls for those modules trimmed by refresh_sdk.sh — checked separately
# below against a re-derived trim so non-trim edits still fail parity).
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

# lib.rs parity: apply refresh_sdk.sh's trim to upstream and bit-compare.
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
mkdir -p "$DEST/examples" "$DEST/sdk"

# Prebuilt binaries — recipients can run these directly without a Rust
# toolchain. Built above against vendored sdk/, which the parity check
# above guarantees is bit-equal to $UPSTREAM_SRC/src.
cp "$QUICKSTART_BIN"                          "$DEST/quickstart"
cp "$FULL_TRADER_BIN"                         "$DEST/full_trader_example"

# MM-facing docs come from bundle/, never from the repo-root copies.
cp "${REPO_ROOT}/.env.example"                "$DEST/.env.example"
cp "${REPO_ROOT}/bundle/README.md"            "$DEST/README.md"
cp "${REPO_ROOT}/bundle/SDK_REFERENCE.md"     "$DEST/SDK_REFERENCE.md"

# Top-level Cargo.toml — recipient-facing manifest from bundle/.
cp "${REPO_ROOT}/bundle/Cargo.toml"             "$DEST/Cargo.toml"

# Example sources — recipients can read them and rebuild against the
# bundled sdk/ for their own bot scaffolding.
cp "${REPO_ROOT}/examples/quickstart.rs"           "$DEST/examples/"
cp "${REPO_ROOT}/examples/full_trader_example.rs"  "$DEST/examples/"
cp "${REPO_ROOT}/examples/dotenv.rs"               "$DEST/examples/"

# Bundled godark crate — copied from $REPO_ROOT/sdk/ after parity check.
cp "${REPO_ROOT}/sdk/Cargo.toml"              "$DEST/sdk/Cargo.toml"
cp -r "${REPO_ROOT}/sdk/src"                  "$DEST/sdk/src"
cp "${REPO_ROOT}/bundle/sdk/README.md"        "$DEST/sdk/README.md"
if [[ -d "${REPO_ROOT}/sdk/shared" ]]; then
  cp -r "${REPO_ROOT}/sdk/shared"             "$DEST/sdk/shared"
fi

# Remove internal maintainer markers from shipped SDK sources (repo copy stays
# parity-checked against upstream; recipients see cleaned comments only).
python3 - "$DEST/sdk/src/order_error_code.rs" "$DEST/sdk/src/transport.rs" <<'PY'
import pathlib, sys
replacements = [
    (pathlib.Path(sys.argv[1]), [
        (
            "//! Mirror of the canonical `OrderErrorCode` enum from\n"
            "//! `gdx-protocol/src/order_error.rs` so the Rust SDK can produce informative\n"
            "//! messages for protobuf-encoded ACK rejections (which carry only a numeric\n"
            "//! `error_code` on the wire).\n"
            "//!\n"
            "//! The protocol crate is internal to the trading core; clients embed this\n"
            "//! standalone copy so adding a new variant on the sequencer side requires\n"
            "//! appending a row to [`ORDER_ERROR_CODES`] (preserving numeric codes; the\n"
            "//! Rust enum in `gdx-protocol` is the source of truth).\n",
            "//! Mirror of the canonical `OrderErrorCode` enum so the Rust SDK can produce\n"
            "//! informative messages for protobuf-encoded ACK rejections (which carry only a\n"
            "//! numeric `error_code` on the wire).\n"
            "//!\n"
            "//! Clients embed this standalone copy so adding a new variant on the sequencer\n"
            "//! side requires appending a row to [`ORDER_ERROR_CODES`] (preserving numeric\n"
            "//! codes; the canonical protocol schema is the source of truth).\n",
        ),
        (
            "    /// Wire code from `gdx-protocol::OrderErrorCode::raw()`.",
            "    /// Wire code from the canonical order-error schema.",
        ),
        (
            "/// All canonical order-error codes the sequencer can emit. Keep in sync with\n"
            "/// `gdx-protocol/src/order_error.rs`.",
            "/// All canonical order-error codes the sequencer can emit.",
        ),
    ]),
    (pathlib.Path(sys.argv[2]), [
        (
            "    // Regression test for the gdx-rust-sdk subscribe race fixed alongside\n"
            "    // gdx-core PR #203. We",
            "    // Regression test for the subscribe race fixed alongside core PR #203. We",
        ),
    ]),
]
for path, pairs in replacements:
    text = path.read_text()
    for old, new in pairs:
        if old not in text:
            raise SystemExit(f"missing expected text in {path}: {old[:60]!r}...")
        text = text.replace(old, new, 1)
    path.write_text(text)
PY
python3 - "$DEST/sdk/Cargo.toml" <<'PY'
import re, pathlib, sys
p = pathlib.Path(sys.argv[1])
text = p.read_text()
text = re.sub(
    r"# Pre-generated protobuf bindings are committed under `src/generated/`, so\n"
    r"# this (?:vendored|bundled) copy of `godark` does NOT regenerate them at build time -\n"
    r"# consumers do not need `protoc` or `prost-build`\. Ship only the library\n"
    r"# target; the SDK's own examples and integration tests are excluded from\n"
    r"# this distribution\.\n",
    "# Pre-generated protobuf bindings ship under `src/generated/`.\n",
    text,
)
text = re.sub(r'^repository = .*$\n', '', text, flags=re.M)
p.write_text(text)
PY

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

# Recipient contract: no maintainer-only directories must leak.
# (sdk/ and examples/ are now part of the recipient-facing layout — the
# previous binary-only contract was relaxed when we started shipping the
# vendored crate + example sources for offline source builds.)
if echo "$LISTING" | grep -E "${DIST_NAME}/(scripts|target|bundle|\.git)/" >/dev/null; then
  echo "error: bundle contains forbidden internal directory" >&2
  exit 1
fi
# Every required path must be present.
for required in \
  "${DIST_NAME}/quickstart" \
  "${DIST_NAME}/full_trader_example" \
  "${DIST_NAME}/README\\.md" \
  "${DIST_NAME}/SDK_REFERENCE\\.md" \
  "${DIST_NAME}/Cargo\\.toml" \
  "${DIST_NAME}/\\.env\\.example" \
  "${DIST_NAME}/examples/quickstart\\.rs" \
  "${DIST_NAME}/examples/full_trader_example\\.rs" \
  "${DIST_NAME}/examples/dotenv\\.rs" \
  "${DIST_NAME}/sdk/Cargo\\.toml" \
  "${DIST_NAME}/sdk/src/lib\\.rs"; do
  if ! echo "$LISTING" | grep -E "${required}" >/dev/null; then
    echo "error: bundle missing required entry: ${required}" >&2
    exit 1
  fi
done

echo
echo "bundle-shape assertion: PASSED"

if echo "$LISTING" | grep -E "${DIST_NAME}/\\.env$" >/dev/null; then
  echo "error: bundle contains .env — ship .env.example only" >&2
  exit 1
fi
if echo "$LISTING" | grep -E "${DIST_NAME}/sdk/UPSTREAM_REF$" >/dev/null; then
  echo "error: bundle contains sdk/UPSTREAM_REF — maintainer metadata must not ship" >&2
  exit 1
fi

# Must NOT leak internal repo names or maintainer markers into the archive.
if unzip -p "$ARCHIVE" 2>/dev/null | strings | grep -qiE \
  'gdx-rust-sdk|UPSTREAM_REF|refresh_sdk|package\.sh|\bvendored\b|gdx-proto|gdx-protocol'; then
  echo "error: bundle contains internal repo references or maintainer markers" >&2
  unzip -p "$ARCHIVE" 2>/dev/null | strings | grep -iE \
    'gdx-rust-sdk|UPSTREAM_REF|refresh_sdk|package\.sh|\bvendored\b|gdx-proto|gdx-protocol' | head -20 >&2 || true
  exit 1
fi

echo "leak guard: PASSED"
echo "built from upstream:    ${UPSTREAM_REPO}@${PINNED_REF} (${upstream_head_sha})"
