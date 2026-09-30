# GoDark Rust SDK Reference

This reference describes the `godark` crate bundled with this repository at
the exact commit recorded in `sdk/UPSTREAM_REF`. The distribution supports two
client surfaces:

- `GodarkClient`: persistent HPKE-encrypted WebSocket trading and push streams.
- `GodarkRestClient`: bearer authentication, one-shot HPKE-encrypted REST
  commands and snapshots, plaintext authenticated order lookup, and public
  market-data snapshots.

The examples intentionally exercise `MARKET` and `LIMIT` placement only. Other
order variants in the public enum are protocol types, not a promise that every
example or environment accepts them.

## Identity: use account APIs

The canonical trading identity is `AccountId`, a 32-byte value displayed and
serialized as a Solana-style base58 string.

- After either client connects, use `client.account()` to read the authenticated
  identity.
- Use builder `.account(...)` or `GODARK_ACCOUNT` only as a fallback when a
  local/static-key authentication response and JWT both omit the account.
- `GodarkRestClient::get_account()` is not an identity accessor. It performs an
  encrypted account-margin snapshot request and returns `AccountMarginUpdate`.
- `GodarkRestClient::get_me()` calls the browser-session `/api/v1/auth/me`
  endpoint. API-key access tokens are normally rejected there; use
  `client.account()` for API-key identity.

The Rust public API does not expose a UUID-named identity compatibility alias,
so examples should not introduce one. Protocol fields that still have
settlement-specific legacy names are not renamed by this reference.

## Configuration

API key-pair authentication requires:

- `GODARK_API_KEY_ID`
- `GODARK_API_SECRET`
- `GODARK_PASSPHRASE`

The builders also accept the corresponding values directly. A legacy
single-token `.api_key(...)` mode exists for local/static-key setups and must
not be combined with a passphrase.

Useful overrides:

- `GODARK_EDGE_URL` / `GDX_EDGE_URL`: WebSocket edge URL; also converted from
  `ws(s)` to `http(s)` when the REST URL is not set.
- `GODARK_REST_URL` / `GDX_REST_URL`: explicit REST origin.
- `GDX_HPKE_STATIC_PUBLIC_KEY`: 64-hex sequencer HPKE pin. Accepted aliases are
  `GDX_HPKE_STATIC_PUBKEY`, `GODARK_HPKE_STATIC_PUBLIC_KEY`, and
  `VITE_GDX_HPKE_STATIC_PUBKEY`.
- `GODARK_ACCOUNT` / `GDX_ACCOUNT`: canonical account fallback.

REST has baked Testnet and Devnet HPKE pins. Localnet REST requires an explicit
pin. WebSocket configuration should provide the pin explicitly.

## WebSocket client

Construct and connect:

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

### Lifecycle and subscriptions

| Method | Purpose |
|---|---|
| `builder()` / `new(config)` | Configure and construct the client |
| `connect()` / `disconnect()` / `logout()` | Session lifecycle |
| `is_connected()` | Current connection state |
| `account()` | Canonical authenticated account |
| `subscribe(channels)` / `unsubscribe(channels)` | Manage edge subscriptions |

Take each receiver before `connect()`; each receiver is single-consumer:

- `take_order_receiver()`
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
`summary`. `PositionsSnapshot` is the authoritative positions collection.

### WebSocket commands

| Method | Supported behavior |
|---|---|
| `place_order(...)` | Encrypted placement with book confirmation |
| `place_order_with_confirmation(...)` | Select `Confirmation::Book` or fast `Confirmation::Ack` |
| `place_order_with_options(...)` | Placement with `PlaceOrderOptions` |
| `place_order_with_confirmation_and_options(...)` | Full confirmation and option control |
| `cancel_order(...)` / `modify_order(...)` | Single-order lifecycle |
| `update_leverage(...)` | Per-symbol leverage |
| `cancel_all_orders(symbol?)` | Cancel all globally or for one symbol |
| `close_all(symbol?)` | Reduce-only IOC close globally or for one symbol |
| `reverse_position(symbol)` | Flatten and open the opposite side |
| `amend_tpsl(...)` / `cancel_tpsl(...)` | Manage attached TP/SL |
| `mass_quote(...)` | Up to 20 quote legs |
| `batch_cancel(...)` / `batch_modify(...)` | Up to 20 order operations |

Prices and sizes on place / modify / mass-quote / batch-modify / TP-SL are
human **decimal strings** (`&str` / `String`), for example `"0.01"` or
`Some("67500.5")` — not `f64`. `PlaceOrderOptions` also carries `reduce_only`,
`post_only`, `stp_mode`, `quote_notional` (decimal string, XOR with base
quantity), `peg_offset_bps`, `trigger_price`, `take_profit_price`,
`stop_loss_price`, and `slippage_bps`. Slippage is expressed in basis points;
`None` delegates to the venue limit.

`Confirmation::Book` is the safe placement default. `Confirmation::Ack`
returns at the sequencer acknowledgement boundary, so callers must consume
order updates to observe a later reject or fill.

## REST client

