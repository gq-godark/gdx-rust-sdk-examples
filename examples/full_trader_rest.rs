//! REST-only trader demo — auth + encrypted snapshots + place/modify/cancel.
//!
//! ```text
//! cargo run --release --example full_trader_rest
//! ```

use godark::{GodarkRestClient, OrderType, Side, TimeInForce};

#[path = "dotenv.rs"]
mod dotenv;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::load_dotenv();

    let base = std::env::var("GODARK_REST_URL")
        .or_else(|_| std::env::var("GDX_REST_URL"))
        .unwrap_or_else(|_| "https://api.godark-dex.com".into());
    let key_id = std::env::var("GODARK_API_KEY_ID")
        .or_else(|_| std::env::var("GDX_API_KEY_ID"))
        .unwrap_or_default();
    let key_secret = std::env::var("GODARK_API_SECRET")
        .or_else(|_| std::env::var("GDX_API_SECRET"))
        .unwrap_or_default();
    let passphrase = std::env::var("GODARK_PASSPHRASE")
        .or_else(|_| std::env::var("GDX_PASSPHRASE"))
        .unwrap_or_default();

    let legacy = std::env::var("GODARK_API_KEY")
        .or_else(|_| std::env::var("GDX_API_KEY"))
        .unwrap_or_default();
    let mut builder = GodarkRestClient::builder().rest_base_url(base);
    if !key_id.is_empty() && !key_secret.is_empty() {
        builder = builder
            .api_key_id(key_id)
            .api_secret(key_secret)
            .passphrase(passphrase);
    } else if !legacy.is_empty() {
        builder = builder.api_key(legacy);
    } else {
        return Err("Set GODARK_API_KEY_ID, GODARK_API_SECRET and GODARK_PASSPHRASE in .env".into());
    }
    let mut client = builder.build()?;

    client.connect().await?;
    let uid = client
        .user_uuid()
        .ok_or("user_uuid missing after connect")?;
    println!(
        "identity: user_uuid={uid} scope={:?}",
        client.token_scope()
    );

    let orders = client.get_open_orders().await?;
    println!("open orders: {} row(s)", orders.rows.len());
    let positions = client.get_positions().await?;
    println!("positions: {} row(s)", positions.rows.len());
    let account = client.get_account().await?;
    if let Some(s) = account.account {
        println!(
            "account free_collateral={} total_collateral={}",
            s.free_collateral, s.total_collateral
        );
    }

    let price: f64 = std::env::var("GDX_LIVE_PRICE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(78000.0);
    let ack = client
        .place_order(
            "BTC-USDC-PERP",
            Side::Buy,
            OrderType::Limit,
            0.001,
            Some(price),
            TimeInForce::Gtc,
            false,
            None,
            None,
            Some("sdk-rust-rest-demo".into()),
        )
        .await?;
    println!("placed order_id={} success={}", ack.order_id, ack.success);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let modify = client
        .modify_order(
            &ack.order_id,
            "BTC-USDC-PERP",
            Some(price - 64.0),
            None,
        )
        .await?;
    println!("modified success={}", modify.success);

    let cancel = client.cancel_order(&ack.order_id, "BTC-USDC-PERP").await?;
    println!("cancelled success={}", cancel.success);

    client.disconnect().await?;
    Ok(())
}
