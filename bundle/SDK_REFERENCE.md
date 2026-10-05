# GoDark Rust SDK Reference

The bundled `godark` crate provides two client surfaces:

- `GodarkClient`: persistent HPKE-encrypted WebSocket trading and push streams.
- `GodarkRestClient`: bearer authentication, one-shot HPKE-encrypted REST
  commands and snapshots, plaintext authenticated order lookup, and public
  market-data snapshots.

The examples place post-only `LIMIT` orders priced from a live mark, and exit
before sending if that mark is missing. They do not place `MARKET` orders.
Other order variants in the public enum are protocol types, not a promise that
every environment accepts them.

## Canonical account identity

The canonical trading identity is `AccountId`, a 32-byte value displayed and
serialized as a Solana-style base58 string.

- After either client connects, use `client.account()`.
- Use builder `.account(...)` or `GODARK_ACCOUNT` only as a local/static-key
  fallback when authentication does not supply the account.
- `GodarkRestClient::get_account()` returns an encrypted account-margin
  snapshot; it is not the identity accessor.
- `GodarkRestClient::get_me()` is a browser-session profile endpoint. API-key
  access tokens are normally rejected there; use `account()` instead.

The Rust API has no UUID-named identity alias. Protocol fields that retain
settlement-specific legacy names are unchanged.

## Configuration

API key-pair authentication requires `GODARK_API_KEY_ID`,
`GODARK_API_SECRET`, and `GODARK_PASSPHRASE`.

Optional overrides:

- `GODARK_EDGE_URL` / `GDX_EDGE_URL`: WebSocket edge URL and REST fallback.
- `GODARK_REST_URL` / `GDX_REST_URL`: explicit REST origin.
- `GDX_HPKE_STATIC_PUBLIC_KEY`: 64-hex sequencer HPKE pin. Aliases:
  `GDX_HPKE_STATIC_PUBKEY`, `GODARK_HPKE_STATIC_PUBLIC_KEY`,
  `VITE_GDX_HPKE_STATIC_PUBKEY`.
- `GODARK_ACCOUNT` / `GDX_ACCOUNT`: canonical account fallback.

REST has baked Testnet and Devnet pins; Localnet REST requires an explicit pin.

## `GodarkClient` (WebSocket)

```rust
use godark::{Environment, GodarkClient};

let config = GodarkClient::builder()
    .environment(Environment::Testnet)
    .api_key_id(std::env::var("GODARK_API_KEY_ID")?)
    .api_secret(std::env::var("GODARK_API_SECRET")?)
    .passphrase(std::env::var("GODARK_PASSPHRASE")?)
    .build()?;

let mut client = GodarkClient::new(config);
client.connect().await?;
println!("account={}", client.account().expect("account after connect"));
```

`connect()` sends the REST `access_token` (`POST /api/v1/auth/token`,
`client_credentials`) as the `/ws/v1` login. It does not send
`key_id:secret:passphrase`.

Channels: `orders`, `positions`, `volume`, `open_interest`, `funding_rate`.
No `trades` or L2 channel on `/ws/v1`. Take
`take_open_orders_snapshot_receiver()` before `connect()`;
`open_orders_snapshot` is delivered on that receiver.

Lifecycle and control:

- `builder()`, `new(config)`, `connect()`, `disconnect()`, `logout()`
- `is_connected()`, `account()`
- `subscribe(channels)`, `unsubscribe(channels)`

Take each single-consumer receiver before `connect()`:

- `take_order_receiver()`
- `take_open_orders_snapshot_receiver()`
- `take_positions_snapshot_receiver()`
- `take_system_health_receiver()`
- `take_balance_receiver()`
- `take_account_margin_receiver()`
- `take_leverage_settings_receiver()`
- `take_funding_rate_receiver()`
- `take_error_receiver()`
- `take_reconnect_receiver()`

`BalanceUpdate` is the collateral-balance stream. `AccountMarginUpdate`
contains the canonical `account` separately from its optional margin
`summary`.

WebSocket commands:

- Placement: `place_order`, `place_order_with_confirmation`,
  `place_order_with_options`, `place_order_with_confirmation_and_options`
- Single order: `cancel_order`, `modify_order`
- Account/position commands: `update_leverage`, `cancel_all_orders`,
  `close_all`, `reverse_position`, `amend_tpsl`, `cancel_tpsl`
- MM/batch commands: `mass_quote`, `batch_cancel`, `batch_modify` (up to 20
  legs or ids)

