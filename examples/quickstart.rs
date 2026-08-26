//! GoDark Rust SDK — Quickstart Example
//!
//! Place a limit sell, then cancel it.
//! This MM distribution supports `MARKET` and `LIMIT` order placement only.
//!
//! ```text
//! cargo run --release --example quickstart
//! ```
//!
//! Reads credentials from `.env` (or the OS environment):
//!   GODARK_API_KEY_ID=gdk_...
//!   GODARK_API_SECRET=...
//!   GODARK_PASSPHRASE=...
//!   # GODARK_EDGE_URL=...   (optional; default Environment::Testnet)

use godark::{Environment, GodarkClient, GodarkError, OrderType, Side, TimeInForce};

#[path = "dotenv.rs"]
mod dotenv;

const SYMBOL: &str = "BTC-USDC-PERP";

fn live_mark_price() -> f64 {
    dotenv::env_first(&["GODARK_E2E_PRICE", "GDX_E2E_PRICE", "GDX_LIVE_PRICE"])
        .and_then(|v| v.parse().ok())
        .unwrap_or(79_000.0)
}

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    dotenv::load_dotenv();

    let mut builder = GodarkClient::builder().environment(Environment::Testnet);
    if let Some(base_url) = dotenv::env_first(&["GODARK_EDGE_URL", "GDX_EDGE_URL"]) {
        builder = builder.base_url(base_url);
    }
    if let Some(legacy) = dotenv::env_first(&["GODARK_API_KEY", "GDX_API_KEY"]) {
        builder = builder.api_key(legacy);
        if let Some(uid) = dotenv::env_first(&["GODARK_USER_UUID", "GDX_USER_UUID"]) {
            builder = builder.user_uuid(uid);
        }
    } else {
        let api_key_id = dotenv::env_first(&["GODARK_API_KEY_ID", "GDX_API_KEY_ID"]).ok_or_else(|| {
            GodarkError::Config("Set GODARK_API_KEY_ID or legacy GODARK_API_KEY".into())
        })?;
        let api_secret = dotenv::env_first(&["GODARK_API_SECRET", "GDX_API_SECRET"]).ok_or_else(|| {
            GodarkError::Config("Set GODARK_API_SECRET or legacy GODARK_API_KEY".into())
        })?;
        let passphrase = dotenv::env_first(&["GODARK_PASSPHRASE", "GDX_PASSPHRASE"]).ok_or_else(|| {
            GodarkError::Config("Set GODARK_PASSPHRASE or legacy GODARK_API_KEY".into())
        })?;
        builder = builder.api_key_id(api_key_id).api_secret(api_secret).passphrase(passphrase);
    }
    let config = builder.build()?;

    let mut client = GodarkClient::new(config);
    client.connect().await?;

    let user = client
        .user_uuid()
        .map(|u| u.to_string())
        .unwrap_or_default();
    println!("Connected as user {user}");

    // Book confirmation waits on private order updates; subscribe first.
    client.subscribe(&["orders"]).await?;

    let mark = live_mark_price();
    let sell_px = (mark * 1.03 * 10.0).round() / 10.0;
    match client
        .place_order(
            SYMBOL,
            Side::Sell,
            OrderType::Limit,
            0.01,
            Some(sell_px),
            TimeInForce::Gtc,
            false,
            None,
            None,
        )
        .await
    {
        Ok(ack) => {
            println!(
                "Place OK -- order_id={} (limit SELL @ {sell_px}, mark={mark})",
                ack.order_id
            );
            // Allow the resting order to settle before cancel (avoids CANCEL_TOO_SOON).
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            let cancel = client.cancel_order(&ack.order_id, SYMBOL).await?;
            println!("Cancel OK -- order_id={}", cancel.order_id);
        }
        Err(e) => {
            dotenv::print_order_error("Order rejected", &e);
            client.disconnect().await;
            return Err(e);
        }
    }

    client.disconnect().await;
    println!("Disconnected");
    Ok(())
}
