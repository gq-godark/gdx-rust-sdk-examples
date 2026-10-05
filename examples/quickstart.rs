//! GoDark Rust SDK — Quickstart Example
//!
//! Place one post-only limit sell at least 500 above the live mark, wait,
//! then cancel that order. Quantity is 0.001. This sample does not place
//! MARKET, IOC, or non-post-only orders. If no live mark is available it
//! exits before sending anything.
//!
//! ```text
//! cargo run --release --example quickstart
//! ```
//!
//! Reads credentials from `.env` (or the OS environment):
//!   GODARK_API_KEY_ID=gdk_...
//!   GODARK_API_SECRET=...
//!   GODARK_PASSPHRASE=...
//!   # GODARK_EDGE_URL=...   (optional; default Environment::Testnet)

use godark::{
    Confirmation, Environment, GodarkClient, GodarkError, OrderType, Side, TimeInForce,
};

#[path = "dotenv.rs"]
mod dotenv;
#[path = "live_mark.rs"]
mod live_mark;

use live_mark::{post_only, sell_limit, SYMBOL};

#[tokio::main]
async fn main() -> Result<(), GodarkError> {
    dotenv::load_dotenv();

    let mut builder = GodarkClient::builder().environment(Environment::Testnet);
    if let Some(base_url) = dotenv::env_first(&["GODARK_EDGE_URL", "GDX_EDGE_URL"]) {
        builder = builder.base_url(dotenv::edge_ws_url(&base_url));
    }
    if let Some(legacy) = dotenv::env_first(&["GODARK_API_KEY", "GDX_API_KEY"]) {
        builder = builder.api_key(legacy);
        if let Some(account) = dotenv::env_first(&["GODARK_ACCOUNT", "GDX_ACCOUNT"]) {
            builder = builder.account(account);
        }
    } else {
        let api_key_id =
            dotenv::env_first(&["GODARK_API_KEY_ID", "GDX_API_KEY_ID"]).ok_or_else(|| {
                GodarkError::Config("Set GODARK_API_KEY_ID or legacy GODARK_API_KEY".into())
            })?;
        let api_secret =
            dotenv::env_first(&["GODARK_API_SECRET", "GDX_API_SECRET"]).ok_or_else(|| {
                GodarkError::Config("Set GODARK_API_SECRET or legacy GODARK_API_KEY".into())
            })?;
        let passphrase =
            dotenv::env_first(&["GODARK_PASSPHRASE", "GDX_PASSPHRASE"]).ok_or_else(|| {
                GodarkError::Config("Set GODARK_PASSPHRASE or legacy GODARK_API_KEY".into())
            })?;
        builder = builder
            .api_key_id(api_key_id)
            .api_secret(api_secret)
            .passphrase(passphrase);
    }
    let config = builder.build()?;

    let mut rest = live_mark::connect_rest().await?;
    if let Err(e) = live_mark::cancel_sample_leftovers(&mut rest).await {
        eprintln!("{e}");
        return Err(GodarkError::Config(e));
    }
    let before_ids = match live_mark::open_order_ids(&mut rest).await {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("{e}");
            return Err(GodarkError::Config(e));
        }
    };
    println!("open orders before: {}", before_ids.len());

    let snap = rest.get_positions().await?;
    let marks = live_mark::marks_from_positions(&snap);
    let mark = match live_mark::resolve_mark(&rest, &marks).await {
        Ok(mark) => mark,
        Err(e) => {
            eprintln!("{e}");
            rest.disconnect().await?;
            return Err(GodarkError::Config(e));
        }
    };
    if let Err(e) = live_mark::flatten_small_positions(&mut rest, mark).await {
        eprintln!("{e}");
        rest.disconnect().await?;
        return Err(GodarkError::Config(e));
    }

    let mut client = GodarkClient::new(config);
    client.connect().await?;

    let account = client
        .account()
        .map(|id| id.to_string())
        .unwrap_or_default();
    println!("Connected as account {account}");

    // Book confirmation waits on private order updates; subscribe first.
    client.subscribe(&["orders"]).await?;

    let sell_px = sell_limit(mark);
    println!(
        "Placing post-only limit SELL qty={} @ {sell_px} (mark ceil {})",
        live_mark::QTY,
        mark.ceil
    );
    let ack = match client
        .place_order_with_options(
            SYMBOL,
            Side::Sell,
            OrderType::Limit,
            Some(live_mark::QTY),
            Some(sell_px.as_str()),
            TimeInForce::Gtc,
            false,
            None,
            None,
            Confirmation::Book,
            post_only(),
        )
        .await
    {
        Ok(ack) if ack.success => {
            println!(
                "Place OK -- order_id={} (post-only limit SELL @ {sell_px})",
                ack.order_id
            );
            ack
        }
        Ok(ack) => {
            eprintln!(
                "Place failed: {:?}",
                ack.error.or(ack.error_code)
            );
            client.disconnect().await;
            rest.disconnect().await?;
            return Err(GodarkError::Config("place rejected".into()));
        }
        Err(e) => {
            dotenv::print_order_error("Order rejected", &e);
            client.disconnect().await;
            let _ = rest.disconnect().await;
            return Err(e);
        }
    };

    live_mark::wait_before_cancel().await;
    let cancel = match client.cancel_order(&ack.order_id, SYMBOL).await {
        Ok(cancel) if cancel.success => cancel,
        Ok(cancel) => {
            eprintln!(
                "cancel rejected: {:?}",
                cancel.error.or(cancel.error_code)
            );
            client.disconnect().await;
            let _ = rest.disconnect().await;
            return Err(GodarkError::Config("cancel rejected".into()));
        }
        Err(e) => {
            dotenv::print_order_error("Cancel rejected", &e);
            client.disconnect().await;
            let _ = rest.disconnect().await;
            return Err(e);
        }
    };
    println!("cancel OK -- order_id={}", cancel.order_id);

    if let Err(e) = live_mark::refresh_rest(&mut rest).await {
        eprintln!("{e}");
        client.disconnect().await;
        return Err(GodarkError::Config(e));
    }
    if let Err(e) = live_mark::flatten_small_positions(&mut rest, mark).await {
        eprintln!("{e}");
        client.disconnect().await;
        let _ = rest.disconnect().await;
        return Err(GodarkError::Config(e));
    }
    if let Err(e) =
        live_mark::assert_no_new_orders_or_positions(&mut rest, &before_ids, &[ack.order_id]).await
    {
        eprintln!("{e}");
        client.disconnect().await;
        let _ = rest.disconnect().await;
        return Err(GodarkError::Config(e));
    }

    client.disconnect().await;
    rest.disconnect().await?;
    println!("Disconnected");
    Ok(())
}
