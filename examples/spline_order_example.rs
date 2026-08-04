//! GoDark Rust SDK — Spline Order Example
//!
//! Place a compressed spline liquidity curve, refresh the anchor, then cancel
//! the whole object via normal `cancel_order`.
//!
//! ```text
//! # Testnet / hosted (API key pair):
//! cargo run --release --example spline_order_example
//!
//! # Localnet (legacy API key):
//! GODARK_EDGE_URL=ws://127.0.0.1:13300 \
//! GODARK_API_KEY=test-key-1 \
//! GODARK_USER_UUID=00000000-0000-4000-8000-000000000001 \
//!   cargo run --release --example spline_order_example
//! ```

use godark::{GodarkClient, GodarkError, SplineRegionInput};

#[path = "dotenv.rs"]
mod dotenv;

const SYMBOL: &str = "BTC-USDC-PERP";

fn build_client() -> Result<GodarkClient, GodarkError> {
    let base_url =
        std::env::var("GODARK_EDGE_URL").unwrap_or_else(|_| "wss://api.godark-dex.com".into());
    let edge_url = std::env::var("GDX_EDGE_URL").unwrap_or(base_url);

    let builder = GodarkClient::builder().base_url(&edge_url);

    // Localnet path: legacy static API key.
    if let Ok(api_key) = std::env::var("GODARK_API_KEY").or_else(|_| std::env::var("GDX_API_KEY")) {
        let mut b = builder.api_key(api_key);
        if let Ok(uuid) =
            std::env::var("GODARK_USER_UUID").or_else(|_| std::env::var("GDX_USER_UUID"))
        {
            b = b.user_uuid(uuid);
        }
        return Ok(GodarkClient::new(b.build()?));
    }

    // Hosted path: API key id + secret + passphrase.
    let api_key_id = std::env::var("GODARK_API_KEY_ID").map_err(|_| {
        GodarkError::Config(
            "Set GODARK_API_KEY (localnet) or GODARK_API_KEY_ID (hosted) in .env".into(),
        )
    })?;
    let api_secret = std::env::var("GODARK_API_SECRET").map_err(|_| {
        GodarkError::Config("Set GODARK_API_SECRET in your environment or .env file".into())
    })?;
    let passphrase = std::env::var("GODARK_PASSPHRASE").map_err(|_| {
        GodarkError::Config("Set GODARK_PASSPHRASE in your environment or .env file".into())
    })?;

    Ok(GodarkClient::new(
        GodarkClient::builder()
            .base_url(&edge_url)
            .api_key_id(api_key_id)
            .api_secret(api_secret)
            .passphrase(passphrase)
            .build()?,
    ))
}

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    dotenv::load_dotenv();

    let mut client = build_client()?;
    client.connect().await?;

    let user = client
        .user_uuid()
        .map(|u| u.to_string())
        .unwrap_or_default();
    println!("Connected as user {user}");

    let bid = vec![SplineRegionInput {
        start_offset: 1,
        end_offset: 5,
        density: 0.01,
    }];
    let ask = vec![SplineRegionInput {
        start_offset: 1,
        end_offset: 4,
        density: 0.01,
    }];

    let ack = match client
        .place_spline_order(SYMBOL, 50_000.0, &bid, &ask, 1, None)
        .await
    {
        Ok(ack) => {
            println!(
                "Spline place -- accepted={} order_id={} sequence={}",
                ack.accepted, ack.order_id, ack.sequence
            );
            if !ack.accepted {
                eprintln!("Spline rejected error_code={:?}", ack.error_code);
                client.disconnect().await;
                return Ok(());
            }
            ack
        }
        Err(e) => {
            dotenv::print_order_error("Spline place failed", &e);
            client.disconnect().await;
            return Ok(());
        }
    };

    let order_id: u64 = ack.order_id.parse().map_err(|_| {
        GodarkError::Config(format!("invalid spline order_id: {}", ack.order_id))
    })?;

    match client
        .update_spline_anchor(SYMBOL, order_id, 50_100.0)
        .await
    {
        Ok(upd) => println!(
            "Spline anchor update -- accepted={} order_id={} sequence={}",
            upd.accepted, upd.order_id, upd.sequence
        ),
        Err(e) => dotenv::print_order_error("Spline anchor update failed", &e),
    }

    match client.cancel_order(&ack.order_id, SYMBOL).await {
        Ok(cancel) => println!(
            "Cancel OK -- success={} order_id={}",
            cancel.success, cancel.order_id
        ),
        Err(e) => dotenv::print_order_error("Cancel failed", &e),
    }

    client.disconnect().await;
    println!("Disconnected");
    Ok(())
}
