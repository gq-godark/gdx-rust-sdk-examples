# GoDark Rust SDK

This package provides the GoDark Rust SDK and minimal examples for encrypted
darkpool trading.

Supported order types in this distribution: `MARKET`, `LIMIT`.

## Package contents

- `examples/` — `quickstart.rs`, `full_trader_example.rs`, `rest_client_example.rs`, `dotenv.rs`
- `sdk/` — bundled `godark` crate
- `Cargo.toml` — workspace manifest for `cargo build --release --examples`
- `README.md`, `SDK_REFERENCE.md` — recipient docs
- `.env.example` — environment template

## 1) Prerequisites

| Item    | Requirement                                                                       |
|---------|-----------------------------------------------------------------------------------|
| OS / arch | any platform Rust supports (Linux, macOS, Windows; amd64, arm64, …)              |
| Rust    | stable ≥ 1.79 (`https://rustup.rs/`)                                              |
| Network | `crates.io` access for runtime deps; `godark` is bundled in `sdk/`                |

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
- `GODARK_PASSPHRASE`

```bash
cp .env.example .env
$EDITOR .env       # fill in your testnet creds
```

Optional override:

- `GODARK_EDGE_URL` — override the edge URL (default: public testnet `wss://api.godark-dex.com` via the SDK Testnet environment preset).
- `GDX_NOISE_STATIC_PUBLIC_KEY` — override the sequencer Noise pin. **Not required for public testnet** — the SDK Environment Testnet preset bakes it in. Aliases: `GDX_NOISE_STATIC_PUBKEY`, `GODARK_NOISE_STATIC_PUBLIC_KEY`.

The OS environment always wins over `.env`.

## 4) Build and run the examples

From inside the unzipped bundle:

```bash
cargo build --release --example quickstart
cargo build --release --example full_trader_example
cargo build --release --example rest_client_example
```

Then run:

```bash
./target/release/examples/quickstart
./target/release/examples/full_trader_example
./target/release/examples/rest_client_example
```

The bundled `Cargo.toml` resolves `godark` from `./sdk`.

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
