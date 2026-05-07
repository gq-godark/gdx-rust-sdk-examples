//! End-to-end trading smoke test — mirrors `cpp/examples/e2e_trading_smoke` and Python `check_edge_api_keys`.
//!
//! Environment (GODARK_* preferred; GDX_* aliases for parity):
//! - `GODARK_API_KEY_ID` / `GDX_API_KEY_ID`
//! - `GODARK_API_SECRET` / `GDX_API_SECRET`
//! - `GODARK_EDGE_URL` / `GDX_EDGE_URL` (optional; default is production URL from the SDK)
//!
//! ```text
//! cargo run --example e2e_trading_smoke
//! cargo run --example e2e_trading_smoke -- --auth-only
//! ```
//!
//! Exit codes: 0 success, 1 config, 2 connect/auth/session, 3 place failed, 4 cancel failed

use std::env;
use std::process::ExitCode;
use std::time::Instant;

use godark::{GodarkClient, GodarkError, OrderType, Side, TimeInForce, TransportConfig};

#[path = "common.rs"]
mod common;

fn print_usage() {
    eprintln!(
        "e2e_trading_smoke — GoDark Rust SDK end-to-end check\n\n\
         Environment:\n  GODARK_API_KEY_ID / GDX_API_KEY_ID\n  GODARK_API_SECRET / GDX_API_SECRET\n  GODARK_EDGE_URL / GDX_EDGE_URL (optional)\n\n\
         Options:\n  --auth-only     Connect + ECDH only (no orders)\n  --help          Show this message"
    );
}

struct Args {
    auth_only: bool,
}

fn parse_args() -> Result<Args, ()> {
    let mut auth_only = false;
    for arg in env::args().skip(1) {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            "--auth-only" => auth_only = true,
            other => {
                eprintln!("Unknown argument: {other}");
                print_usage();
                return Err(());
            }
        }
    }
    Ok(Args { auth_only })
}

enum Credentials {
    Bare(String),
    KeySecret(String, String),
}

fn credentials() -> Result<Credentials, ExitCode> {
    if let Some(token) = common::env_first(&["GODARK_API_KEY", "GDX_API_KEY"]) {
        return Ok(Credentials::Bare(token));
    }
    let id = common::env_first(&["GODARK_API_KEY_ID", "GDX_API_KEY_ID"]);
    let secret = common::env_first(&["GODARK_API_SECRET", "GDX_API_SECRET"]);
    match (id, secret) {
        (Some(i), Some(s)) => Ok(Credentials::KeySecret(i, s)),
        _ => {
            eprintln!(
                "Missing credentials. Set GODARK_API_KEY (bare token) \
                 or GODARK_API_KEY_ID + GODARK_API_SECRET (GDX_* aliases also accepted)."
            );
            Err(ExitCode::from(1))
        }
    }
}

fn map_early_error(operation: &str, e: GodarkError) -> ExitCode {
    let code: u8 = match &e {
        GodarkError::Config(_) => 1,
        GodarkError::Authentication(_)
        | GodarkError::Connection(_)
        | GodarkError::Session(_)
        | GodarkError::Timeout(_)
        | GodarkError::Encryption(_)
        | GodarkError::WebSocket(_)
        | GodarkError::Proto(_) => 2,
        GodarkError::Order { .. } => 3,
    };
    common::print_godark_error("[e2e]", operation, &e);
    ExitCode::from(code)
}

#[tokio::main]
async fn main() -> ExitCode {
    common::load_dotenv();

    let args = match parse_args() {
        Ok(a) => a,
        Err(()) => return ExitCode::from(1),
    };

    let creds = match credentials() {
        Ok(x) => x,
        Err(c) => return c,
    };

    let t0 = Instant::now();

    let skip_verify = env::var("GDX_TLS_SKIP_VERIFY")
        .or_else(|_| env::var("GODARK_TLS_SKIP_VERIFY"))
        .map(|v| matches!(v.trim(), "1" | "true" | "TRUE" | "yes"))
        .unwrap_or(false);
    if skip_verify {
        eprintln!("[e2e] TLS verify disabled (dev/testnet)");
    }
    let transport = TransportConfig {
        tls_skip_verify: skip_verify,
        ..TransportConfig::default()
    };

    let mut builder = GodarkClient::builder().transport(transport);
    builder = match creds {
        Credentials::Bare(token) => builder.api_key(token),
        Credentials::KeySecret(id, secret) => builder.api_key_id(id).api_secret(secret),
    };
    let config = match builder.build() {
        Ok(c) => c,
        Err(e) => return map_early_error("build", e),
    };

    let mut client = GodarkClient::new(config);

    // Receiver must be taken before connect (same as quickstart).
    let _order_rx = match client.take_order_receiver() {
        Some(rx) => rx,
        None => {
            eprintln!("[e2e] order update receiver already taken");
            return ExitCode::from(1);
        }
    };

    eprintln!("[e2e] Connecting (GODARK_EDGE_URL / GDX_EDGE_URL or default) …");

    if let Err(e) = client.connect().await {
        return map_early_error("connect", e);
    }

    let ms_connect = t0.elapsed().as_millis();
    let uid = client
        .user_uuid()
        .map(|u| u.to_string())
        .unwrap_or_default();
    eprintln!("[e2e] Auth + ECDH OK — user_uuid={uid} ({ms_connect} ms)");

    if args.auth_only {
        client.disconnect().await;
        eprintln!("[e2e] --auth-only: skipping orders. Done.");
        return ExitCode::SUCCESS;
    }

    if let Err(e) = client.subscribe(&["orders", "positions"]).await {
        client.disconnect().await;
        return map_early_error("subscribe", e);
    }

    const SYMBOL: &str = "BTC-USDC-PERP";
    const QTY: f64 = 0.01;
    // Far-from-market sell so it rests; testnet has no deviation cap.
    // Override with GODARK_TEST_LIMIT_PRICE on localnet (1000 bps cap).
    let price: f64 = env::var("GODARK_TEST_LIMIT_PRICE")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(999_999.0);

    eprintln!("[e2e] Placing LIMIT SELL {QTY} @ {price} …");

    let ack = match client
        .place_order(
            SYMBOL,
            Side::Sell,
            OrderType::Limit,
            QTY,
            Some(price),
            TimeInForce::Gtc,
            false,
            None,
            None,
        )
        .await
    {
        Ok(a) => a,
        Err(e) => {
            common::print_godark_error("[e2e]", "place_order", &e);
            client.disconnect().await;
            return ExitCode::from(3);
        }
    };

    eprintln!(
        "[e2e] Place OK — order_id={} sequence={}",
        ack.order_id, ack.sequence
    );

    eprintln!("[e2e] Cancelling order …");
    if let Err(e) = client.cancel_order(&ack.order_id, SYMBOL).await {
        common::print_godark_error("[e2e]", "cancel_order", &e);
        client.disconnect().await;
        return ExitCode::from(4);
    }

    eprintln!("[e2e] Cancel OK — order_id={}", ack.order_id);

    client.disconnect().await;

    let total = t0.elapsed().as_millis();
    eprintln!("[e2e] Full encrypted trading path validated ({total} ms total).");

    ExitCode::SUCCESS
}
