//! REST-only trader demo — auth, snapshots, then one post-only limit.
//!
//! Places a post-only BUY at least 500 below the live mark, size 0.001,
//! modifies it further away, waits, and cancels that order id. No MARKET,
//! IOC, or non-post-only order. Exits before sending if no mark is available.
//!
//! ```text
//! cargo run --release --example full_trader_rest
//! ```

use godark::{OrderType, Side, TimeInForce};

#[path = "dotenv.rs"]
mod dotenv;
#[path = "live_mark.rs"]
mod live_mark;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenv::load_dotenv();

    let mut client = live_mark::connect_rest().await?;
    let account_id = client.account().ok_or("account missing after connect")?;
    println!(
        "identity: account={account_id} scope={:?}",
        client.token_scope()
    );

    live_mark::cancel_sample_leftovers(&mut client).await?;
    let before_ids = live_mark::open_order_ids(&mut client).await?;
    println!("open orders before: {}", before_ids.len());
    let positions = client.get_positions().await?;
    println!("positions: {} row(s)", positions.rows.len());
    let account = client.get_account().await?;
    if let Some(s) = account.summary {
        println!(
            "account free_collateral={} total_collateral={}",
            s.free_collateral, s.total_collateral
        );
    }

    let marks = live_mark::marks_from_positions(&positions);
    let mark = live_mark::resolve_mark(&client, &marks).await?;
    live_mark::flatten_small_positions(&mut client, mark).await?;

    let limit_price = live_mark::buy_limit(mark).ok_or("mark too small for a post-only buy")?;
    let modify_price =
        live_mark::buy_limit_deeper(mark, 500).ok_or("mark too small to modify the buy")?;
    println!(
        "Placing post-only limit BUY qty={} @ {limit_price} (mark floor {})",
        live_mark::QTY,
        mark.floor
    );
    let ack = client
        .place_order_with_options(
            live_mark::SYMBOL,
            Side::Buy,
            OrderType::Limit,
            Some(live_mark::QTY),
            Some(limit_price.as_str()),
            TimeInForce::Gtc,
            false,
            None,
            None,
            Some("sdk-rust-rest-demo".into()),
            live_mark::post_only(),
        )
        .await?;
    if !ack.success {
        return Err(format!("place rejected: {:?}", ack.error.or(ack.error_code)).into());
    }
    println!("placed order_id={} success={}", ack.order_id, ack.success);

    live_mark::refresh_rest(&mut client).await?;
    let modify = client
        .modify_order(
            &ack.order_id,
            live_mark::SYMBOL,
            Some(modify_price.as_str()),
            None,
            None,
        )
        .await?;
    if !modify.success {
        let _ = client.cancel_order(&ack.order_id, live_mark::SYMBOL).await;
        return Err(format!("modify rejected: {:?}", modify.error.or(modify.error_code)).into());
    }
    println!("modified success={} @ {modify_price}", modify.success);

    live_mark::wait_before_cancel().await;
    live_mark::refresh_rest(&mut client).await?;
    let cancel = client.cancel_order(&ack.order_id, live_mark::SYMBOL).await?;
    if !cancel.success {
        return Err(format!("cancel rejected: {:?}", cancel.error.or(cancel.error_code)).into());
    }
    println!("cancelled success={}", cancel.success);

    live_mark::flatten_small_positions(&mut client, mark).await?;
    live_mark::assert_no_new_orders_or_positions(&mut client, &before_ids, &[ack.order_id]).await?;

    client.disconnect().await?;
    Ok(())
}
