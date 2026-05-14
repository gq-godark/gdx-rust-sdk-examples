# GoDark Rust SDK

This package provides two prebuilt market-maker examples for the GoDark Rust
SDK, **plus their full source tree and the vendored `godark` crate** so you
can rebuild the examples or scaffold your own bot directly against the
shipped sources — no private registry required, no `protoc` required (the
SDK ships pre-generated protobuf bindings under `sdk/src/generated/`).

Supported order types in this distribution: `MARKET`, `LIMIT`.

## Package contents

- `quickstart`, `full_trader_example` — **prebuilt Linux x86_64 binaries** (run
  these directly with no toolchain installed)
- `examples/` — example **source files** (`quickstart.rs`,
  `full_trader_example.rs`, `dotenv.rs`)
- `sdk/` — **vendored `godark` crate** source (with pre-generated protobuf
  bindings); `sdk/UPSTREAM_REF` records the upstream commit the binaries
  were built from
- `Cargo.toml` — workspace manifest depending on `godark = { path = "sdk" }`,
  ready for `cargo build --release --examples`
- `README.md`, `SDK_REFERENCE.md` — recipient docs
- `.env.example` — environment template

## 1) Prerequisites

To **run the prebuilt binaries**, you only need the Linux runtime libs:

| Item        | Requirement                                                                   |
|-------------|-------------------------------------------------------------------------------|
| OS / arch   | Linux x86_64 (built on Ubuntu, glibc ≥ 2.18)                                  |
| TLS runtime | `libssl.so.3` + `libcrypto.so.3` (`apt install libssl3` on Debian/Ubuntu)     |
| Other       | `libstdc++` / `libgcc_s` / `libm` / `libc` (standard system libraries)        |

To **rebuild from source** (or build your own bot against the bundled
`sdk/`), additionally install:

| Item        | Requirement                                                                   |
|-------------|-------------------------------------------------------------------------------|
| Rust        | stable ≥ 1.79 (`curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \| sh -s -- -y --default-toolchain stable`) |
| Network     | `crates.io` access for the standard runtime crates (`tokio`, `prost`, `serde`, `reqwest`, …); the `godark` crate itself is bundled |

> **macOS / Windows / aarch64?** The prebuilt binaries are Linux x86_64 only,
> but the source-build path works on any platform Rust supports — clone or
> unzip this bundle and run `cargo build --release --examples`.

## 2) Create testnet credentials

1. Open the testnet frontend: `https://app.godark-dex.com`
2. Create an account using email sign-up.
3. Fund the account using the faucet: `https://faucet.godark-dex.com`
4. In the frontend, go to **Settings → API Key Management** and click
   **Create API Key**.

## 3) Configure environment

Copy `.env.example` to `.env` and set:

- `GODARK_API_KEY_ID`
- `GODARK_API_SECRET`

```bash
cp .env.example .env
$EDITOR .env       # fill in your testnet creds
```

Optional override:

- `GODARK_EDGE_URL` — defaults to `wss://api.godark-dex.com` if unset.

The OS environment always wins over `.env`.

## 4) Run quickstart

Run the prebuilt binary directly:

```bash
./quickstart
```

Or the full trader example:

```bash
./full_trader_example
```

To rebuild from the included sources instead (e.g. on a non-Linux host or
after editing `examples/*.rs`):

```bash
cargo build --release --examples
./target/release/examples/quickstart
./target/release/examples/full_trader_example
```

The bundled `Cargo.toml` already wires `godark = { path = "sdk" }`, so the
build is fully offline-capable for the SDK itself; `cargo` only fetches
the third-party runtime crates (`tokio`, `prost`, `serde`, `reqwest`, …)
from `crates.io`.

## Cargo integration (your own bot)

The bundle includes a vendored `godark` crate under `sdk/`. To build your
own bot against the same SDK revision, point your `Cargo.toml` at the
bundled crate via a path dependency:

```toml
# Cargo.toml — your own bot
[dependencies]
godark  = { path = "path/to/this-bundle/sdk" }
tokio   = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync"] }
dotenvy = "0.15"
```

(Or copy `sdk/` into your own project and reference it as `path = "sdk"`.)

Then in `src/main.rs`:

```rust
use godark::{GodarkClient, GodarkError, OrderType, Side, TimeInForce};

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    let _ = dotenvy::dotenv();

    let config = GodarkClient::builder()
        .base_url(std::env::var("GODARK_EDGE_URL")
            .unwrap_or_else(|_| "wss://api.godark-dex.com".into()))
        .api_key_id(std::env::var("GODARK_API_KEY_ID").expect("GODARK_API_KEY_ID"))
        .api_secret(std::env::var("GODARK_API_SECRET").expect("GODARK_API_SECRET"))
        .build()?;

    let mut client = GodarkClient::new(config);
    client.connect().await?;

    let ack = client
        .place_order(
            "BTC-USDC-PERP",
            Side::Sell,
            OrderType::Limit,
            0.01,
            Some(999_999.0),
            TimeInForce::Gtc,
            false,
            None,
            None,
        )
        .await?;

    client.cancel_order(&ack.order_id, "BTC-USDC-PERP").await?;
    client.disconnect().await;
    Ok(())
}
```

If you'd rather pin against the upstream `gdx-rust-sdk` repository directly
(useful if you're tracking a moving branch rather than a release pin), the
bundled `sdk/UPSTREAM_REF` file records the exact commit this distribution
was built from:

```toml
godark = { git = "https://github.com/gq-godark/gdx-rust-sdk.git",
           rev = "<contents of sdk/UPSTREAM_REF>" }
```

Note: building `gdx-rust-sdk` from upstream source requires `protoc` on
`$PATH` (the upstream crate regenerates protobuf bindings via
`prost-build`). The bundled `sdk/` does **not** share that requirement
because the bindings are pre-generated under `sdk/src/generated/`.

See `SDK_REFERENCE.md` for the full client API.
