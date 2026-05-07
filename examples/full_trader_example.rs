//! GoDark Rust SDK — Complete Trader Example
//!
//! Mirrors `python/examples/full_trader_example.py` and
//! `javascript/examples/full-trader-example.ts`:
//!
//!   1. Configure transport (TLS, timeouts, headers)
//!   2. Authenticate with API key pair
//!   3. Take error / order / position channels
//!   4. Subscribe to private order + position streams
//!   5. Stream public market data (orderbook + trades)
//!   6. Place, modify, and cancel orders
//!   7. Drain queued order updates via the channel
//!   8. Clean shutdown
//!
//! ## How to run
//!
//! ```text
//! cd gdx/sdks/rust
//! cargo run --example full_trader_example
//! ```
//!
//! Or with `cargo run --release --example full_trader_example`.
//!
//! Prerequisites: gdx-edge listening on **:4000** (e.g. localnet `localnet_edge_1`).
//!
//! Override URL or keys with env (optional):
//!   `GDX_EDGE_URL`, `GDX_API_KEY_ID`, `GDX_API_SECRET`
//!
//! Default `base_url` is `wss://api.godark-dex.com` (testnet, matches SDK
//! default + `full_trader_example.py`); set `GDX_EDGE_URL=ws://localhost:4000`
//! for localnet.

use std::collections::HashMap;
use std::time::Duration;

use godark::{GodarkClient, MarketDataClient, OrderType, Side, TimeInForce, TransportConfig};
use serde_json::Value;

#[path = "common.rs"]
mod common;

const SYMBOL: &str = "BTC-USDC-PERP";

const DEFAULT_API_KEY_ID: &str = "YOUR_API_KEY_ID";
const DEFAULT_API_SECRET: &str = "YOUR_API_SECRET";

fn edge_url() -> String {
    common::env_first(&["GODARK_EDGE_URL", "GDX_EDGE_URL"])
        .unwrap_or_else(|| "wss://api.godark-dex.com".into())
}

fn api_key_id() -> String {
    common::env_first(&["GODARK_API_KEY_ID", "GDX_API_KEY_ID"])
        .unwrap_or_else(|| DEFAULT_API_KEY_ID.into())
}

fn api_secret() -> String {
    common::env_first(&["GODARK_API_SECRET", "GDX_API_SECRET"])
        .unwrap_or_else(|| DEFAULT_API_SECRET.into())
}

/// Optional bare static token (e.g. localnet `test-key-1`). Takes precedence
/// over id/secret when set, mirroring the cpp examples.
fn api_key_bare() -> Option<String> {
    common::env_first(&["GODARK_API_KEY", "GDX_API_KEY"])
}

fn tls_skip_verify() -> bool {
    common::env_first(&["GODARK_TLS_SKIP_VERIFY", "GDX_TLS_SKIP_VERIFY"])
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false)
}

fn transport_config() -> TransportConfig {
    let mut headers = HashMap::new();
    headers.insert("X-Trader-Tag".into(), "rust-full-trader-demo".into());
    TransportConfig {
        extra_headers: headers,
        connect_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(10),
        heartbeat_interval: Duration::from_secs(30),
        stale_timeout: Duration::from_secs(60),
        tls_skip_verify: tls_skip_verify(),
        use_docs_wire: true,
    }
}

