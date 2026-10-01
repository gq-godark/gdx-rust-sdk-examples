# GoDark Rust SDK

Encrypted Rust client for the GoDark DEX. Protocol matches **gdx-edge** and
**gdx-sequencer**: HPKE Base (RFC 9180) over WebSocket binary frames, and
one-shot HPKE on REST.

## Quickstart

```bash
git clone <repo-url> gdx-rust-sdk
cd gdx-rust-sdk
cargo build --all-targets
cargo test
```

Published crate: `cargo add godark`. `build.rs` uses committed `src/generated`
unless you regenerate (`bash scripts/proto_gen.sh`).

## Layout

```
src/
  lib.rs           public re-exports (`GodarkClient`, `GodarkRestClient`, …)
  client.rs        WebSocket trading client
  rest_client.rs   REST trading client
  config.rs        Environment + builder
  types.rs         domain types
  enums.rs         order/side/status enums
  error.rs
  market_data.rs
  hpke.rs          crate-private crypto
  session.rs       crate-private WS HPKE session
  wire.rs          crate-private TradingWsBinaryFrame
  transport.rs     crate-private WS transport
  generated/       committed prost bindings
tests/             mocks + live `#[ignore]` suites
examples/
```

## WebSocket

JSON text frames are control only: `login`, `subscribe`/`unsubscribe`, `ping`,
`logout`, `get_order_history`. Encrypted orders and HPKE setup are **binary**
`TradingWsBinaryFrame` (`HpkeSetup` → `HpkeSetupReply` → `EncryptedOrder` /
`EncryptedPush`).

Login returns `conn_id`. HPKE info is `gdx-hpke/v1\0 ‖ account ‖ conn_id_be`.
Send nonces start at **0** (sequencer `last_recv_nonce` is unset until the first
request). Wire `version = 2`.

Authenticated identity is a 32-byte [`AccountId`](src/types.rs), encoded as a
Solana-style base58 string in JSON and as raw bytes in protobuf. The edge
normally returns `account` during authentication (and in JWT `sub`). Local
static-key setups can provide `.account(...)` or `GODARK_ACCOUNT`.

Pin the sequencer static public key (64 hex):

```rust
use godark::{Environment, GodarkClient};

let config = GodarkClient::builder()
    .environment(Environment::Localnet)
    .api_key_id("gdk_...")
    .api_secret("...")
    .passphrase("...")
    .hpke_static_public_key_hex(std::env::var("GDX_HPKE_STATIC_PUBLIC_KEY")?)
    .build()?;
```

| Environment | Default URL |
|---|---|
| Testnet | `wss://api.godark-dex.com/ws/v1` |
| Devnet | `wss://api.devnet.godark-dex.com/ws/v1` |
| Localnet | `ws://127.0.0.1:13300/ws/v1` |

HPKE pins are **not** baked into the crate. Set `.hpke_static_public_key_hex(...)`
or `GDX_HPKE_STATIC_PUBLIC_KEY`.

Balances come from sequencer `BalanceUpdateMessage` / encrypted
`balance_and_position` (trading collateral `balance_raw`).

Prices and sizes on place / modify / mass-quote / batch-modify / TP-SL /
`quote_notional` / `min_fill_size` / triggers are **decimal strings only**
(for example `"0.01"`, `Some("67500.5")`). There is no public `f64` / `f32` /
integer price or size input; invalid strings are rejected by
`normalize_decimal`. `place_order_with_options` also accepts
`PlaceOrderOptions::slippage_bps` for market and stop-market orders (max walk
from mark in basis points; `None` → venue limit).

## REST

`POST /api/v1/auth/token`, then encrypted `POST/PATCH/DELETE /api/v1/orders`
with JSON `{ header, encrypted_body, encapped_key, request_id }`. Each call is
a fresh HPKE setup (`info = gdx-hpke/v1/rest\0 ‖ account ‖ request_id_be`,
`conn_id = 0`).

Live snapshot reads (same envelope, `request_type` snake_case in the header):

| Method | Path | `request_type` | Reply |
|---|---|---|---|
| `get_open_orders()` | `POST /api/v1/openOrders` | `get_open_orders` | `open_orders_snapshot` |
| `get_positions()` | `POST /api/v1/positions` | `get_positions` | `positions_snapshot` |
| `get_account()` | `POST /api/v1/account` | `get_account` | `account_margin_update` |

## Testing

```bash
cargo test
```

Live tests: `GDX_LIVE_EDGE=1` plus API credentials, `--ignored`.
