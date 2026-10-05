//! Minimal GodarkRestClient demo — auth + account reads.
//!
//! For encrypted place/modify/cancel over REST (one-shot HPKE), see `full_trader_rest`.
//!
//! ```text
//! cargo run --release --example rest_client_example
//! ```
//!
//! Environment:
//!   GODARK_API_KEY_ID, GODARK_API_SECRET, GODARK_PASSPHRASE
//!   GODARK_REST_URL (optional; default https://api.godark-dex.com)

use godark::{GodarkError, GodarkRestClient};

#[path = "dotenv.rs"]
mod dotenv;

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

    let mut builder = GodarkRestClient::builder()
        .api_key_id(api_key_id)
        .api_secret(api_secret)
        .passphrase(passphrase);
    if let Ok(rest) = std::env::var("GODARK_REST_URL") {
        if !rest.trim().is_empty() {
            builder = builder.rest_base_url(rest.trim());
        }
    }
    let mut client = builder.build()?;

    println!("connecting (REST auth/token)...");
    client.connect().await?;

    let positions = client.get_positions().await?;
    let orders = client.get_open_orders().await?;
    let account = client.get_account().await?;
    let funding = client.get_funding_rates().await?;
    let interest = client.get_open_interest().await?;
    let volume = client.get_volume().await?;
    println!("positions: {} rows", positions.rows.len());
    println!("open_orders: {} rows", orders.rows.len());
    println!(
        "account total_collateral={}",
        account
            .summary
            .as_ref()
            .map(|s| s.total_collateral.as_str())
            .unwrap_or("?")
    );
    println!("funding_rates: {funding}");
    println!("open_interest: {interest}");
    println!("volume: {volume}");

    println!("REST reads succeeded.");
    println!("For REST trading (place/modify/cancel), see full_trader_rest.");
    client.disconnect().await?;
    Ok(())
}
