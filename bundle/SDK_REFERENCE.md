# GoDark Rust SDK Reference (MM Distribution)

This reference describes the API surface used by the two prebuilt examples
shipped in this distribution. The examples use WebSocket encrypted trading
via `godark::GodarkClient`. REST and standalone market-data surfaces are
intentionally excluded.

Order placement support in this MM distribution is limited to `MARKET` and
`LIMIT`.

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
- `GODARK_EDGE_URL` (optional, defaults to `wss://api.godark-dex.com`)

Use `.env.example` as the template for your local `.env`.

## GodarkClient API

**Crate:** `godark` (statically linked into each example binary in this
distribution; also available under `sdk/` for path-dependency builds).

### Core lifecycle

| Method | Signature | Purpose |
|--------|-----------|---------|
| `builder` | `GodarkClient::builder() -> ConfigBuilder` | Start a new client config |
| `new` | `GodarkClient::new(config) -> GodarkClient` | Construct the client |
| `connect` | `async fn connect(&mut self) -> Result<(), GodarkError>` | Authenticate and establish encrypted session |
| `disconnect` | `async fn disconnect(&mut self)` | Graceful disconnect |
| `is_connected` | `fn is_connected(&self) -> bool` | Connection state |
| `user_uuid` | `fn user_uuid(&self) -> Option<&Uuid>` | Authenticated user id |

### Trading commands

| Method | Signature (abridged) | Purpose |
|--------|----------------------|---------|
| `place_order` | `async fn place_order(symbol, side, order_type, quantity, price?, tif, post_only, ...) -> Result<OrderAck>` | Place encrypted order |
| `cancel_order` | `async fn cancel_order(order_id, symbol) -> Result<OrderAck>` | Cancel order |
| `modify_order` | `async fn modify_order(order_id, symbol, new_price?, new_quantity?) -> Result<OrderAck>` | Modify order |

### Subscriptions

| Method | Purpose |
|--------|---------|
| `subscribe(&["orders", "positions"])` | Subscribe to private channels |
| `unsubscribe(&[...])` | Unsubscribe |

### Receivers (channels)

The SDK exposes one `tokio::sync::mpsc::Receiver<T>` per push stream. Take
each one **before** calling `connect()` (single-consumer):

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
| `take_error_receiver()` | `Receiver<GodarkError>` | Non-fatal SDK errors |

| Push                  | Field highlights                                                                                | Typical use                                                                          |
|-----------------------|-------------------------------------------------------------------------------------------------|--------------------------------------------------------------------------------------|
| `PositionsSnapshot`   | `rows[]` (`PositionRow{symbol_id, side, size, entry_price, mark_price, unrealized_pnl, ...}`), `source` (Initial / Periodic / Event) | Hydrate the open-positions table on connect; refresh every ~5s.                      |
| `SystemHealthUpdate`  | `total_nodes`, `ready`, `degraded`, `accepting_orders`                                          | Display node-cluster status; pause submissions if `accepting_orders == false`.       |
| `BalanceUpdate`       | `shielded_balance_raw` (raw lamports-style integer)                                             | Refresh the wallet/equity widget after each fill or settlement.                      |
| `MarginAlert`         | `symbol_id`, `tier`, `margin_ratio_bps`, `liquidation_price_bps`, `recovered`                   | Show / clear the margin-tier banner per `(owner, symbol_id)`.                        |
| `FundingRateUpdate`   | `symbol_id`, `current_rate`, `predicted_rate`, `next_funding_time`                              | Update funding ticker / book metadata.                                               |
| `SettlementUpdate`    | `batch_id`, `status` (Submitted / Confirmed / Failed), `tx_signature`, `affected_user_uuids[]`  | Reconcile settled batches, surface Solana tx links.                                  |

### Concurrency rule

Only one command (`place_order`, `cancel_order`, `modify_order`) should be in
flight at a time. Call these sequentially.

