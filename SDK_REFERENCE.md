# GoDark Rust SDK Reference (developer / maintainer)

This is the comprehensive reference for maintainers and developers working
*inside* this repository (writing examples, reviewing the vendored `sdk/`,
refreshing pins, etc.).

A trimmed, recipient-facing copy is maintained at
[`bundle/SDK_REFERENCE.md`](bundle/SDK_REFERENCE.md) and is the one copied
into the root of released ZIP bundles as `SDK_REFERENCE.md`. The bundle
version intentionally omits sections that recipients don't need (refresh /
parity / pin discipline, error-code internals, forward-compat enum strategy,
crate sourcing options).

> Scope: the MM examples use **WebSocket encrypted trading** via
> `godark::GodarkClient`. Encrypted REST trading is not supported — all order
> flow (place / modify / cancel / mass-quote) runs over the HPKE WebSocket
> client. Standalone market-data surfaces are
> intentionally excluded from this distribution. Order placement support
> is limited to `MARKET` and `LIMIT`.

## Quick Start

```rust
use godark::{GodarkClient, OrderType, Side, TimeInForce};

#[tokio::main]
async fn main() -> Result<(), godark::GodarkError> {
    let config = GodarkClient::builder()
        .base_url("wss://api.godark-dex.com")  // optional override
        .api_key_id(std::env::var("GODARK_API_KEY_ID").unwrap())
        .api_secret(std::env::var("GODARK_API_SECRET").unwrap())
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

## Configuration

The MM examples expect:

- `GODARK_API_KEY_ID` (required)
- `GODARK_API_SECRET` (required)
- `GODARK_PASSPHRASE` (required for API key-pair auth)
- `GDX_HPKE_STATIC_PUBLIC_KEY` (required for encrypted WebSocket trading) — 64 hex chars; aliases `GDX_HPKE_STATIC_PUBKEY`, `GODARK_HPKE_STATIC_PUBLIC_KEY`. Or set `hpke_static_public_key_hex` on `GodarkClient::builder()`.
- `GODARK_EDGE_URL` (optional, defaults to `wss://api.godark-dex.com`)

Use `.env.example` as the template for your local `.env`. The shared helper
`examples/dotenv.rs` (`load_dotenv` + `print_order_error`) is reused by both
example binaries.

### WebSocket transport defaults

Pass custom values via `.transport(TransportConfig { ... })` on
`GodarkClient::builder()`. Defaults:

| Field | Default | Purpose |
|-------|---------|---------|
| `heartbeat_interval` | `30s` | Ping interval |
| `stale_timeout` | `120s` | Absolute cap with no inbound traffic |
| `missed_heartbeat_limit` | `2` | Consecutive missed ping intervals before stale close |
| `auto_reconnect` | `true` | Reconnect after unexpected disconnect; manual `disconnect()` does not |

On stale disconnect the SDK emits a non-fatal `GodarkError::Connection` on
`take_error_receiver()` (message contains `stale heartbeat`), then
`take_reconnect_receiver()` events (`Disconnected`, `Attempting`, `Reconnected`).

## Installing the SDK

In this repository, the example binaries depend on the vendored crate via a
relative path:

```toml
# Cargo.toml
[dependencies]
godark = { path = "sdk" }
```

The vendored `sdk/` ships pre-generated protobuf bindings under
`sdk/src/generated/`, so consumers do **not** need `protoc` or `prost-build`
installed.

To consume `godark` from your own project outside this repo, either:

1. Copy the vendored `sdk/` directory and depend on it by path (the same way
   this repo does), or
2. Depend on the public upstream crate by git URL pinned to the SHA recorded
   in [`sdk/UPSTREAM_REF`](sdk/UPSTREAM_REF):

   ```toml
   godark = { git = "https://github.com/gq-godark/gdx-rust-sdk.git", rev = "<sha from sdk/UPSTREAM_REF>" }
   ```

   Note that consuming `gdx-rust-sdk` directly (option 2) builds the SDK
   from its own source tree, which pulls `gdx-proto` as a recursive
   submodule and re-runs `prost-build`; you'll need `protoc` available.

