//! GoDark Rust SDK — Trader Reference Example
//!
//! Demonstrates:
//!   1. Load credentials from `.env` / environment
//!   2. Connect and authenticate (HPKE WebSocket session)
//!   3. Take receivers for orders, positions, and the other sequencer pushes
//!   4. Subscribe to the private order + position channels
//!   5. Place, modify, and cancel post-only `LIMIT` orders priced off the live mark
//!   6. Mass-quote / cancel-by-id ladder demo
//!   7. Drain queued updates between actions
//!   8. Print a session summary including push-callback counts
//!   9. Clean disconnect
//!
//! ```text
//! cargo run --release --example full_trader_example
//! ```

use std::collections::HashMap;
use std::time::Duration;

use godark::{
    Confirmation, Environment, GodarkClient, MassQuoteLegInput, OrderType, Side,
    TimeInForce, TransportConfig,
};

#[path = "dotenv.rs"]
mod dotenv;
#[path = "live_mark.rs"]
mod live_mark;

const SYMBOL: &str = live_mark::SYMBOL;

#[tokio::main]
async fn main() {
    dotenv::load_dotenv();

    let sep = "=".repeat(60);
    println!("{sep}");
    println!("  GoDark Rust SDK — Trader Reference Example");
    println!("{sep}");
    println!("This sample places post-only LIMIT orders only");

    let legacy_key = dotenv::env_first(&["GODARK_API_KEY", "GDX_API_KEY"]);
    let edge_override = dotenv::env_first(&["GODARK_EDGE_URL", "GDX_EDGE_URL"]);
    println!(
        "Endpoint: {}",
        edge_override
            .as_deref()
            .unwrap_or(Environment::Testnet.edge_base_url())
    );

    let mut headers = HashMap::new();
    headers.insert("X-Trader-Tag".into(), "rust-full-trader-demo".into());
    let transport = TransportConfig {
        extra_headers: headers,
        connect_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(10),
        heartbeat_interval: Duration::from_secs(30),
        stale_timeout: Duration::from_secs(120),
        missed_heartbeat_limit: 2,
        ..TransportConfig::default()
    };

    let mut builder = GodarkClient::builder()
        .environment(Environment::Testnet)
        .transport(transport);
    if let Some(legacy) = legacy_key {
        builder = builder.api_key(legacy);
        if let Some(account) = dotenv::env_first(&["GODARK_ACCOUNT", "GDX_ACCOUNT"]) {
            builder = builder.account(account);
        }
    } else {
        let Some(api_key_id) = dotenv::env_first(&["GODARK_API_KEY_ID", "GDX_API_KEY_ID"]) else {
            eprintln!(
                "Missing credentials. Set GODARK_API_KEY_ID/GODARK_API_SECRET/GODARK_PASSPHRASE \
                 or legacy GODARK_API_KEY for localnet."
            );
            std::process::exit(1);
        };
        let Some(api_secret) = dotenv::env_first(&["GODARK_API_SECRET", "GDX_API_SECRET"]) else {
            eprintln!(
                "Missing credentials. Set GODARK_API_KEY_ID/GODARK_API_SECRET/GODARK_PASSPHRASE \
                 or legacy GODARK_API_KEY for localnet."
            );
            std::process::exit(1);
        };
        let Some(passphrase) = dotenv::env_first(&["GODARK_PASSPHRASE", "GDX_PASSPHRASE"]) else {
            eprintln!(
                "Missing credentials. Set GODARK_PASSPHRASE \
                 or legacy GODARK_API_KEY for localnet."
            );
            std::process::exit(1);
        };
        builder = builder
            .api_key_id(api_key_id)
            .api_secret(api_secret)
            .passphrase(passphrase);
    }
    if let Some(base_url) = edge_override.as_deref() {
        builder = builder.base_url(dotenv::edge_ws_url(&base_url));
    }
    let config = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {e}");
            std::process::exit(1);
        }
    };

    let mut client = GodarkClient::new(config);

    let mut order_rx = client.take_order_receiver().expect("order receiver");
    let mut positions_snapshot_rx = client
        .take_positions_snapshot_receiver()
        .expect("positions snapshot receiver");
    let mut system_health_rx = client
        .take_system_health_receiver()
        .expect("system health receiver");
    let mut balance_rx = client.take_balance_receiver().expect("balance receiver");
    let mut account_margin_rx = client
        .take_account_margin_receiver()
        .expect("account margin receiver");
    let mut funding_rate_rx = client
        .take_funding_rate_receiver()
        .expect("funding rate receiver");
    let mut leverage_settings_rx = client
        .take_leverage_settings_receiver()
        .expect("leverage settings receiver");
    let mut error_rx = client.take_error_receiver().expect("error receiver");

    println!("Connecting...");
    if let Err(e) = client.connect().await {
        eprintln!("Failed to connect: {e}");
        std::process::exit(1);
    }

    let account = client
        .account()
        .map(|id| id.to_string())
        .unwrap_or_default();
    println!("Authenticated as account={account}  (HPKE session)");

    if let Err(e) = client
        .subscribe(&["orders", "positions", "funding_rate"])
        .await
    {
        eprintln!("Subscribe failed: {e}");
        client.disconnect().await;
        std::process::exit(1);
    }
    println!("Subscribed to order + position + funding updates");

    // Drain the initial PositionsSnapshot the sequencer pushes right after subscribe.
    tokio::time::sleep(Duration::from_millis(200)).await;
    // BTC-USDC-PERP is symbol_id 1; capture mark as a decimal string for display.
    let mut last_mark_btc: Option<String> = None;
    while let Ok(snap) = positions_snapshot_rx.try_recv() {
        println!(
            "SNAP   source={:?}  rows={}  ts={}",
            snap.source,
            snap.rows.len(),
            snap.server_timestamp
        );
        for row in &snap.rows {
            if row.symbol_id == 1 {
                if let Some(m) = row.mark_price.as_deref().filter(|s| !s.is_empty()) {
                    last_mark_btc = Some(m.to_string());
                }
            }
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

    println!("Setting leverage to 1 via GodarkClient.update_leverage...");
    if let Err(e) = async {
        let ack = client.update_leverage(SYMBOL, 1).await?;
        println!(
            "update_leverage: success={}  order_id={}",
            ack.success, ack.order_id
        );
        Ok::<(), godark::GodarkError>(())
    }
    .await
    {
        dotenv::print_order_error("update_leverage rejected", &e);
    }

    let mut rest = match live_mark::connect_rest().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("REST connect failed: {e}");
            client.disconnect().await;
            std::process::exit(1);
        }
    };
    if let Err(e) = live_mark::cancel_sample_leftovers(&mut rest).await {
        eprintln!("{e}");
        client.disconnect().await;
        std::process::exit(1);
    }
    let before_ids = match live_mark::open_order_ids(&mut rest).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("{e}");
            client.disconnect().await;
            std::process::exit(1);
        }
    };
    println!("open orders before: {}", before_ids.len());

    let mut snapshot_marks = Vec::new();
    if let Some(m) = last_mark_btc {
        snapshot_marks.push(m);
    }
    if let Ok(snap) = rest.get_positions().await {
        snapshot_marks.extend(live_mark::marks_from_positions(&snap));
    }
    let mark = match live_mark::resolve_mark(&rest, &snapshot_marks).await {
        Ok(mark) => mark,
        Err(e) => {
            eprintln!("{e}");
            client.disconnect().await;
            std::process::exit(1);
        }
    };
    if let Err(e) = live_mark::flatten_small_positions(&mut rest, mark).await {
        eprintln!("{e}");
        client.disconnect().await;
        std::process::exit(1);
    }

    let Some(buy_px) = live_mark::buy_limit(mark) else {
        eprintln!("mark too small for a post-only buy");
        client.disconnect().await;
        std::process::exit(1);
    };
    let Some(modify_px) = live_mark::buy_limit_deeper(mark, 500) else {
        eprintln!("mark too small to modify the buy");
        client.disconnect().await;
        std::process::exit(1);
    };
    let sell_px = live_mark::sell_limit(mark);
    let mut ours: Vec<String> = Vec::new();

    println!(
        "Placing post-only limit BUY qty={} @ {buy_px} (mark floor {})",
        live_mark::QTY,
        mark.floor
    );
    let buy_id = match client
        .place_order_with_options(
            SYMBOL,
            Side::Buy,
            OrderType::Limit,
            Some(live_mark::QTY),
            Some(buy_px.as_str()),
            TimeInForce::Gtc,
            false,
            None,
            None,
            Confirmation::Book,
            live_mark::post_only(),
        )
        .await
    {
        Ok(ack) if ack.success => {
            println!(
                "BUY placed: order_id={}  sequence={}",
                ack.order_id, ack.sequence
            );
            ours.push(ack.order_id.clone());
            ack.order_id
        }
        Ok(ack) => {
            eprintln!("BUY rejected: {:?}", ack.error.or(ack.error_code));
            client.disconnect().await;
            std::process::exit(1);
        }
        Err(e) => {
            dotenv::print_order_error("BUY rejected", &e);
            client.disconnect().await;
            std::process::exit(1);
        }
    };

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_orders(&mut order_rx, "after BUY");

    println!("Modifying order price to {modify_px}...");
    match client
        .modify_order(&buy_id, SYMBOL, Some(modify_px.as_str()), None, None)
        .await
    {
        Ok(ack) if ack.success => println!("Modified: order_id={}", ack.order_id),
        Ok(ack) => {
            eprintln!("Modify rejected: {:?}", ack.error.or(ack.error_code));
            abort_orders(&mut client, &mut rest, &before_ids, &ours, mark).await;
        }
        Err(e) => {
            dotenv::print_order_error("Modify rejected", &e);
            abort_orders(&mut client, &mut rest, &before_ids, &ours, mark).await;
        }
    }
    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_orders(&mut order_rx, "after MODIFY");

    println!("Post-only limits only; this sample does not place market or IOC orders.");

    println!(
        "Placing post-only limit SELL qty={} @ {sell_px}",
        live_mark::QTY
    );
    match client
        .place_order_with_options(
            SYMBOL,
            Side::Sell,
            OrderType::Limit,
            Some(live_mark::QTY),
            Some(sell_px.as_str()),
            TimeInForce::Gtc,
            false,
            None,
            None,
            Confirmation::Book,
            live_mark::post_only(),
        )
        .await
    {
        Ok(sell_ack) if sell_ack.success => {
            println!("SELL placed: order_id={}", sell_ack.order_id);
            ours.push(sell_ack.order_id);
        }
        Ok(sell_ack) => {
            eprintln!(
                "SELL rejected: {:?}",
                sell_ack.error.or(sell_ack.error_code)
            );
            abort_orders(&mut client, &mut rest, &before_ids, &ours, mark).await;
        }
        Err(e) => {
            dotenv::print_order_error("SELL rejected", &e);
            abort_orders(&mut client, &mut rest, &before_ids, &ours, mark).await;
        }
    }

    tokio::time::sleep(Duration::from_secs(1)).await;
    drain_orders(&mut order_rx, "after SELL");

    let Some(ladder_1) = live_mark::buy_limit(mark) else {
        eprintln!("mark too small for quote leg");
        abort_orders(&mut client, &mut rest, &before_ids, &ours, mark).await;
    };
    let Some(ladder_2) = live_mark::buy_limit_deeper(mark, 100) else {
        eprintln!("mark too small for quote leg");
        abort_orders(&mut client, &mut rest, &before_ids, &ours, mark).await;
    };
    let Some(ladder_3) = live_mark::buy_limit_deeper(mark, 200) else {
        eprintln!("mark too small for quote leg");
        abort_orders(&mut client, &mut rest, &before_ids, &ours, mark).await;
    };
    let mk = |price: &str| MassQuoteLegInput {
        side: Side::Buy,
        price: price.to_string(),
        quantity: live_mark::QTY.to_string(),
        cancel_order_id: None,
        time_in_force: None,
        expiry_time: None,
    };
    println!("Mass-quoting a 3-level post-only BUY ladder...");
    let ladder = vec![mk(&ladder_1), mk(&ladder_2), mk(&ladder_3)];
    let mut quote_failed = false;
    match client.mass_quote(SYMBOL, &ladder, Some(true)).await {
        Ok(mq) => {
            println!(
                "Mass quote: success={}  sequence={}  legs={}",
                mq.success,
                mq.sequence,
                mq.results.len()
            );
            if !mq.success {
                quote_failed = true;
            }
            for r in &mq.results {
                println!(
                    "  leg {}: status={}  new_order_id={}  fills={}  err={:?}",
                    r.leg_index,
                    r.status,
                    r.new_order_id.as_deref().unwrap_or("—"),
                    r.fill_count,
                    r.error_code
                );
                if r.fill_count > 0 || !r.status.eq_ignore_ascii_case("open") {
                    quote_failed = true;
                }
                if let Some(id) = r.new_order_id.as_deref() {
                    if !id.is_empty() && id != "0" {
                        ours.push(id.to_string());
                    }
                }
            }
        }
        Err(e) => {
            dotenv::print_order_error("Mass quote rejected", &e);
            abort_orders(&mut client, &mut rest, &before_ids, &ours, mark).await;
        }
    }

    live_mark::wait_before_cancel().await;
    if let Err(e) = live_mark::refresh_rest(&mut rest).await {
        eprintln!("{e}");
        client.disconnect().await;
        std::process::exit(1);
    }
    drain_orders(&mut order_rx, "before cancel");
    println!("Cancelling {} order(s) this process placed...", ours.len());
    if let Err(e) = cancel_ours(&client, &ours).await {
        eprintln!("{e}");
        let _ = live_mark::flatten_small_positions(&mut rest, mark).await;
        client.disconnect().await;
        std::process::exit(1);
    }
    if let Err(e) = live_mark::flatten_small_positions(&mut rest, mark).await {
        eprintln!("{e}");
        client.disconnect().await;
        std::process::exit(1);
    }
    if let Err(e) =
        live_mark::assert_no_new_orders_or_positions(&mut rest, &before_ids, &ours).await
    {
        eprintln!("{e}");
        client.disconnect().await;
        std::process::exit(1);
    }
    if quote_failed {
        eprintln!("one or more mass-quote legs did not rest as post-only");
        client.disconnect().await;
        std::process::exit(1);
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
        println!("BAL    balance_raw={}", b.balance_raw);
    }
    let mut margin_count = 0usize;
    while let Ok(a) = account_margin_rx.try_recv() {
        margin_count += 1;
        println!(
            "MARGIN account={}  ts={}  isolated_margin={}  cross_im={}",
            a.account,
            a.server_timestamp,
            a.summary
                .as_ref()
                .map(|s| s.isolated_margin.as_str())
                .unwrap_or(""),
            a.summary
                .as_ref()
                .map(|s| s.cross_im.as_str())
                .unwrap_or("")
        );
    }
    let mut funding_count = 0usize;
    while let Ok(f) = funding_rate_rx.try_recv() {
        funding_count += 1;
        println!(
            "FUND   symbol={}  rate={}  last={}",
            f.symbol_id, f.funding_rate, f.last_funding_rate
        );
    }
    let mut leverage_count = 0usize;
    while let Ok(ls) = leverage_settings_rx.try_recv() {
        leverage_count += 1;
        let rows: Vec<String> = ls
            .settings
            .iter()
            .take(5)
            .map(|r| format!("{}={}x", r.symbol_id, r.leverage))
            .collect();
        println!("LEVERAGE settings=[{}]", rows.join(", "));
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
         funding={funding_count}  leverage={leverage_count}"
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
        let badges = format!(
            "{}{}{}",
            u.cancel_reason
                .map(|r| format!("  cancel_reason={r:?}"))
                .unwrap_or_default(),
            if u.reduce_only {
                "  reduce_only=true"
            } else {
                ""
            },
            if u.post_only { "  post_only=true" } else { "" },
        );
        println!(
            "ORDER  {:?}  id={}  status={:?}  filled={}  remaining={}{badges}",
            u.update_type, u.order_id, u.status, u.filled_qty, u.remaining_qty
        );
    }
    if count > 0 {
        println!("  ({count} order update(s) {label})");
    }
}

