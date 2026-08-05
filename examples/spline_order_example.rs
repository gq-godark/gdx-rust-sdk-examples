//! GoDark Rust SDK — Spline Order Example
//!
//! Place a compressed spline liquidity curve, refresh the anchor, then cancel
//! the whole object via normal `cancel_order`.
//!
//! ```text
//! # Testnet / hosted (API key pair):
//! cargo run --release --example spline_order_example
//!
//! GODARK_EDGE_URL=ws://127.0.0.1:13300 \
//! GODARK_API_KEY=test-key-1 \
//! GODARK_USER_UUID=00000000-0000-4000-8000-000000000001 \
//! GDX_NOISE_STATIC_PUBLIC_KEY=<64-hex from sequencer log> \
//!   cargo run --release --example spline_order_example
//! ```

use godark::{GodarkClient, GodarkError, SplineRegionInput};

#[path = "dotenv.rs"]
mod dotenv;

const SYMBOL: &str = "BTC-USDC-PERP";
const DEFAULT_ANCHOR_PRICE: f64 = 68_000.0;

fn noise_pin_from_env() -> Option<String> {
    for key in [
        "GDX_NOISE_STATIC_PUBLIC_KEY",
        "GODARK_NOISE_STATIC_PUBLIC_KEY",
        "GDX_NOISE_STATIC_PUBKEY",
    ] {
        if let Ok(v) = std::env::var(key) {
            let t = v.trim();
            if !t.is_empty() {
                return Some(t.to_string());
            }
        }
    }
    None
}

fn build_client() -> Result<GodarkClient, GodarkError> {
    let base_url =
        std::env::var("GODARK_EDGE_URL").unwrap_or_else(|_| "wss://api.godark-dex.com".into());
    let edge_url = std::env::var("GDX_EDGE_URL").unwrap_or(base_url);

    println!("Config: edge_url={edge_url}");
    let mut builder = GodarkClient::builder().base_url(&edge_url);
    if let Some(pin) = noise_pin_from_env() {
        println!(
            "Config: using Noise static public key pin ({} hex chars)",
            pin.len()
        );
        builder = builder.noise_static_public_key_hex(pin);
    } else {
        println!("Config: no Noise static public key pin found in env");
    }

    // Localnet path: legacy static API key.
    if let Ok(api_key) = std::env::var("GODARK_API_KEY").or_else(|_| std::env::var("GDX_API_KEY")) {
        println!("Config: using localnet legacy API key auth ({api_key})");
        let mut b = builder.api_key(api_key);
        if let Ok(uuid) =
            std::env::var("GODARK_USER_UUID").or_else(|_| std::env::var("GDX_USER_UUID"))
        {
            println!("Config: user_uuid={uuid}");
            b = b.user_uuid(uuid);
        }
        return Ok(GodarkClient::new(b.build()?));
    }

    // Hosted path: API key id + secret + passphrase.
    println!("Config: using hosted API key pair auth");
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
        builder
            .api_key_id(api_key_id)
            .api_secret(api_secret)
            .passphrase(passphrase)
            .build()?,
    ))
}

fn env_f64(name: &str, default: f64) -> f64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(default)
}

fn env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(default)
}

fn spline_shape_from_env() -> String {
    std::env::var("GODARK_SPLINE_SHAPE")
        .unwrap_or_else(|_| "step".into())
        .trim()
        .to_ascii_lowercase()
}

