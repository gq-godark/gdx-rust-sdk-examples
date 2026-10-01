# GoDark Rust SDK

This package provides the GoDark Rust SDK and minimal examples for encrypted
darkpool trading.

Supported order types in this distribution: `MARKET`, `LIMIT`. Prices and
sizes on the public trading API are **decimal strings only** (for example
`"0.01"`, `Some("68000")`) — not `f64` / `f32` / integers.

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
- `GDX_HPKE_STATIC_PUBLIC_KEY` — sequencer HPKE static public key (64 hex).
  Required for localnet/devnet. Aliases: `GDX_HPKE_STATIC_PUBKEY`,
  `GODARK_HPKE_STATIC_PUBLIC_KEY`, `VITE_GDX_HPKE_STATIC_PUBKEY`.
- `GODARK_ACCOUNT` — 32-byte account encoded as base58; only needed for local
  static-key authentication when the login response omits `account`.

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

## Participant flow

Environment names only (values stay in `.env`): `GODARK_API_KEY_ID`,
`GODARK_API_SECRET`, `GODARK_PASSPHRASE`. Optional: `GODARK_EDGE_URL`,
`GODARK_REST_URL`, `GDX_HPKE_STATIC_PUBLIC_KEY`, `GODARK_ACCOUNT`.

1. **REST auth.** `GodarkRestClient::connect` posts
   `POST /api/v1/auth/token` (`client_credentials`) and keeps the
   `access_token`.
2. **WebSocket login.** `GodarkClient::connect` mints that same REST access
   token and sends it as the `/ws/v1` `login` frame. Do not send
   `key_id:secret:passphrase` on the socket.
3. **Subscribe.** Channels on `/ws/v1` are `orders`, `positions`, `volume`,
   `open_interest`, and `funding_rate`. There is no `trades` or L2 channel.
   Take `take_open_orders_snapshot_receiver()` before `connect`; an
   `open_orders_snapshot` is delivered to that caller.
4. **Place with strings.** Prices and sizes are `&str` / `String`
   (`"0.01"`, `Some("68000")`). `slippage_bps` applies only to `MARKET` and
   `STOP_MARKET`. A peg (`peg_offset_bps`) is not post-only unless you set
   `post_only: true`.
5. **Client order id.** Set `PlaceOrderOptions.client_order_id` on a
   WebSocket place. The SDK registers it only after that place succeeds, and
   caches the mapping only after `_register_coid` returns HTTP 200. A REST
   place does not register the id.
6. **Read a position.** `get_positions()` (REST) or the `positions` channel
   (`take_positions_snapshot_receiver`).
7. **Cancel.** `cancel_order(&order_id, symbol)`.

```rust
use godark::{GodarkClient, GodarkError, OrderType, Side, TimeInForce};

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    let _ = dotenvy::dotenv();

    let config = GodarkClient::builder()
        .api_key_id(std::env::var("GODARK_API_KEY_ID").expect("GODARK_API_KEY_ID"))
        .api_secret(std::env::var("GODARK_API_SECRET").expect("GODARK_API_SECRET"))
        .passphrase(std::env::var("GODARK_PASSPHRASE").expect("GODARK_PASSPHRASE"))
        .build()?;

    let mut client = GodarkClient::new(config);
    let mut positions = client
        .take_positions_snapshot_receiver()
        .expect("positions receiver");
    client.connect().await?;
    client
        .subscribe(&["orders", "positions", "volume", "open_interest", "funding_rate"])
        .await?;

    let ack = client
        .place_order(
            "BTC-USDC-PERP",
            Side::Sell,
            OrderType::Limit,
            "0.01",
            Some("68000"),
            TimeInForce::Gtc,
            false,
            None,
            None,
        )
        .await?;

    if let Ok(snap) = positions.try_recv() {
        println!("positions: {}", snap.rows.len());
    }
    client.cancel_order(&ack.order_id, "BTC-USDC-PERP").await?;
    client.disconnect().await;
    Ok(())
}
```

See `SDK_REFERENCE.md` for the full client API.
