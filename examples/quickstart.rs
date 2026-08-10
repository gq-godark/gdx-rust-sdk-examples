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

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    dotenv::load_dotenv();

    let api_key_id = std::env::var("GODARK_API_KEY_ID").map_err(|_| {
        GodarkError::Config("Set GODARK_API_KEY_ID in your environment or .env file".into())
    })?;
    let api_secret = std::env::var("GODARK_API_SECRET").map_err(|_| {
        GodarkError::Config("Set GODARK_API_SECRET in your environment or .env file".into())
    })?;
    let passphrase = std::env::var("GODARK_PASSPHRASE").map_err(|_| {
        GodarkError::Config("Set GODARK_PASSPHRASE in your environment or .env file".into())
    })?;
    let mut builder = GodarkClient::builder()
        .environment(Environment::Testnet)
        .api_key_id(api_key_id)
        .api_secret(api_secret)
        .passphrase(passphrase);
    if let Ok(base_url) = std::env::var("GODARK_EDGE_URL") {
        if !base_url.trim().is_empty() {
            builder = builder.base_url(base_url.trim());
        }
    }
    let config = builder.build()?;

    let mut client = GodarkClient::new(config);
    client.connect().await?;

    let user = client
        .user_uuid()
        .map(|u| u.to_string())
        .unwrap_or_default();
    println!("Connected as user {user}");

    match client
        .place_order(
            SYMBOL,
            Side::Sell,
            OrderType::Limit,
            0.01,
            Some(999_999.0),
            TimeInForce::Gtc,
            false,
            None,
            None,
        )
        .await
    {
        Ok(ack) => {
            println!("Place OK -- order_id={}", ack.order_id);
            let cancel = client.cancel_order(&ack.order_id, SYMBOL).await?;
            println!("Cancel OK -- order_id={}", cancel.order_id);
        }
        Err(e) => dotenv::print_order_error("Order rejected", &e),
    }

    client.disconnect().await;
    println!("Disconnected");
    Ok(())
}
