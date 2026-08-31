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

    match client.get_me().await {
        Ok(me) => println!(
            "me: id={} wallet={} tier={}",
            me.id, me.wallet_address, me.tier
        ),
        Err(err) => println!("get_me skipped: {err}"),
    }

    match client.get_leverage().await {
        Ok(lev) => {
            println!("leverage settings: {} entries", lev.settings.len());
            for row in lev.settings.iter().take(5) {
                println!("  symbol_id={} leverage={}", row.symbol_id, row.leverage);
            }
        }
        Err(err) => println!("get_leverage skipped: {err}"),
    }

    println!("REST reads succeeded.");
    println!("For REST trading (place/modify/cancel), see full_trader_rest.");
    client.disconnect().await?;
    Ok(())
}