async fn abort_orders(
    client: &mut godark::GodarkClient,
    rest: &mut godark::GodarkRestClient,
    before_ids: &[String],
    ours: &[String],
    mark: live_mark::Mark,
) -> ! {
    live_mark::wait_before_cancel().await;
    if let Err(e) = cancel_ours(client, ours).await {
        eprintln!("{e}");
    }
    if let Err(e) = live_mark::flatten_small_positions(rest, mark).await {
        eprintln!("{e}");
    }
    if let Err(e) =
        live_mark::assert_no_new_orders_or_positions(rest, before_ids, ours).await
    {
        eprintln!("{e}");
    }
    client.disconnect().await;
    std::process::exit(1);
}

async fn cancel_ours(client: &godark::GodarkClient, ids: &[String]) -> Result<(), String> {
    let mut err = None;
    for id in ids {
        match client.cancel_order(id, SYMBOL).await {
            Ok(ca) if ca.success => println!("  cancel order_id={}", ca.order_id),
            Ok(ca) => {
                let msg = format!(
                    "cancel {id} rejected: {:?}",
                    ca.error.or(ca.error_code)
                );
                eprintln!("{msg}");
                err = Some(msg);
            }
            Err(e) => {
                let msg = format!("cancel {id}: {e}");
                eprintln!("{msg}");
                err = Some(msg);
            }
        }
    }
    match err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}