## Core Types

| Type | Notable fields |
|------|----------------|
| `OrderAck` | `order_id`, `success`, `sequence`, `error_code: Option<String>`, `error: Option<String>` |
| `OrderUpdate` | `order_id`, `symbol_id`, `side`, `status`, `update_type`, `price`, `quantity`, `filled_qty`, `remaining_qty`, `cum_fill`, `cancel_reason`, `reject_reason_code`, `correlation_id`, `timestamp` |
| `PositionUpdate` | `user_uuid`, `symbol_id`, `side`, `update_type`, `size`, `entry_price`, `previous_size`, `fill_price`, `fill_qty`, `correlation_id`, `timestamp` |
| `PositionsSnapshot` | `user_uuid`, `rows: Vec<PositionRow>`, `server_timestamp`, `source: PositionsSnapshotSource`, `correlation_id` |

## Enums

Important enums used in MM examples (all implement `Debug`):

- `Side`: `Buy`, `Sell`
- `OrderType`: `Market`, `Limit`, `PegToMid`, `PegToBid`, `PegToAsk`
- `TimeInForce`: `Gtc`, `Ioc`, `Fok`, `Gtd`
- `OrderStatus`: `New`, `PartiallyFilled`, `Filled`, `Cancelled`, `Rejected`
- `OrderUpdateType`: `Open`, `Filled`, `PartiallyFilled`, `Cancelled`, `Rejected`, `Modified`, `CancelRejected`, `ModifyRejected`
- `PositionUpdateType`: `Snapshot`, `Open`, `Increase`, `Decrease`, `Close`
- `CancelReason`: `UserRequested`, `IocRemainder`, `FokNotFilled`, `Expired`, `System`
- `PositionsSnapshotSource`: `Unspecified`, `Initial`, `Periodic`, `Event`
- `SettlementBatchStatus`: `Unspecified`, `Submitted`, `Confirmed`, `Failed`

Note: the SDK enum includes additional order types for compatibility, but this
MM distribution supports placing only `Market` and `Limit` orders.

## Errors

`GodarkError` is the single error type returned from every fallible SDK call:

- `Authentication(String)`
- `Session(String)`
- `Order { message: String, error_code: Option<String> }`
  — also carries the symbolic reason (e.g. `"PRICE_DEVIATION_TOO_LARGE"`,
  `"MARGIN_INSUFFICIENT"`). See the `quickstart` source for the match-and-print
  pattern.
- `Connection(String)`
- `Encryption(String)`
- `Timeout(String)`
- `Config(String)`

## Example files in this distribution

| File | Built binary | Purpose |
|------|--------------|---------|
| `examples/quickstart.rs` | `./quickstart` | Minimal connect, place, cancel |
| `examples/full_trader_example.rs` | `./full_trader_example` | Reference bot flow with all 6 push callbacks (positions snapshot, system health, balance, margin, funding rate, settlement) |
| `examples/dotenv.rs` | (helper) | Shared `.env` loader and symbolic-error printer used by both example mains |

Both prebuilt binaries are Linux x86_64 ELFs built against the bundled
`sdk/`. To rebuild from the included sources, run
`cargo build --release --examples` from the bundle root.

## Cargo integration (your own bot)

The bundle includes a bundled `godark` crate under `sdk/`. Depend on it
via a path dependency from your own `Cargo.toml`:

```toml
# Cargo.toml — your own bot
[dependencies]
godark  = { path = "path/to/this-bundle/sdk" }
tokio   = { version = "1", features = ["rt-multi-thread", "macros", "time", "sync"] }
dotenvy = "0.15"
```

Then in `src/main.rs`:

```rust
use godark::{GodarkClient, OrderType, Side, TimeInForce};

#[tokio::main]
async fn main() -> Result<(), godark::GodarkError> {
    let _ = dotenvy::dotenv();

    let config = GodarkClient::builder()
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
