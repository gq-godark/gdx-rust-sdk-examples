//! GoDark Rust SDK — Trader Reference Example
//!
//! Demonstrates:
//!   1. Load credentials from `.env` / environment
//!   2. Connect and authenticate (Noise XK encrypted WebSocket session)
//!   3. Take receivers for order, position, and all 6 sequencer push streams
//!   4. Subscribe to the private order + position channels
//!   5. Place, modify, and cancel `MARKET` / `LIMIT` orders
//!   6. Mass-quote a BUY ladder, batch-cancel it, then demo post_only true/false
//!   7. Drain queued updates between actions
//!   8. Print a session summary including push-callback counts
//!   9. Clean disconnect
//!
//! ```text
//! cargo run --release --example full_trader_example
//! ```

use std::collections::HashMap;
use std::time::Duration;

use godark::{GodarkClient, MassQuoteLegInput, OrderType, Side, TimeInForce, TransportConfig};

#[path = "dotenv.rs"]
mod dotenv;

const SYMBOL: &str = "BTC-USDC-PERP";

#[tokio::main]
async fn main() {
    dotenv::load_dotenv();

    let sep = "=".repeat(60);
    println!("{sep}");
    println!("  GoDark Rust SDK — Trader Reference Example");
    println!("{sep}");
    println!("Order-type support in this distribution: MARKET, LIMIT");

    let api_key_id = match std::env::var("GODARK_API_KEY_ID") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!(
                "Missing credentials. Set GODARK_API_KEY_ID and GODARK_API_SECRET \
                 (or provide them in .env)."
            );
            std::process::exit(1);
        }
    };
    let api_secret = match std::env::var("GODARK_API_SECRET") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!(
                "Missing credentials. Set GODARK_API_KEY_ID and GODARK_API_SECRET \
                 (or provide them in .env)."
            );
            std::process::exit(1);
        }
    };
    let passphrase = match std::env::var("GODARK_PASSPHRASE") {
        Ok(v) if !v.is_empty() => v,
        _ => {
            eprintln!(
                "Missing credentials. Set GODARK_PASSPHRASE \
                 (or provide it in .env)."
            );
            std::process::exit(1);
        }
    };
    let base_url =
        std::env::var("GODARK_EDGE_URL").unwrap_or_else(|_| "wss://api.godark-dex.com".into());

    println!("Endpoint: {base_url}");

    let mut headers = HashMap::new();
    headers.insert("X-Trader-Tag".into(), "rust-full-trader-demo".into());
    let transport = TransportConfig {
        extra_headers: headers,
        connect_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(10),
        heartbeat_interval: Duration::from_secs(30),
        stale_timeout: Duration::from_secs(60),
        ..TransportConfig::default()
    };

    let config = match GodarkClient::builder()
        .base_url(&base_url)
        .api_key_id(api_key_id)
        .api_secret(api_secret)
        .passphrase(passphrase)
        .transport(transport)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {e}");
            std::process::exit(1);
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

    println!("Connecting...");
    if let Err(e) = client.connect().await {
        eprintln!("Failed to connect: {e}");
        std::process::exit(1);
    }

    let user = client
        .user_uuid()
        .map(|u| u.to_string())
        .unwrap_or_default();
    println!("Authenticated as user_uuid={user}  (session encrypted)");

    if let Err(e) = client.subscribe(&["orders", "positions"]).await {
        eprintln!("Subscribe failed: {e}");
        client.disconnect().await;
        std::process::exit(1);
    }
    println!("Subscribed to order + position updates");

    // Drain the initial PositionsSnapshot the sequencer pushes right after the
    // trading session is established.
    tokio::time::sleep(Duration::from_millis(200)).await;
    while let Ok(pos) = position_rx.try_recv() {
        println!(
            "POS    side={:?}  size={}  entry={}",
            pos.side, pos.size, pos.entry_price
        );
    }
    let mut last_btc_mark: f64 = 0.0;
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
            if row.symbol_id == 1 {
                if let Some(m) = row.mark_price.as_deref().and_then(|s| s.parse::<f64>().ok()) {
                    if m > 0.0 {
                        last_btc_mark = m;
                    }
                }
            }
        }
    }

    println!("Placing limit BUY @ 67500...");
    let buy_ack = match client
        .place_order(
            SYMBOL,
            Side::Buy,
            OrderType::Limit,
            0.1,
            Some(67_500.0),
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
            dotenv::print_order_error("BUY rejected", &e);
            client.disconnect().await;
            std::process::exit(1);
        }
    };

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_orders(&mut order_rx, "after BUY");

    println!("Modifying order price to 68000...");
    match client
        .modify_order(&buy_ack.order_id, SYMBOL, Some(68_000.0), None)
        .await
    {
        Ok(ack) => println!("Modified: order_id={}", ack.order_id),
        Err(e) => dotenv::print_order_error("Modify rejected", &e),
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_orders(&mut order_rx, "after MODIFY");

    println!("Placing limit SELL @ 95000...");
    match client
        .place_order(
            SYMBOL,
            Side::Sell,
            OrderType::Limit,
            0.05,
            Some(95_000.0),
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
                Err(e) => dotenv::print_order_error("Cancel SELL rejected", &e),
            }
        }
        Err(e) => dotenv::print_order_error("SELL rejected", &e),
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_orders(&mut order_rx, "after SELL/CANCEL");

    // --- Bulk quote (mass quote) -------------------------------------------
    // Place a whole ladder of resting quotes in a single batched request. With
    // the default (post_only) mode every leg is post-only: a leg that would
    // cross is rejected as "failed" so the batch fuses into one MPC round. Pass
    // Some(false) for the relaxed path, where a crossing leg takes liquidity
    // up to its limit and rests the remainder (reported per leg as fill_count).
    // Anchor the ladder/cross to the live BTC mark captured from the snapshot so
    // the crossing demo below is deterministic regardless of current price. Fall
    // back to GDX_BASE (default 64000) only if no mark was seen yet.
    let mut base = last_btc_mark;
    if base <= 0.0 {
        base = std::env::var("GDX_BASE")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| *v > 0.0)
            .unwrap_or(64_000.0);
    }
    let round1 = |x: f64| (x * 10.0).round() / 10.0;
    println!("Mass-quoting a 3-level BUY ladder (post-only), base={base:.2}...");
    let ladder = [
        MassQuoteLegInput {
            side: Side::Buy,
            price: round1(base * (1.0 - 0.003)),
            quantity: 0.02,
            cancel_order_id: None,
            time_in_force: None,
            expiry_time: None,
        },
        MassQuoteLegInput {
            side: Side::Buy,
            price: round1(base * (1.0 - 0.006)),
            quantity: 0.02,
            cancel_order_id: None,
            time_in_force: None,
            expiry_time: None,
        },
        MassQuoteLegInput {
            side: Side::Buy,
            price: round1(base * (1.0 - 0.009)),
            quantity: 0.02,
            cancel_order_id: None,
            time_in_force: None,
            expiry_time: None,
        },
    ];
    let mut resting_ids: Vec<u64> = Vec::new();
    match client.mass_quote(SYMBOL, &ladder, 1, None).await {
        Ok(mq) => {
            println!(
                "Mass quote: success={} sequence={} legs={}",
                mq.success,
                mq.sequence,
                mq.results.len()
            );
            for r in &mq.results {
                println!(
                    "  leg {}: status={} new_order_id={} fills={} err={:?}",
                    r.leg_index,
                    r.status,
                    r.new_order_id.as_deref().unwrap_or(""),
                    r.fill_count,
                    r.error_code
                );
                if r.status == "open" {
                    if let Some(id) = r.new_order_id.as_deref().and_then(|s| s.parse().ok()) {
                        resting_ids.push(id);
                    }
                }
            }
        }
        Err(e) => dotenv::print_order_error("Mass quote rejected", &e),
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_orders(&mut order_rx, "after MASS QUOTE");

    if !resting_ids.is_empty() {
        println!(
            "Batch-cancelling {} ladder orders (cleanup)...",
            resting_ids.len()
        );
        match client.batch_cancel(SYMBOL, &resting_ids).await {
            Ok(bc) => {
                for r in &bc.results {
                    println!(
                        "  cancel id={}: cancelled={} err={:?}",
                        r.order_id, r.cancelled, r.error_code
                    );
                }
            }
            Err(e) => dotenv::print_order_error("Batch cancel rejected", &e),
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
        drain_orders(&mut order_rx, "after BATCH CANCEL");
    }

    // Demonstrate the batch-level post_only flag on a crossing leg. Price a BUY
    // ~5% above the live mark: aggressive enough to cross the resting ask, yet
    // within the exchange's 10%-of-oracle limit.
    let cross_px = round1(base * 1.05);
    println!("Mass-quoting a crossing BUY with post_only=true (expect rejected/2018)...");
    match client
        .mass_quote(
            SYMBOL,
            &[MassQuoteLegInput {
                side: Side::Buy,
                price: cross_px,
                quantity: 0.001,
                cancel_order_id: None,
                time_in_force: None,
                expiry_time: None,
            }],
            1,
            Some(true),
        )
        .await
    {
        Ok(mq) => {
            for r in &mq.results {
                println!(
                    "  leg {}: status={} err={:?} fills={}",
                    r.leg_index, r.status, r.error_code, r.fill_count
                );
            }
        }
        Err(e) => dotenv::print_order_error("post_only=true mass quote rejected", &e),
    }
    tokio::time::sleep(Duration::from_millis(500)).await;

    println!("Mass-quoting a crossing BUY with post_only=false (expect filled, fill_count>0)...");
    match client
        .mass_quote(
            SYMBOL,
            &[MassQuoteLegInput {
                side: Side::Buy,
                price: cross_px,
                quantity: 0.003,
                cancel_order_id: None,
                time_in_force: None,
                expiry_time: None,
            }],
            1,
            Some(false),
        )
        .await
    {
        Ok(mq) => {
            for r in &mq.results {
                println!(
                    "  leg {}: status={} new_order_id={} err={:?} fills={}",
                    r.leg_index,
                    r.status,
                    r.new_order_id.as_deref().unwrap_or(""),
                    r.error_code,
                    r.fill_count
                );
            }
        }
        Err(e) => dotenv::print_order_error("post_only=false mass quote rejected", &e),
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_orders(&mut order_rx, "after post_only mass quotes");

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
            "HEALTH component={}  state={}  serving={}  cause={}",
            h.component_id, h.state, h.serving, h.cause
        );
    }
    let mut balance_count = 0usize;
    while let Ok(b) = balance_rx.try_recv() {
        balance_count += 1;
        println!("BAL    shielded_raw={}", b.shielded_balance_raw);
    }
    let mut margin_count = 0usize;
    while let Ok(a) = margin_alert_rx.try_recv() {
        margin_count += 1;
        println!(
            "MARGIN symbol={}  tier={}  ratio_bps={}",
            a.symbol_id, a.tier, a.margin_ratio_bps
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
        println!("SETTLE batch={}  status={:?}", s.batch_id, s.status);
    }

    let mut error_count = 0usize;
    while let Ok(e) = error_rx.try_recv() {
        error_count += 1;
        eprintln!("SDK ERROR (non-fatal): {e}");
    }

    println!("{sep}");
    println!("  Session complete");
    println!(
        "  Pushes: snapshots={snap_count}  health={health_count}  \
         balance={balance_count}  margin={margin_count}  \
         funding={funding_count}  settle={settle_count}"
    );
    println!("  Non-fatal errors received: {error_count}");
    println!("{sep}");

    client.disconnect().await;
    println!("Disconnected cleanly");
}

fn drain_orders(rx: &mut tokio::sync::mpsc::Receiver<godark::OrderUpdate>, label: &str) {
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
