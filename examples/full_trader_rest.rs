//! REST-only trader demo — mirrors the docs onboarding flow.
//!
//! ```bash
//! GDX_REST_URL=http://127.0.0.1:4000 \
//! GDX_API_KEY_ID=... GDX_API_SECRET=... \
//!   cargo run --example full_trader_rest
//! ```
//!
//! Falls back to legacy `test-key-1` static key when no key id/secret env vars are set.

use std::time::Duration;

use godark::{GodarkRestClient, OrderType, Side, TimeInForce};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base = std::env::var("GDX_REST_URL").unwrap_or_else(|_| "http://127.0.0.1:4000".into());
    let key_id = std::env::var("GDX_API_KEY_ID").unwrap_or_default();
    let key_secret = std::env::var("GDX_API_SECRET").unwrap_or_default();

    let mut builder = GodarkRestClient::builder().rest_base_url(base);
    if !key_id.is_empty() && !key_secret.is_empty() {
        builder = builder.api_key_id(key_id).api_secret(key_secret);
    } else {
        builder = builder.api_key("test-key-1");
    }
    let mut client = builder.build()?;

    println!("connecting (auth + ECDH session.setup)…");
    client.connect().await?;
    println!("session established: {}", client.is_session_established());

    // GODARK_TEST_LIMIT_PRICE overrides for environments with a deviation cap.
    let limit_price: f64 = std::env::var("GODARK_TEST_LIMIT_PRICE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000.0);

    let ack = client
        .place_order(
            "BTC-USDC-PERP",
            Side::Buy,
            OrderType::Limit,
            0.001,
            Some(limit_price),
            TimeInForce::Gtc,
            false,
            None,
            None,
            Some("sdk-rust-rest-demo".into()),
        )
        .await?;
    println!(
        "placed: order_id={} sequence={}",
        ack.order_id, ack.sequence
    );

    if let Ok(snap) = client.get_order(&ack.order_id).await {
        println!("get_order snapshot: {snap}");
    }

    let _ = client
        .await_terminal_status(&ack.order_id, Duration::from_secs(2))
        .await;

    let _ = client.cancel_order(&ack.order_id, "BTC-USDC-PERP").await;

    client.disconnect().await?;
    println!("done");
    Ok(())
}