## GodarkClient API

**Crate:** `godark` (vendored under `sdk/` in this repo; upstream at
[`gq-godark/gdx-rust-sdk`](https://github.com/gq-godark/gdx-rust-sdk)).

### Core lifecycle

| Method | Signature | Purpose |
|--------|-----------|---------|
| `builder` | `GodarkClient::builder() -> GodarkConfigBuilder` | Start a new client config |
| `new` | `GodarkClient::new(config) -> GodarkClient` | Construct the client |
| `connect` | `async fn connect(&mut self) -> Result<(), GodarkError>` | Authenticate and establish HPKE WebSocket session |
| `disconnect` | `async fn disconnect(&mut self)` | Graceful disconnect |
| `is_connected` | `fn is_connected(&self) -> bool` | Connection state |
| `user_uuid` | `fn user_uuid(&self) -> Option<&Uuid>` | Authenticated user id |

### Trading commands

| Method | Signature (abridged) | Purpose |
|--------|----------------------|---------|
| `place_order` | `async fn place_order(symbol, side, order_type, quantity, price?, tif, post_only, ...) -> Result<OrderAck>` | Place encrypted order |
| `update_leverage` | `async fn update_leverage(symbol, leverage) -> Result<OrderAck>` | Set per-symbol account leverage |
| `cancel_order` | `async fn cancel_order(order_id, symbol) -> Result<OrderAck>` | Cancel order |
| `modify_order` | `async fn modify_order(order_id, symbol, new_price?, new_quantity?, new_trigger_price?) -> Result<OrderAck>` | Modify price, quantity, and/or stop trigger |
| `mass_quote` | `async fn mass_quote(symbol, legs, post_only?) -> Result<MassQuoteAck>` | Bulk cancel-replace ladder |

### Subscriptions

| Method | Purpose |
|--------|---------|
| `subscribe(&["orders", "positions"])` | Subscribe to private channels |
| `unsubscribe(&[...])` | Unsubscribe |

### Receivers (channels)

The SDK exposes one `tokio::sync::mpsc::Receiver<T>` per push stream. Each
receiver is **single-consumer**: take it **before** calling `connect()`.
After `connect()`, the transport task starts forwarding into the channel
buffers — if the receiver hasn't been taken by then, pushes for that stream
will fill the buffer and back-pressure the dispatcher.

| Method | Receiver type | Stream |
|--------|---------------|--------|
| `take_order_receiver()` | `Receiver<OrderUpdate>` | Order lifecycle |
| `take_position_receiver()` | `Receiver<PositionUpdate>` | Per-fill position deltas |
| `take_positions_snapshot_receiver()` | `Receiver<PositionsSnapshot>` | Initial / periodic / event-triggered snapshots |
| `take_system_health_receiver()` | `Receiver<SystemHealthUpdate>` | Sequencer / MPC node cluster pulses |
| `take_balance_receiver()` | `Receiver<BalanceUpdate>` | Updated shielded balance |
| `take_margin_alert_receiver()` | `Receiver<MarginAlert>` | Margin tier transition / recovery |
| `take_funding_rate_receiver()` | `Receiver<FundingRateUpdate>` | Per-symbol funding ticks |
| `take_settlement_receiver()` | `Receiver<SettlementUpdate>` | Settlement batch lifecycle |
| `take_error_receiver()` | `Receiver<GodarkError>` | Non-fatal SDK errors (stale heartbeat, decrypt failures, …) |
| `take_reconnect_receiver()` | `Receiver<ReconnectEvent>` | Disconnect / backoff / reconnected lifecycle |

### Push-payload reference

| Push                  | Field highlights                                                                                | Typical use                                                                          |
|-----------------------|-------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------|
| `PositionsSnapshot`   | `rows[]` (`PositionRow{symbol_id, side, size, entry_price, mark_price, unrealized_pnl, ...}`), `source` (Initial / Periodic / Event) | Hydrate the open-positions table on connect; refresh every ~5s.                      |
| `SystemHealthUpdate`  | `total_nodes`, `ready`, `degraded`, `accepting_orders`                                          | Display node-cluster status; pause submissions if `accepting_orders == false`.       |
| `BalanceUpdate`       | `shielded_balance_raw` (raw lamports-style integer)                                             | Refresh the wallet/equity widget after each fill or settlement.                      |
| `MarginAlert`         | `owner`, `symbol_id`, `tier`, `margin_ratio_bps`, `liquidation_price_bps`, `recovered`          | Show / clear the margin-tier banner per `(owner, symbol_id)`.                        |
| `FundingRateUpdate`   | `symbol_id`, `current_rate`, `predicted_rate`, `next_funding_time`                              | Update funding ticker / book metadata.                                               |
| `SettlementUpdate`    | `batch_id`, `status` (Submitted / Confirmed / Failed), `tx_signature`, `affected_user_uuids[]`  | Reconcile settled batches, surface Solana tx links.                                  |

### Concurrency rule

Only one command (`place_order`, `cancel_order`, `modify_order`) should be
in flight at a time. Call these sequentially. The push receivers above may
be consumed concurrently from independent tasks — that's the intended
pattern in `full_trader_example.rs`.

## Core Types

### OrderAck

| Field | Type | Notes |
|-------|------|-------|
| `order_id` | `String` | Server-assigned id; use for subsequent `cancel`/`modify` |
| `success` | `bool` | False ⇒ order was rejected; inspect `error_code` and `error` |
| `sequence` | `String` | Sequencer ack ordering token |
| `error_code` | `Option<String>` | Symbolic code, e.g. `"PRICE_DEVIATION_TOO_LARGE"` |
| `error` | `Option<String>` | Human-readable message |

### OrderUpdate

| Field | Type | Notes |
|-------|------|-------|
| `order_id`, `user_uuid`, `symbol_id` | identifiers | — |
| `side` | `Side` | `Buy` / `Sell` |
| `status`, `update_type` | `OrderStatus`, `OrderUpdateType` | Final state vs. lifecycle event |
| `price`, `quantity`, `filled_qty`, `remaining_qty`, `cum_fill` | `String` (decimal) | Stringly-typed decimals to preserve precision |
| `cancel_reason` | `Option<CancelReason>` | Set on cancels |
| `reject_reason` | `Option<String>` | Set on `Rejected` updates |
| `correlation_id` | `u128` | Echoes the client-side request id |
| `timestamp` | `u64` | Server-side event time (epoch nanos) |

### PositionUpdate

Per-fill delta. Use this stream to drive incremental P&L / position
accounting between snapshot refreshes.

| Field | Type |
|-------|------|
| `user_uuid`, `symbol_id`, `side` | identifiers |
| `update_type` | `PositionUpdateType` |
| `size`, `entry_price`, `previous_size`, `fill_price`, `fill_qty` | `String` (decimal) |
| `correlation_id`, `timestamp` | `u128`, `u64` |

### PositionRow / PositionsSnapshot

`PositionsSnapshot` is the periodic/event-triggered authoritative view of
all open positions for the authenticated user. `rows` holds one
`PositionRow` per `(symbol_id, side)` pair:

| `PositionRow` field | Type | Notes |
|---------------------|------|-------|
| `symbol_id`, `side`, `size`, `entry_price`, `leverage` | required | — |
| `mark_price`, `unrealized_pnl`, `notional` | `Option<String>` | Server may omit on stale rows |
| `mark_publish_time_sec` | `Option<u64>` | Last mark refresh per row |

`PositionsSnapshot` additionally carries `source: PositionsSnapshotSource`
(`Initial` / `Periodic` / `Event`) and an optional `correlation_id`.

### Other push payloads

| Type | Notable fields |
|------|----------------|
| `SystemHealthUpdate` | `total_nodes`, `accepting_orders`, `ready`, `degraded`, `exhausted`, `warming`, `draining`, `waiting` |
| `BalanceUpdate` | `user_uuid`, `shielded_balance_raw`, `timestamp` |
| `MarginAlert` | `owner`, `symbol_id`, `tier`, `margin_ratio_bps`, `mark_price_bps`, `liquidation_price_bps`, `state_version`, `recovered`, `ts` |
| `FundingRateUpdate` | `symbol_id`, `current_rate`, `predicted_rate`, `next_funding_time`, `timestamp` |
| `SettlementUpdate` | `batch_id`, `status: SettlementBatchStatus`, `tx_signature`, `timestamp`, `affected_user_uuids` |

## Enums

All enums in the public API derive `Debug`, `Clone`, `Copy`,
`PartialEq`/`Eq` (with `Hash` where useful):

- `Side`: `Buy`, `Sell`
- `OrderType`: `Market`, `Limit`, `PegToMid`, `PegToBid`, `PegToAsk`
- `TimeInForce`: `Gtc`, `Ioc`, `Fok`, `Gtd`
- `OrderStatus`: `New`, `PartiallyFilled`, `Filled`, `Cancelled`, `Rejected`
- `OrderUpdateType`: `Open`, `Filled`, `PartiallyFilled`, `Cancelled`, `Rejected`, `Modified`, `CancelRejected`, `ModifyRejected`
- `PositionUpdateType`: `Snapshot`, `Open`, `Increase`, `Decrease`, `Close`
- `CancelReason`: `UserRequested`, `IocRemainder`, `FokNotFilled`, `Expired`, `System`
- `PositionsSnapshotSource`: `Unspecified`, `Initial`, `Periodic`, `Event`
- `SettlementBatchStatus`: `Unspecified`, `Submitted`, `Confirmed`, `Failed`

`OrderType` includes the `PegTo*` variants for API completeness, but this MM
distribution only exercises `Market` and `Limit` from the examples.

## Errors

### GodarkError variants

`GodarkError` is the single error type returned from every fallible SDK
call:

| Variant | When |
|---------|------|
| `Authentication(String)` | API key rejection at session bring-up |
| `Session(String)` | HPKE setup handshake or rekey failure |
| `Order { message, error_code }` | Order rejected by the sequencer; `error_code` carries the symbolic reason (see below) |
| `Connection(String)` | Transport-level failure |
| `Encryption(String)` | Cipher / nonce failure on encrypted payloads |
| `Timeout(String)` | Per-command response timeout |
| `Config(String)` | Builder / configuration validation |
| `WebSocket(tokio_tungstenite::tungstenite::Error)` | Raw WS error (transparent) |
| `Proto(prost::DecodeError)` | Malformed proto frame (transparent) |

The `Order` variant is the one application code typically branches on:

```rust
match client.place_order(...).await {
    Ok(ack) if ack.success => { /* placed */ }
    Ok(ack) => print_order_error(ack.error_code.as_deref(), ack.error.as_deref()),
    Err(godark::GodarkError::Order { error_code, message }) => {
        eprintln!("rejected: {message} (code={error_code:?})");
    }
    Err(e) => return Err(e),
}
```

### Order error codes

The sequencer's numeric ack codes are mapped to symbolic strings (e.g.
`PRICE_DEVIATION_TOO_LARGE`, `MARGIN_INSUFFICIENT`,
`SELF_TRADE_PREVENTION`) by the `godark::order_error_code` module. The
following items are re-exported at the crate root:

| Item | Purpose |
|------|---------|
| `find_order_error(code: u16) -> Option<&'static OrderErrorEntry>` | Lookup by numeric code |
| `OrderErrorEntry` | `{ code, symbolic, description }` |
| `ORDER_ERROR_CODES` | Static slice of every known entry, useful for tests / docs generation |

The `OrderAck::error_code` and `GodarkError::Order { error_code }` fields
already carry the symbolic string, so most callers won't need the lookup
table directly — it's primarily there for debugging and for renderers that
want to surface the long-form description alongside the symbolic name.

## Forward compatibility: `EdgeMessage`

`EdgeMessage` (in `godark::proto_bridge`) is the internal enum that
`SequencerToEdgeMessage` variants are folded into before dispatch to the
public push channels. It is marked `#[non_exhaustive]` and carries an
explicit `Unknown` variant.

When the upstream proto schema gains a new `SequencerToEdgeMessage::Inner`
variant, the SDK maps it to `EdgeMessage::Unknown` rather than panicking or
silently dropping the frame. This means:

- Adding a new proto variant on the server is **non-breaking** for SDK
  consumers — they continue to receive every push they previously
  subscribed to.
- Adopting the new variant in your application requires a vendored-SDK
  refresh (via `scripts/refresh_sdk.sh`) and a corresponding `match`-arm
  update where you consume push receivers.
- The `#[non_exhaustive]` attribute also forces downstream `match`
  expressions on `EdgeMessage` to include a wildcard arm, which keeps
  downstream code from breaking the moment a new variant is added even
  before the consumer updates.

The same forward-compat strategy applies to `NodeResponseKind::Unknown` for
sequencer command acks.

## Example files in this distribution

| File | Purpose |
|------|---------|
| `examples/quickstart.rs` | Minimal connect, place, cancel |
| `examples/full_trader_example.rs` | Reference bot flow: callbacks, place / modify / cancel, mass-quote / batch-cancel |
| `examples/dotenv.rs` | Shared helper (`load_dotenv` + `print_order_error`) |

## SDK source layout

The `godark` SDK is vendored under `sdk/`:

```text
sdk/
├── UPSTREAM_REF        # exact upstream commit SHA the vendored copy was cut from
├── Cargo.toml          # godark crate manifest (no [build-dependencies])
├── README.md           # SDK README copied from upstream
├── shared/symbols.json # canonical perp symbol table baked in via include_str!
└── src/
    ├── lib.rs          # public crate root (with `pub mod market_data;` etc. removed)
    ├── client.rs, transport.rs, session.rs, ...
    └── generated/      # PRE-GENERATED protobuf bindings (no protoc required)
        ├── mod.rs
        ├── gdx.common.v1.rs
        ├── gdx.edge.v1.rs
        └── gdx.sequencer.v1.rs
```

`sdk/src/lib.rs` differs from upstream's `lib.rs` only in that the
`market_data`, `rest_client`, and `rest_transport` module declarations (and
their `pub use` re-exports) are stripped: those source files are excluded
from the vendored copy because this distribution covers WebSocket encrypted
trading only. The packaging script verifies this trim is deterministic on
every release run.

## Refreshing the vendored SDK

Maintainers refresh `sdk/` from a sibling `gdx-rust-sdk` checkout:

```bash
./scripts/refresh_sdk.sh /path/to/gdx-rust-sdk
```

The script:

1. Refuses to run if the upstream worktree is dirty (so the recorded SHA
   matches reality).
2. Rsyncs upstream's crate, dropping the excluded source files.
3. Rewrites `sdk/src/lib.rs` to drop the corresponding module declarations.
4. Strips dev/build/example dependencies from `sdk/Cargo.toml` and adds
   `autoexamples = false`, `autotests = false`, `autobenches = false`.
5. Writes the upstream HEAD SHA into `sdk/UPSTREAM_REF`.

After running it, `scripts/package.sh` performs a parity check between the
vendored `sdk/` and a freshly-built install at the pinned SHA; any drift
fails the release. Layer 2 automation (`auto-bump-sdk-pin.yml`) wraps this
loop into a rolling auto-PR triggered by SDK pushes.

## RestClient example

`GodarkRestClient` is exercised by `rest_client_example` / `rest-client-example`: REST auth, `/auth/me`, leverage read, and public funding/OI/volume GETs. Encrypted place/cancel/modify/update-leverage remain WebSocket-only via `GodarkClient`.
