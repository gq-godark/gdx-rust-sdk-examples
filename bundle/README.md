# GoDark Rust SDK

This package provides two prebuilt market-maker examples for the GoDark Rust
SDK and the minimal files needed to run them.

Supported order types in this distribution: `MARKET`, `LIMIT`.

## Package contents

- `quickstart` — prebuilt Linux x86_64 binary (connect → place limit sell → cancel)
- `full_trader_example` — prebuilt Linux x86_64 binary (all 6 sequencer push callbacks)
- `SDK_REFERENCE.md` — API reference
- `.env.example` — environment template

The `godark` SDK is statically linked into each binary, so no Rust toolchain
or private registry is required to run the examples.

## 1) Prerequisites

| Item        | Requirement                                                                   |
|-------------|-------------------------------------------------------------------------------|
| OS / arch   | Linux x86_64 (built on Ubuntu, glibc ≥ 2.18)                                  |
| TLS runtime | `libssl.so.3` + `libcrypto.so.3` (`apt install libssl3` on Debian/Ubuntu)     |
| Other       | `libstdc++` / `libgcc_s` / `libm` / `libc` (standard system libraries)        |

> **macOS / Windows / aarch64?** Build from source instead — see the
> [`gdx-rust-sdk-examples`](https://github.com/gq-godark/gdx-rust-sdk-examples)
> repository for the source tree and a `cargo build --release --examples`
> workflow.

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

```bash
./quickstart
```

Or run the full trader example:

```bash
./full_trader_example
```

## Cargo integration (your own bot)

If you want to build your own bot against the `godark` crate, depend on it as
a path or git dependency from the upstream
[`gdx-rust-sdk`](https://github.com/gq-godark/gdx-rust-sdk) repository — this
distribution only ships the prebuilt example binaries, not the crate source.

```toml
# Cargo.toml
[dependencies]
godark = { git = "https://github.com/gq-godark/gdx-rust-sdk", rev = "<pinned-sha>" }
tokio  = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync"] }
dotenvy = "0.15"
```

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