fn build_spline_regions(
    shape: &str,
    bid_start: u32,
    bid_end: u32,
    ask_start: u32,
    ask_end: u32,
    q_start: f64,
    slope: f64,
) -> (Vec<SplineRegionInput>, Vec<SplineRegionInput>) {
    match shape {
        "linear_taper" | "lineartaper" | "linear-taper" => (
            vec![SplineRegionInput::linear_taper(bid_start, bid_end, q_start, slope)],
            vec![SplineRegionInput::linear_taper(ask_start, ask_end, q_start, slope)],
        ),
        _ => (
            vec![SplineRegionInput::step(bid_start, bid_end, q_start)],
            vec![SplineRegionInput::step(ask_start, ask_end, q_start)],
        ),
    }
}

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    dotenv::load_dotenv();

    println!("Step 1: building client");
    let mut client = build_client()?;

    println!("Step 2: connecting and completing auth + Noise handshake");
    client.connect().await?;

    let user = client
        .user_uuid()
        .map(|u| u.to_string())
        .unwrap_or_default();
    println!("Connected as user {user}");

    let anchor_price = env_f64("GODARK_SPLINE_ANCHOR_PRICE", DEFAULT_ANCHOR_PRICE);
    let updated_anchor_price = env_f64("GODARK_SPLINE_UPDATED_ANCHOR_PRICE", anchor_price + 100.0);
    println!(
        "Config: symbol={SYMBOL} anchor_price={anchor_price} updated_anchor_price={updated_anchor_price}"
    );

    let shape = spline_shape_from_env();
    let bid_start = env_u32("GODARK_SPLINE_BID_START", 1);
    let bid_end = env_u32("GODARK_SPLINE_BID_END", 5);
    let ask_start = env_u32("GODARK_SPLINE_ASK_START", 1);
    let ask_end = env_u32("GODARK_SPLINE_ASK_END", 4);
    let q_start = env_f64("GODARK_SPLINE_Q_START", 0.01);
    let slope = env_f64("GODARK_SPLINE_SLOPE_PER_TICK", -0.001);
    let (bid, ask) = build_spline_regions(
        &shape, bid_start, bid_end, ask_start, ask_end, q_start, slope,
    );
    println!(
        "Config: shape={shape} q_start={q_start} slope_per_tick={slope}"
    );
    println!("Config: bid_regions={bid:?}");
    println!("Config: ask_regions={ask:?}");

    println!("Step 3: placing spline order");
    let ack = match client
        .place_spline_order(SYMBOL, anchor_price, &bid, &ask, 1, None)
        .await
    {
        Ok(ack) => {
            println!(
                "Step 3 result: spline place accepted={} order_id={} sequence={}",
                ack.accepted, ack.order_id, ack.sequence
            );
            if !ack.accepted {
                eprintln!("Step 3 rejected: error_code={:?}", ack.error_code);
                client.disconnect().await;
                return Ok(());
            }
            ack
        }
        Err(e) => {
            dotenv::print_order_error("Step 3 failed: spline place", &e);
            client.disconnect().await;
            return Ok(());
        }
    };

    let order_id: u64 = ack
        .order_id
        .parse()
        .map_err(|_| GodarkError::Config(format!("invalid spline order_id: {}", ack.order_id)))?;

    println!("Step 4: updating spline anchor for order_id={order_id}");
    match client
        .update_spline_anchor(SYMBOL, order_id, updated_anchor_price)
        .await
    {
        Ok(upd) => println!(
            "Step 4 result: spline anchor update accepted={} order_id={} sequence={}",
            upd.accepted, upd.order_id, upd.sequence
        ),
        Err(e) => dotenv::print_order_error("Step 4 failed: spline anchor update", &e),
    }

    let cancel_wait_ms = env_u64("GODARK_SPLINE_CANCEL_WAIT_MS", 750);
    if cancel_wait_ms > 0 {
        println!("Step 5: waiting {cancel_wait_ms}ms before cancel");
        tokio::time::sleep(std::time::Duration::from_millis(cancel_wait_ms)).await;
    }

    println!("Step 6: cancelling spline order_id={}", ack.order_id);
    match client.cancel_order(&ack.order_id, SYMBOL).await {
        Ok(cancel) => println!(
            "Step 6 result: cancel success={} order_id={}",
            cancel.success, cancel.order_id
        ),
        Err(e) => dotenv::print_order_error("Step 6 failed: cancel", &e),
    }

    println!("Step 7: disconnecting");
    client.disconnect().await;
    println!("Disconnected");
    Ok(())
}
