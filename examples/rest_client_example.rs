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

    let api_key_id = dotenv::env_first(&["GODARK_API_KEY_ID", "GDX_API_KEY_ID"]).ok_or_else(|| {
        GodarkError::Config("Set GODARK_API_KEY_ID in your environment or .env file".into())
    })?;
    let api_secret = dotenv::env_first(&["GODARK_API_SECRET", "GDX_API_SECRET"]).ok_or_else(|| {
        GodarkError::Config("Set GODARK_API_SECRET in your environment or .env file".into())
    })?;
    let passphrase = dotenv::env_first(&["GODARK_PASSPHRASE", "GDX_PASSPHRASE"]).ok_or_else(|| {
        GodarkError::Config("Set GODARK_PASSPHRASE in your environment or .env file".into())
    })?;

    let mut builder = GodarkRestClient::builder()
        .api_key_id(api_key_id)
        .api_secret(api_secret)
        .passphrase(passphrase);
    if let Some(rest) = dotenv::env_first(&[
        "GODARK_REST_URL",
        "GDX_REST_URL",
        "GODARK_EDGE_URL",
        "GDX_EDGE_URL",
    ]) {
        builder = builder.rest_base_url(rest);
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
