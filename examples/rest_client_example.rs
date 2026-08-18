//! Minimal GodarkRestClient demo — auth + account reads + public market data.
//!
//! Encrypted place/cancel/modify/update_leverage require GodarkClient (WebSocket /
//! Noise XK); see `quickstart` / `full_trader_example`.
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

    // Public market-data GETs — no connect() required.
    let rates = client.get_funding_rates().await?;
    let oi = client.get_open_interest().await?;
    let vol = client.get_volume().await?;
    let rates_n = rates.as_array().map(|a| a.len()).unwrap_or(0);
    let oi_n = oi.as_array().map(|a| a.len()).unwrap_or(0);
    let vol_n = vol["symbols"].as_array().map(|a| a.len()).unwrap_or(0);
    println!(
        "funding_rates: {rates_n} symbols (first={:?})",
        rates.get(0)
    );
    println!("open_interest: {oi_n} symbols (first={:?})", oi.get(0));
    println!(
        "volume: total_24h={} symbols={vol_n}",
        vol["total_volume_24h"]
    );

    println!("connecting (REST auth/token)...");
    client.connect().await?;

    let me = client.get_me().await?;
    println!(
        "me: id={} wallet={} tier={}",
        me.id, me.wallet_address, me.tier
    );

    let lev = client.get_leverage().await?;
    println!("leverage settings: {} entries", lev.settings.len());
    for row in lev.settings.iter().take(5) {
        println!(
            "  symbol_id={} leverage={}",
            row.symbol_id, row.leverage
        );
    }

    match client.get_my_balance().await {
        Ok(bal) => println!(
            "balance: shielded_raw={} wallet_ui={}",
            bal.shielded_balance_raw, bal.wallet_usdt_ui
        ),
        Err(err) => println!("get_my_balance skipped: {err}"),
    }

    println!("REST reads succeeded.");
    println!("Encrypted trading requires GodarkClient over WebSocket (Noise XK).");
    client.disconnect().await?;
    Ok(())
}
