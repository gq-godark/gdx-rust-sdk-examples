//! GoDark Rust SDK — Trader Reference Example
//!
//! Demonstrates:
//!   1. Load credentials from `.env` / environment
//!   2. REST pre-flight: fetch shielded balance via `get_my_balance`
//!   3. Connect and authenticate (encrypted ECDH session)
//!   3. Take receivers for order, position, and all 6 sequencer push streams
//!   4. Subscribe to the private order + position channels
//!   5. Place, modify, and cancel `MARKET` / `LIMIT` orders
//!   6. Drain queued updates between actions
//!   7. Print a session summary including push-callback counts
//!   8. Clean disconnect
//!
//! ```text
//! cargo run --release --example full_trader_example
//! ```

use std::collections::HashMap;
use std::time::Duration;

use godark::{GodarkClient, GodarkRestClient, OrderType, Side, TimeInForce, TransportConfig};

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

    let mut rest = match GodarkRestClient::builder()
        .api_key_id(&api_key_id)
        .api_secret(&api_secret)
        .passphrase(&passphrase)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("REST config error: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = rest.connect().await {
        eprintln!("REST connect failed: {e}");
        std::process::exit(1);
    }
    match rest.get_my_balance().await {
        Ok(bal) => println!("Balance: shielded_raw={}", bal.shielded_balance_raw),
        Err(e) => {
            eprintln!("GetMyBalance failed: {e}");
            let _ = rest.disconnect().await;
            std::process::exit(1);
        }
    }
    let _ = rest.disconnect().await;

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
            "HEALTH nodes={}  accepting={}  ready={}",
            h.total_nodes, h.accepting_orders, h.ready
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
