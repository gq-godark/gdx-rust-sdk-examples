# GoDark Rust SDK

This package provides the GoDark Rust SDK and minimal examples for encrypted
darkpool trading.

Supported order types in this distribution: `MARKET`, `LIMIT`.

## Package contents

- `quickstart`, `full_trader_example` — prebuilt Linux x86_64 binaries
- `examples/` — `quickstart.rs`, `full_trader_example.rs`, `dotenv.rs`
- `sdk/` — bundled `godark` crate
- `Cargo.toml` — workspace manifest for `cargo build --release --examples`
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
| Network     | `crates.io` access for runtime deps; `godark` is bundled in `sdk/` |

> **macOS / Windows / aarch64?** Use the source build path:
> `cargo build --release --examples`.

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

## Cargo integration (your own bot)

Point your `Cargo.toml` at the bundled crate:

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

See `SDK_REFERENCE.md` for the full client API.