`GodarkRestClient` is an implemented encrypted trading client, not a read-only
or plaintext-order fallback. Every command listed as encrypted below performs
a fresh one-shot HPKE setup and decrypts the node response in the SDK.

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

### Authentication and identity

| Method | Behavior |
|---|---|
| `connect()` | Obtains a bearer token, resolves canonical account, and loads instruments |
| `disconnect()` | Best-effort token revocation and local session reset |
| `account()` | Canonical authenticated account |
| `token_scope()` | Scope returned by token authentication |
| `get_me()` | Browser-session profile endpoint; generally not usable with API-key tokens |

### Encrypted REST commands

| Method | Route/behavior |
|---|---|
| `place_order(...)` | `POST /api/v1/orders`; supports optional client order id |
| `cancel_order(...)` | `DELETE /api/v1/orders/{order_id}` |
| `cancel_order_by_client_id(...)` | Resolve client id, then cancel by real order id |
| `modify_order(...)` | `PATCH /api/v1/orders/{order_id}` |
| `update_leverage(...)` | `POST /api/v1/leverage` |
| `get_open_orders()` | `POST /api/v1/openOrders` → `OpenOrdersSnapshot` |
| `get_positions()` | `POST /api/v1/positions` → `PositionsSnapshot` |
| `get_account()` | `POST /api/v1/account` → `AccountMarginUpdate` |
| `mass_quote(...)` | `POST /api/v1/orders/massQuote`, 1–20 legs |
| `batch_cancel(...)` | Encrypted batch command, 1–20 ids |
| `batch_modify(...)` | Encrypted post-only batch amend, 1–20 legs |

The REST `place_order(...)` signature supports the base placement fields plus
an optional `client_order_id`. It does not expose `PlaceOrderOptions`, explicit
confirmation selection, or `slippage_bps`.

### REST reads

| Method | Behavior |
|---|---|
| `get_order(order_id)` | Authenticated plaintext status lookup |
| `get_order_by_client_id(client_id)` | Authenticated plaintext lookup |
| `await_terminal_status(...)` | Poll lookup until filled, cancelled, or rejected |
| `get_leverage()` | Authenticated leverage snapshot |
| `get_funding_rates()` | Public funding-rate snapshot |
| `get_open_interest()` | Public open-interest snapshot |
| `get_volume()` | Public 24-hour volume snapshot |

Plaintext status and public snapshot reads do not mean order submission is
plaintext. REST placement, modification, cancellation, leverage update,
account/open-order/position snapshots, and supported batch commands are HPKE
encrypted.

### Explicit REST gaps

The bundled `GodarkRestClient` does not currently expose:

- `place_order_with_options` or a REST `slippage_bps` parameter
- `Confirmation::Book` / `Confirmation::Ack` selection
- cancel-all, close-all, reverse-position, amend-TP/SL, or cancel-TP/SL wrappers
- WebSocket subscriptions, push receivers, or reconnect events
- typed return models for order lookup and public funding/open-interest/volume
  reads (these return `serde_json::Value`)

Do not infer SDK support merely because an edge route exists.

## Main public types

- Identity: `AccountId`
- Placement: `OrderAck`, `Confirmation`, `PlaceOrderOptions`
- Orders: `OrderUpdate`, `OrderStatus`, `OrderUpdateType`, `CancelReason`
- Positions/account: `PositionRow`, `PositionsSnapshot`,
  `PositionsSnapshotSource`, `BalanceUpdate`, `AccountMarginUpdate`,
  `AccountMarginSummary`, `LeverageSettings`
- Batch/MM: `MassQuoteLegInput`, `MassQuoteAck`, `BatchCancelAck`,
  `BatchModifyLegInput`, `BatchModifyAck`, `CountAck`, `TpslAck`
- Enums: `Side`, `OrderType`, `TimeInForce`, `StpMode`

All fallible operations return `GodarkError`. Order rejects use
`GodarkError::Order { message, error_code, user_message }`; the symbolic error
catalog is available through `find_order_error` and `ORDER_ERROR_CODES`.

## Examples and package boundary

- `examples/quickstart.rs`: minimal encrypted WebSocket place/cancel.
- `examples/full_trader_example.rs`: WebSocket callbacks and advanced commands.
- `examples/full_trader_rest.rs`: repository-only full encrypted REST flow.
- `examples/rest_client_example.rs`: bundle-safe REST auth and account-oriented
  reads.

The release bundle intentionally ships `rest_client_example.rs`, but not
`full_trader_rest.rs`. This is an example-packaging gap, not an SDK capability
gap.

Build all repository examples with:

```bash
cargo build --release --examples
```

## Maintainer pin and parity contract

`sdk/UPSTREAM_REF` is the exact upstream `gdx-rust-sdk` commit used by this
distribution. `scripts/package.sh` verifies that the sibling upstream checkout
is exactly at that pin and that the compiled bundled source matches it (apart
from the documented market-data trim) before creating a ZIP.

Do not hand-edit `sdk/src`. Refresh from the sibling SDK, update the exact pin,
then run formatting, linting, tests, release build, and package parity checks.