Prices and sizes on place / modify / mass-quote / batch-modify / TP-SL are
human **decimal strings only** (`&str` / `String`) — not `f64` / `f32` /
integers. Invalid strings are rejected before sealing.
`PlaceOrderOptions` includes `reduce_only`, `post_only`, `stp_mode`,
`quote_notional` (decimal string), `peg_offset_bps`, `trigger_price`,
`take_profit_price`, `stop_loss_price`, `slippage_bps`, and `client_order_id`.
`slippage_bps` applies only to `MARKET` and `STOP_MARKET` (`None` uses the
venue limit). Peg (`peg_offset_bps`) is not post-only.

A client order id is registered only after a successful WebSocket place, and
cached only after `_register_coid` returns HTTP 200. REST place does not
register it.

`Confirmation::Book` waits beyond the fast acknowledgement for a matching
order update. `Confirmation::Ack` returns at the sequencer acknowledgement
boundary; callers must consume updates for later rejects and fills.

## `GodarkRestClient`

Encrypted REST trading is supported. Each encrypted command performs a fresh
one-shot HPKE setup and decrypts the node response inside the SDK.

```rust
use godark::{Environment, GodarkRestClient};

let mut client = GodarkRestClient::builder()
    .environment(Environment::Testnet)
    .api_key_id(std::env::var("GODARK_API_KEY_ID")?)
    .api_secret(std::env::var("GODARK_API_SECRET")?)
    .passphrase(std::env::var("GODARK_PASSPHRASE")?)
    .build()?;

client.connect().await?;
println!("account={}", client.account().expect("account after connect"));
let account_margin = client.get_account().await?;
```

Authentication and identity:

- `connect()`, `disconnect()`
- `account()` — canonical authenticated account
- `token_scope()`
- `get_me()` — browser-session profile endpoint, generally unavailable to
  API-key tokens

Encrypted REST methods:

| Method | Behavior |
|---|---|
| `place_order(...)` | Place; optional client order id is not registered |
| `cancel_order(...)` | Cancel by server order id |
| `cancel_order_by_client_id(...)` | Resolve client id, then cancel |
| `modify_order(...)` | Modify price, quantity, and/or trigger |
| `update_leverage(...)` | Update per-symbol leverage |
| `get_open_orders()` | Typed `OpenOrdersSnapshot` |
| `get_positions()` | Typed `PositionsSnapshot` |
| `get_account()` | Typed `AccountMarginUpdate` |
| `mass_quote(...)` | 1–20 quote legs |
| `batch_cancel(...)` | 1–20 order ids |
| `batch_modify(...)` | 1–20 post-only amendments |

REST reads:

- `get_order(order_id)` and `get_order_by_client_id(client_id)` perform
  authenticated plaintext status lookups.
- `await_terminal_status(...)` polls until filled, cancelled, or rejected.
- `get_leverage()` returns typed leverage settings.
- `get_funding_rates()`, `get_open_interest()`, and `get_volume()` return
  public `serde_json::Value` snapshots.

Plaintext lookup does not mean order flow is plaintext. Placement,
modification, cancellation, leverage update, account/open-order/position
snapshots, and the supported batch commands are HPKE encrypted.

### Explicit REST gaps

`GodarkRestClient` does not currently expose:

- `place_order_with_options` or REST `slippage_bps`
- explicit `Confirmation::Book` / `Confirmation::Ack`
- cancel-all, close-all, reverse-position, amend-TP/SL, or cancel-TP/SL
- subscriptions, push receivers, or reconnect events
- typed order-lookup or public funding/open-interest/volume models

Do not infer SDK support merely because an edge route exists.

## Main public types

- Identity: `AccountId`
- Placement: `OrderAck`, `Confirmation`, `PlaceOrderOptions`
- Orders: `OrderUpdate`, `OrderStatus`, `OrderUpdateType`, `CancelReason`
- Positions/account: `PositionRow`, `PositionsSnapshot`, `BalanceUpdate`,
  `AccountMarginUpdate`, `AccountMarginSummary`, `LeverageSettings`
- Batch/MM: `MassQuoteLegInput`, `MassQuoteAck`, `BatchCancelAck`,
  `BatchModifyLegInput`, `BatchModifyAck`, `CountAck`, `TpslAck`
- Enums: `Side`, `OrderType`, `TimeInForce`, `StpMode`

All fallible calls return `GodarkError`. Order rejects use
`GodarkError::Order { message, error_code, user_message }`.

## Included examples

- `quickstart.rs`: minimal encrypted WebSocket place/cancel.
- `full_trader_example.rs`: WebSocket callbacks and advanced commands.
- `rest_client_example.rs`: REST authentication and account-oriented reads.
- `dotenv.rs`: shared environment loader and error printer.

The crate supports encrypted REST placement, modification, and cancellation,
although the package includes only the smaller REST read example.

Build from the package root:

```bash
cargo build --release --examples
```
