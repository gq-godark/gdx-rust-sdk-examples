//! GoDark SDK — Market data streaming example
//!
//! Stream L2 orderbook and trades without authentication.
//!
//! ```text
//! cargo run --example market_data
//! ```
//!
//! Set `GODARK_EDGE_URL` to override the default edge base URL
//! (`wss://api.godark-dex.com`, the public testnet — no public mainnet today).
//! The transport appends `/ws/v1` at connect time; the public market-data
//! channel is served at `<host>/ws/gomarket`.

use std::time::Duration;

use godark::{GodarkError, MarketDataClient, TransportConfig};
use serde_json::Value;

const DEFAULT_EDGE_URL: &str = "wss://api.godark-dex.com";
const STREAM_SECS: u64 = 30;

fn tls_skip_verify() -> bool {
    for var in ["GDX_TLS_SKIP_VERIFY", "GODARK_TLS_SKIP_VERIFY"] {
        if let Ok(v) = std::env::var(var) {
            if matches!(v.trim(), "1" | "true" | "TRUE" | "yes") {
                return true;
            }
        }
    }
    false
}

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    // Base URL for the edge (gomarket WebSocket is derived inside the client).
    let base_url = std::env::var("GDX_EDGE_URL")
        .ok()
        .or_else(|| std::env::var("GODARK_EDGE_URL").ok())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_EDGE_URL.to_string());

    let transport = TransportConfig {
        tls_skip_verify: tls_skip_verify(),
        ..TransportConfig::default()
    };
    let mut client = MarketDataClient::with_transport(&base_url, transport);

    // Open the public market-data WebSocket.
    client.connect().await?;

    // Single consumer for `(channel:symbol, payload)` pairs from the client.
    let mut event_rx = client
        .take_event_receiver()
        .ok_or_else(|| GodarkError::Config("event receiver was already taken".into()))?;

    // Subscribe to order book and trades for BTC perpetual.
    client.subscribe_orderbook("BTC-USDC-PERP").await?;
    client.subscribe_trades("BTC-USDC-PERP").await?;

    println!(
        "Streaming market data for {} seconds (orderbook + trades on BTC-USDC-PERP)...",
        STREAM_SECS
    );

    let end = tokio::time::Instant::now() + Duration::from_secs(STREAM_SECS);
    tokio::pin!(let sleep = tokio::time::sleep_until(end););

    loop {
        tokio::select! {
            _ = &mut sleep => break,
            msg = event_rx.recv() => {
                match msg {
                    Some((key, val)) => print_market_event(&key, &val),
                    None => {
                        println!("Event channel closed.");
                        break;
                    }
                }
            }
        }
    }

    client.disconnect().await;
    Ok(())
}

fn print_market_event(key: &str, val: &Value) {
    let typ = val.get("type").and_then(|c| c.as_str()).unwrap_or("");
    if matches!(
        typ,
        "status" | "subscribed" | "unsubscribed" | "pong" | "error"
    ) {
        return;
    }
    // `key` is `channel:symbol` from the client; gomarket data uses `type` + `symbol`.
    let channel = val.get("channel").and_then(|c| c.as_str()).unwrap_or("");

    if typ == "orderbook" || channel == "orderbook" || key.starts_with("orderbook:") {
        let bids = val.get("bids").and_then(|b| b.as_array());
        let asks = val.get("asks").and_then(|a| a.as_array());
        let best_bid = bids
            .and_then(|b| b.first())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".into());
        let best_ask = asks
            .and_then(|a| a.first())
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".into());
        println!("Orderbook | best bid: {best_bid} | best ask: {best_ask}");
    } else if typ == "trade" || channel == "trades" || key.starts_with("trades:") {
        let price = val
            .get("price")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".into());
        let size = val
            .get("size")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".into());
        let side = val
            .get("side")
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".into());
        println!("Trade | price={price} size={size} side={side}");
    } else {
        println!("{key} | {}", val);
    }
}
