# GoDark Rust Examples (Darkpool MM Distribution)

This repository is a self-contained, market-maker-facing distribution for
GoDark's Rust SDK. It includes:

- two minimal darkpool trading examples (`quickstart` + `full_trader_example`)
- the full `godark` SDK source vendored under `sdk/` — no private registry
  required, no `protoc` required (pre-generated protobuf bindings ship with
  the SDK)
- a simple `.env` workflow (no shell `export` required)
- a `scripts/package.sh` that produces a portable Linux x86_64 binary tarball
  for downstream MMs who don't want to install a Rust toolchain

## Two ways to use

### A — Pre-built binaries (no Rust toolchain required)

```bash
# unzip the distribution archive provided to you
unzip godark-rust-examples-linux-x86_64.zip
cd godark-rust-examples-linux-x86_64/

cp .env.example .env
$EDITOR .env       # set GODARK_API_KEY_ID, GODARK_API_SECRET

./quickstart
./full_trader_example
```

#### Platform requirements

| Item        | Requirement                                                                   |
|-------------|-------------------------------------------------------------------------------|
| OS / arch   | Linux x86_64 (built on Ubuntu 24.04, glibc ≥ 2.18)                            |
| TLS runtime | `libssl.so.3` + `libcrypto.so.3` (`apt install libssl3` on Debian/Ubuntu)     |
| Other       | `libstdc++` / `libgcc_s` / `libm` / `libc` (standard system libraries)        |

> **macOS / Windows / aarch64?** Build from source (next section).

### B — Build from source

#### Prerequisites

- Rust ≥ 1.79 (`rustup install stable`)
- Cargo (bundled with the toolchain)
- Network access to `crates.io` for the standard runtime crates that the SDK
  pulls in (`tokio`, `prost`, `serde`, `reqwest`, etc.). The `godark` SDK
  itself is vendored under `sdk/`; you do not need access to any private
  registry.

#### Build

```bash
cargo build --release --examples
```

Built binaries land in `target/release/examples/`. Or run a single example
directly:

```bash
cargo run --release --example quickstart
cargo run --release --example full_trader_example
```

## Testnet onboarding

Before running the examples, complete this setup flow:

1. Open the testnet frontend: `https://app.godark-dex.com`
2. Create an account using email sign-up.
3. Fund your testnet account using the faucet: `https://faucet.godark-dex.com`
4. In the frontend, go to **Settings → API Key Management** and click
   **Create API Key**.
5. Use the generated key id and secret for your local `.env`.

## Configure credentials

Copy `.env.example` to `.env` and fill in your API credentials:

```bash
cp .env.example .env
```

Required keys:

- `GODARK_API_KEY_ID`
- `GODARK_API_SECRET`

Optional:

- `GODARK_EDGE_URL` (defaults to `wss://api.godark-dex.com`)

The OS environment always wins over `.env`.

## Examples

| Target | Source | Purpose |
|--------|--------|---------|
| `quickstart` | `examples/quickstart.rs` | Minimal connect → place limit sell → cancel; demonstrates the symbolic `OrderError::error_code` reason on rejection. |
| `full_trader_example` | `examples/full_trader_example.rs` | Full darkpool trading flow with all 6 sequencer push callbacks (`positions_snapshot`, `system_health`, `balance_update`, `margin_alert`, `funding_rate`, `settlement`), order placement, modify, cancel, and queued-update drain. |

Order-type support in this MM distribution is limited to `MARKET` and `LIMIT`.

## Packaging for Market Makers

Create a clean distributable archive of the two pre-built binaries:

```bash
./scripts/package.sh
```

This creates `godark-rust-examples-linux-x86_64.tar.gz` containing:

- `quickstart`
- `full_trader_example`
- `.env.example`
- `README.md`
- `SDK_REFERENCE.md`

Internal files (`scripts/`, `sdk/`, `target/`, `.git/`) are not included.

## Layout

| Path | Purpose |
|------|---------|
| `examples/quickstart.rs` | Minimal connect / place / cancel example |
| `examples/full_trader_example.rs` | Reference bot flow with all 6 push callbacks |
| `examples/dotenv.rs` | Tiny shared helper (`load_dotenv` + symbolic error printer) |
| `Cargo.toml` | Examples crate; depends on the vendored `godark` via `path = "sdk"` |
| `sdk/` | Vendored `godark` SDK source (with pre-generated protobuf bindings under `sdk/src/generated/`) |
| `.env.example` | Credential template for local `.env` |
| `SDK_REFERENCE.md` | API usage reference for trading integration |
| `scripts/refresh_sdk.sh` | Internal script for refreshing `sdk/` from a sibling SDK checkout |
| `scripts/package.sh` | Produces the binary tarball shipped to MMs |
