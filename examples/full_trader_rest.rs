//! REST-only trader demo — mirrors the docs onboarding flow.
//!
//! Configuration is shared with the WebSocket examples — set `GODARK_*` (or
//! `GDX_*`) variables in a sibling `.env` file. With nothing else set the SDK
//! derives the REST base URL from `GODARK_EDGE_URL` (`ws[s]://...` →
//! `http[s]://...`), so a single env var configures both protocols. Override
//! with `GODARK_REST_URL` when REST and WebSocket live on different hosts.
//!
//! ```bash
//! cargo run --example full_trader_rest
//! ```
//!
//! Falls back to the legacy `test-key-1` static key when no key id/secret is
//! present (mirrors the cpp examples).

use std::time::Duration;

use godark::{GodarkRestClient, OrderType, Side, TimeInForce};

#[path = "common.rs"]
mod common;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    common::load_dotenv();

    let key_id = common::env_first(&["GODARK_API_KEY_ID", "GDX_API_KEY_ID"]);
    let key_secret = common::env_first(&["GODARK_API_SECRET", "GDX_API_SECRET"]);
    let bare_key = common::env_first(&["GODARK_API_KEY", "GDX_API_KEY"]);

    // Leaving `rest_base_url` unset lets the SDK resolver pick it up from
    // GODARK_REST_URL / GDX_REST_URL or derive it from GODARK_EDGE_URL.
    let mut builder = GodarkRestClient::builder();
    if let Some(rest_url) = common::env_first(&["GODARK_REST_URL", "GDX_REST_URL"]) {
        builder = builder.rest_base_url(rest_url);
    }

    builder = match (key_id, key_secret, bare_key) {
        (Some(id), Some(secret), _) => builder.api_key_id(id).api_secret(secret),
        (_, _, Some(key)) => builder.api_key(key),
        _ => builder.api_key("test-key-1"),
    };

    let mut client = builder.build()?;

    println!("connecting (auth + ECDH session.setup)…");
    if let Err(e) = client.connect().await {
        common::print_godark_error("[rest]", "connect", &e);
        return Err(e.into());
    }
    println!("session established: {}", client.is_session_established());

    // GODARK_TEST_LIMIT_PRICE overrides for environments with a deviation cap.
    let limit_price: f64 = std::env::var("GODARK_TEST_LIMIT_PRICE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(30_000.0);

    let ack = match client
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
        .await
    {
        Ok(ack) => ack,
        Err(e) => {
            common::print_godark_error("[rest]", "place_order", &e);
            client.disconnect().await?;
            return Err(e.into());
        }
    };
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

    if let Err(e) = client.cancel_order(&ack.order_id, "BTC-USDC-PERP").await {
        common::print_godark_error("[rest]", "cancel_order", &e);
    }

    client.disconnect().await?;
    println!("done");
    Ok(())
}