#[tokio::main]
async fn main() {
    common::load_dotenv();

    let sep = "=".repeat(60);
    println!("{sep}");
    println!("  GoDark SDK — Complete Trader Example (Rust)");
    println!("{sep}");

    let url = edge_url();

    let mut builder = GodarkClient::builder()
        .base_url(&url)
        .transport(transport_config());
    builder = if let Some(token) = api_key_bare() {
        builder.api_key(token)
    } else {
        builder.api_key_id(api_key_id()).api_secret(api_secret())
    };
    let config = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {e}");
            return;
        }
    };

    let mut client = GodarkClient::new(config);

    let mut order_rx = client.take_order_receiver().expect("order receiver");
    let mut position_rx = client.take_position_receiver().expect("position receiver");
    let mut positions_snapshot_rx = client
        .take_positions_snapshot_receiver()
        .expect("positions snapshot receiver");
    let mut system_health_rx = client
        .take_system_health_receiver()
        .expect("system health receiver");
    let mut balance_rx = client.take_balance_receiver().expect("balance receiver");
    let mut margin_alert_rx = client
        .take_margin_alert_receiver()
        .expect("margin alert receiver");
    let mut funding_rate_rx = client
        .take_funding_rate_receiver()
        .expect("funding rate receiver");
    let mut settlement_rx = client.take_settlement_receiver().expect("settlement receiver");
    let mut error_rx = client.take_error_receiver().expect("error receiver");

    // ── 1. Connect & authenticate ──────────────────────────────────
    println!("Connecting...");
    if let Err(e) = client.connect().await {
        eprintln!("Failed to connect: {e}");
        return;
    }

    let uid = client
        .user_uuid()
        .map(|u| u.to_string())
        .unwrap_or_default();
    println!("Authenticated as user_uuid={uid}  (session encrypted)");

    // ── 2. Subscribe to private channels ───────────────────────────
    if let Err(e) = client.subscribe(&["orders", "positions"]).await {
        eprintln!("Subscribe failed: {e}");
        client.disconnect().await;
        return;
    }
    println!("Subscribed to order + position updates");

    // Drain any initial position snapshot that arrives immediately.
    tokio::time::sleep(Duration::from_millis(200)).await;
    while let Ok(pos) = position_rx.try_recv() {
        println!(
            "POS    side={:?}  size={}  entry={}",
            pos.side, pos.size, pos.entry_price
        );
    }
    // Drain the initial PositionsSnapshot the sequencer pushes right after
    // the trading session is established.
    while let Ok(snap) = positions_snapshot_rx.try_recv() {
        println!(
            "SNAP   source={:?}  rows={}  ts={}",
            snap.source,
            snap.rows.len(),
            snap.server_timestamp
        );
        for row in &snap.rows {
            println!(
                "  ↳ symbol={}  side={:?}  size={}  entry={}  mark={}",
                row.symbol_id,
                row.side,
                row.size,
                row.entry_price,
                row.mark_price.as_deref().unwrap_or("—")
            );
        }
    }

    // ── 3. Start market data feed (no auth needed) ─────────────────
    let md_transport = TransportConfig {
        tls_skip_verify: tls_skip_verify(),
        ..TransportConfig::default()
    };
    let mut md = MarketDataClient::with_transport(&url, md_transport);
    let mut md_event_rx = md.take_event_receiver();
    match md.connect().await {
        Ok(()) => {
            let _ = md.subscribe_orderbook(SYMBOL).await;
            let _ = md.subscribe_trades(SYMBOL).await;
            println!("Market data streaming for {SYMBOL}");
        }
        Err(e) => {
            eprintln!("Market data unavailable (continuing without): {e}");
        }
    }

    // Print market data for a short window.
    if let Some(ref mut rx) = md_event_rx {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while let Ok(Some((key, val))) = tokio::time::timeout_at(deadline, rx.recv()).await {
            print_market_event(&key, &val);
        }
    }

    // ── 4. Place a limit BUY ───────────────────────────────────────
    // GODARK_TEST_LIMIT_PRICE overrides for environments with a deviation cap
    // (localnet caps at 1000 bps; testnet has no cap).
    let buy_price: f64 = std::env::var("GODARK_TEST_LIMIT_PRICE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(67_500.0);
    let modify_price = buy_price + 500.0;
    let sell_price = buy_price + 2_000.0;

    println!("Placing limit BUY @ {buy_price}...");
    let buy_ack = match client
        .place_order(
            SYMBOL,
            Side::Buy,
            OrderType::Limit,
            0.1,
            Some(buy_price),
            TimeInForce::Gtc,
            false,
            None,
            None,
        )
        .await
    {
        Ok(ack) => {
            println!(
                "BUY placed: order_id={}  sequence={}",
                ack.order_id, ack.sequence
            );
            ack
        }
        Err(e) => {
            common::print_godark_error("[full]", "place_order BUY", &e);
            md.disconnect().await;
            client.disconnect().await;
            return;
        }
    };

    // Let order updates arrive.
    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_order_updates(&mut order_rx, "after BUY");

    // ── 5. Modify the order ────────────────────────────────────────
    println!("Modifying order price to ${modify_price}...");
    match client
        .modify_order(&buy_ack.order_id, SYMBOL, Some(modify_price), None)
        .await
    {
        Ok(ack) => println!("Modified: order_id={}", ack.order_id),
        Err(e) => common::print_godark_error("[full]", "modify_order", &e),
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_order_updates(&mut order_rx, "after MODIFY");

    // ── 6. Place a SELL and cancel it ──────────────────────────────
    println!("Placing limit SELL @ {sell_price}...");
    match client
        .place_order(
            SYMBOL,
            Side::Sell,
            OrderType::Limit,
            0.05,
            Some(sell_price),
            TimeInForce::Gtc,
            false,
            None,
            None,
        )
        .await
    {
        Ok(sell_ack) => {
            println!("SELL placed: order_id={}", sell_ack.order_id);

            tokio::time::sleep(Duration::from_millis(500)).await;

            match client.cancel_order(&sell_ack.order_id, SYMBOL).await {
                Ok(cancel_ack) => {
                    println!("SELL cancelled: order_id={}", cancel_ack.order_id)
                }
                Err(e) => common::print_godark_error("[full]", "cancel_order SELL", &e),
            }
        }
        Err(e) => common::print_godark_error("[full]", "place_order SELL", &e),
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_order_updates(&mut order_rx, "after SELL/CANCEL");

    // ── 7. Drain remaining queued updates ──────────────────────────
    println!("Draining any remaining queued updates (short window)...");
    let mut drained = 0usize;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(400);
    while let Ok(Some(u)) = tokio::time::timeout_at(deadline, order_rx.recv()).await {
        drained += 1;
        println!("  (queued) order_id={} status={:?}", u.order_id, u.status);
    }
    println!("Drained {drained} queued order update(s)");

    // ── 8. Cancel original BUY (cleanup) ───────────────────────────
    println!("Cancelling original BUY (cleanup)...");
    match client.cancel_order(&buy_ack.order_id, SYMBOL).await {
        Ok(_) => println!("Original BUY cancelled"),
        Err(_) => println!("Original BUY already filled or cancelled"),
    }

    // Drain any sequencer pushes that arrived during the session.
    let mut snap_count = 0usize;
    while let Ok(snap) = positions_snapshot_rx.try_recv() {
        snap_count += 1;
        println!(
            "SNAP   source={:?}  rows={}  ts={}",
            snap.source,
            snap.rows.len(),
            snap.server_timestamp
        );
    }
    let mut health_count = 0usize;
    while let Ok(h) = system_health_rx.try_recv() {
        health_count += 1;
        println!(
            "HEALTH nodes={}  accepting={}  ready={}  degraded={}",
            h.total_nodes, h.accepting_orders, h.ready, h.degraded
        );
    }
    let mut balance_count = 0usize;
    while let Ok(b) = balance_rx.try_recv() {
        balance_count += 1;
        println!(
            "BAL    user={}  shielded_raw={}  ts={}",
            b.user_uuid, b.shielded_balance_raw, b.timestamp
        );
    }
    let mut margin_count = 0usize;
    while let Ok(a) = margin_alert_rx.try_recv() {
        margin_count += 1;
        println!(
            "MARGIN owner={}  symbol={}  tier={}  ratio_bps={}  recovered={}",
            a.owner, a.symbol_id, a.tier, a.margin_ratio_bps, a.recovered
        );
    }
    let mut funding_count = 0usize;
    while let Ok(f) = funding_rate_rx.try_recv() {
        funding_count += 1;
        println!(
            "FUND   symbol={}  current={}  predicted={}",
            f.symbol_id, f.current_rate, f.predicted_rate
        );
    }
    let mut settle_count = 0usize;
    while let Ok(s) = settlement_rx.try_recv() {
        settle_count += 1;
        println!(
            "SETTLE batch={}  status={:?}  tx={}",
            s.batch_id, s.status, s.tx_signature
        );
    }

    // Drain any errors that arrived during the session.
    let mut error_count = 0usize;
    while let Ok(e) = error_rx.try_recv() {
        error_count += 1;
        eprintln!("SDK ERROR (non-fatal): {e}");
    }

    // ── 9. Summary ─────────────────────────────────────────────────
    println!("{sep}");
    println!("  Session complete");
    println!(
        "  Pushes: snapshots={snap_count}  health={health_count}  \
         balance={balance_count}  margin={margin_count}  \
         funding={funding_count}  settle={settle_count}"
    );
    println!("  Non-fatal errors received: {error_count}");
    println!("{sep}");

    // ── 10. Disconnect ─────────────────────────────────────────────
    md.disconnect().await;
    client.disconnect().await;
    println!("Disconnected cleanly");
}

fn drain_order_updates(rx: &mut tokio::sync::mpsc::Receiver<godark::OrderUpdate>, label: &str) {
    let mut count = 0usize;
    while let Ok(u) = rx.try_recv() {
        count += 1;
        println!(
            "ORDER  {:?}  id={}  status={:?}  filled={}  remaining={}",
            u.update_type, u.order_id, u.status, u.filled_qty, u.remaining_qty
        );
    }
    if count > 0 {
        println!("  ({count} order update(s) {label})");
    }
}

fn print_market_event(key: &str, val: &Value) {
    let typ = val.get("type").and_then(|c| c.as_str()).unwrap_or("");
    if matches!(
        typ,
        "status" | "subscribed" | "unsubscribed" | "pong" | "error"
    ) {
        return;
    }
    let channel = val.get("channel").and_then(|c| c.as_str()).unwrap_or("");

    if typ == "orderbook" || channel == "orderbook" || key.starts_with("orderbook:") {
        let best_ask = val
            .get("asks")
            .and_then(|a| a.as_array())
            .and_then(|a| a.first())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".into());
        println!("ORDERBOOK  best_ask={best_ask}");
    } else if typ == "trade" || channel == "trades" || key.starts_with("trades:") {
        let price = val.get("price").map(|v| v.to_string()).unwrap_or_default();
        let size = val.get("size").map(|v| v.to_string()).unwrap_or_default();
        let side = val.get("side").map(|v| v.to_string()).unwrap_or_default();
        println!("TRADE  price={price}  size={size}  side={side}");
    }
}
