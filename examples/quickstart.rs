//! GoDark SDK — Quickstart example
//!
//! Place a limit order on BTC-USDC-PERP and read a few order updates.
//!
//! ```text
//! cargo run --example quickstart -- <API_KEY_ID> <API_SECRET>
//! ```
//!
//! Or set `GODARK_API_KEY_ID` and `GODARK_API_SECRET`. Optional: `GODARK_EDGE_URL` for the edge base URL.
//!
//! Localnet shortcut: set `GODARK_API_KEY=test-key-1` (bare static token) — the
//! local stack ships with this key pre-seeded. Mirrors the cpp examples.

use std::env;
use std::time::Duration;

use godark::{GodarkClient, GodarkError, OrderType, Side, TimeInForce};

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    // Resolve credentials in order of precedence:
    //   1. CLI args:  `quickstart <ID> <SECRET>`
    //   2. Bare static token via `GODARK_API_KEY` (localnet `test-key-1`)
    //   3. RFC 6749 client_credentials via `GODARK_API_KEY_ID` + `GODARK_API_SECRET`
    let cli_id = env::args().nth(1).filter(|s| !s.is_empty());
    let cli_secret = env::args().nth(2).filter(|s| !s.is_empty());
    let bare_token = env::var("GODARK_API_KEY").ok().filter(|s| !s.is_empty());
    let env_id = env::var("GODARK_API_KEY_ID").ok().filter(|s| !s.is_empty());
    let env_secret = env::var("GODARK_API_SECRET").ok().filter(|s| !s.is_empty());

    let mut builder = GodarkClient::builder();
    builder = match (cli_id, cli_secret, bare_token, env_id, env_secret) {
        (Some(id), Some(secret), _, _, _) => builder.api_key_id(id).api_secret(secret),
        (_, _, Some(token), _, _) => builder.api_key(token),
        (_, _, _, Some(id), Some(secret)) => builder.api_key_id(id).api_secret(secret),
        _ => {
            return Err(GodarkError::Config(
                "Set GODARK_API_KEY (bare token) or GODARK_API_KEY_ID + GODARK_API_SECRET, \
                 or pass <API_KEY_ID> <API_SECRET> as arguments"
                    .into(),
            ));
        }
    };
    let config = builder.build()?;

    let mut client = GodarkClient::new(config);

    // Take the order-update channel before connecting (one receiver per client).
    let mut order_rx = client
        .take_order_receiver()
        .ok_or_else(|| GodarkError::Config("order update receiver was already taken".into()))?;

    // Connect: WebSocket, authenticate, ECDH session.
    client.connect().await?;

    // Subscribe to order and position streams (matches Python `subscribe(["orders", "positions"])`.
    client.subscribe(&["orders", "positions"]).await?;

    // Place a limit buy well off the market (adjust price for your environment).
    // Localnet has a 1000 bps deviation cap — set GODARK_TEST_LIMIT_PRICE to a
    // value near the live mark when running against `gdx up`. Testnet has no
    // cap so the default works there.
    let limit_price: f64 = env::var("GODARK_TEST_LIMIT_PRICE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(67_500.0);

    let ack = client
        .place_order(
            "BTC-USDC-PERP",
            Side::Buy,
            OrderType::Limit,
            0.1,
            Some(limit_price),
            TimeInForce::Gtc,
            false,
            None,
            None,
        )
        .await?;

    println!(
        "Order placed: order_id={} sequence={}",
        ack.order_id, ack.sequence
    );

    // Print a few order updates as they arrive (non-blocking with overall timeout).
    const MAX_UPDATES: usize = 8;
    const UPDATES_WAIT: Duration = Duration::from_secs(15);

    let updates_deadline = tokio::time::Instant::now() + UPDATES_WAIT;
    let mut count = 0usize;

    while count < MAX_UPDATES && tokio::time::Instant::now() < updates_deadline {
        let remaining = updates_deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining, order_rx.recv()).await {
            Ok(Some(update)) => {
                println!(
                    "[{:?}] order={} status={:?} filled_qty={}",
                    update.update_type, update.order_id, update.status, update.filled_qty
                );
                count += 1;
            }
            Ok(None) => {
                println!("Order update channel closed.");
                break;
            }
            Err(_) => {
                println!("No further order updates within timeout; continuing to cancel.");
                break;
            }
        }
    }

    // Cancel the resting order.
    let cancel_ack = client.cancel_order(&ack.order_id, "BTC-USDC-PERP").await?;
    println!(
        "Cancel ack: order_id={} sequence={}",
        cancel_ack.order_id, cancel_ack.sequence
    );

    // Clean shutdown: stop background tasks and close the socket.
    client.disconnect().await;

    Ok(())
}
