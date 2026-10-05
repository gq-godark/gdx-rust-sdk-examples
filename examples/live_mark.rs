//! Live mark and post-only prices for the trading samples.
//!
//! No hardcoded BTC mark. A price env var, when set, is the mark. Otherwise
//! the mark comes from a position snapshot that actually carries one, then
//! from Devnet open interest (`oi_ccy / open_interest` for BTC). If neither
//! is available the caller must exit before placing.

#![allow(dead_code)]

use godark::{
    GodarkError, GodarkRestClient, OrderType, PlaceOrderOptions, PositionsSnapshot, Side,
    TimeInForce,
};

pub const SYMBOL: &str = "BTC-USDC-PERP";
pub const BTC_SYMBOL_ID: u64 = 1;
/// Sample size cap: at most 0.001, at most 4 decimal places.
pub const QTY: &str = "0.001";
const OFFSET: u64 = 500;

#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub floor: u64,
    pub ceil: u64,
}

pub fn sell_limit(mark: Mark) -> String {
    (mark.ceil + OFFSET).to_string()
}

pub fn buy_limit(mark: Mark) -> Option<String> {
    buy_limit_deeper(mark, 0)
}

/// Post-only buy at least 500 below the mark, plus `extra` further away.
pub fn buy_limit_deeper(mark: Mark, extra: u64) -> Option<String> {
    let gap = OFFSET.checked_add(extra)?;
    mark.floor.checked_sub(gap).map(|px| px.to_string())
}

pub fn post_only() -> PlaceOrderOptions {
    PlaceOrderOptions {
        post_only: true,
        ..PlaceOrderOptions::default()
    }
}

fn reduce_only() -> PlaceOrderOptions {
    PlaceOrderOptions {
        reduce_only: true,
        post_only: false,
        ..PlaceOrderOptions::default()
    }
}

pub async fn wait_before_cancel() {
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
}

pub fn rest_from_env() -> Result<GodarkRestClient, GodarkError> {
    let mut builder = GodarkRestClient::builder();
    if let Some(url) = crate::dotenv::env_first(&[
        "GODARK_REST_URL",
        "GDX_REST_URL",
        "GODARK_EDGE_URL",
        "GDX_EDGE_URL",
    ]) {
        builder = builder.rest_base_url(url);
    }
    if let Some(account) = crate::dotenv::env_first(&["GODARK_ACCOUNT", "GDX_ACCOUNT"]) {
        builder = builder.account(account);
    }
    if let Some(legacy) = crate::dotenv::env_first(&["GODARK_API_KEY", "GDX_API_KEY"]) {
        builder = builder.api_key(legacy);
    } else {
        let api_key_id = crate::dotenv::env_first(&["GODARK_API_KEY_ID", "GDX_API_KEY_ID"])
            .ok_or_else(|| GodarkError::Config("Set GODARK_API_KEY_ID or GDX_API_KEY_ID".into()))?;
        let api_secret = crate::dotenv::env_first(&["GODARK_API_SECRET", "GDX_API_SECRET"])
            .ok_or_else(|| GodarkError::Config("Set GODARK_API_SECRET or GDX_API_SECRET".into()))?;
        let passphrase = crate::dotenv::env_first(&["GODARK_PASSPHRASE", "GDX_PASSPHRASE"])
            .ok_or_else(|| GodarkError::Config("Set GODARK_PASSPHRASE or GDX_PASSPHRASE".into()))?;
        builder = builder
            .api_key_id(api_key_id)
            .api_secret(api_secret)
            .passphrase(passphrase);
    }
    builder.build()
}

pub async fn connect_rest() -> Result<GodarkRestClient, GodarkError> {
    let mut client = rest_from_env()?;
    client.connect().await?;
    Ok(client)
}

/// Mint a new access token on an existing client. Does not revoke the old one.
pub async fn refresh_rest(rest: &mut GodarkRestClient) -> Result<(), String> {
    rest.connect().await.map_err(|e| format!("refresh auth: {e}"))
}

/// Cancel resting BTC orders at the sample size from a previous failed run.
pub async fn cancel_sample_leftovers(rest: &mut GodarkRestClient) -> Result<(), String> {
    let orders = rest
        .get_open_orders()
        .await
        .map_err(|e| format!("open orders: {e}"))?;
    for row in orders.rows {
        if row.symbol_id != BTC_SYMBOL_ID || !same_qty(&row.remaining_qty, QTY) {
            continue;
        }
        println!("cancelling leftover sample order {}", row.order_id);
        let cancel = rest
            .cancel_order(&row.order_id, SYMBOL)
            .await
            .map_err(|e| format!("cancel leftover {}: {e}", row.order_id))?;
        if !cancel.success {
            return Err(format!(
                "cancel leftover {} rejected: {:?}",
                row.order_id,
                cancel.error.or(cancel.error_code)
            ));
        }
    }
    Ok(())
}

fn same_qty(raw: &str, target: &str) -> bool {
    match (parse_pos(raw.trim().trim_start_matches(['+', '-'])), parse_pos(target)) {
        (Some(a), Some(b)) => scale_up(a.0, a.1, a.1.max(b.1)) == scale_up(b.0, b.1, a.1.max(b.1)),
        _ => false,
    }
}

/// Resolve a mark. Price env is optional and is itself a mark, not a limit.
/// With no env, use snapshot marks, then open interest. Empty means place nothing.
pub async fn resolve_mark(
    rest: &GodarkRestClient,
    snapshot_marks: &[String],
) -> Result<Mark, String> {
    if let Some(raw) = crate::dotenv::env_first(&["GODARK_E2E_PRICE", "GDX_E2E_PRICE", "GDX_LIVE_PRICE"]) {
        return mark_from_decimal(&raw)
            .ok_or_else(|| "price env is not a positive decimal mark".into());
    }
    for raw in snapshot_marks {
        if let Some(mark) = mark_from_decimal(raw) {
            println!("mark from position snapshot: floor={} ceil={}", mark.floor, mark.ceil);
            return Ok(mark);
        }
    }
    let oi = rest
        .get_open_interest()
        .await
        .map_err(|e| format!("open interest: {e}"))?;
    match mark_from_open_interest(&oi) {
        Some(mark) => {
            println!(
                "mark from open interest (oi_ccy/open_interest): floor={} ceil={}",
                mark.floor, mark.ceil
            );
            Ok(mark)
        }
        None => Err(
            "no live mark: position snapshot has none and open interest has no BTC oi_ccy/open_interest"
                .into(),
        ),
    }
}

pub fn marks_from_positions(snap: &PositionsSnapshot) -> Vec<String> {
    snap.rows
        .iter()
        .filter(|row| row.symbol_id == BTC_SYMBOL_ID)
        .filter_map(|row| row.mark_price.clone().filter(|m| !m.is_empty()))
        .collect()
}

pub fn position_open(size: &str) -> bool {
    !decimal_is_zero(size)
}

pub async fn open_order_ids(rest: &mut GodarkRestClient) -> Result<Vec<String>, String> {
    let orders = rest
        .get_open_orders()
        .await
        .map_err(|e| format!("open orders: {e}"))?;
    Ok(orders
        .rows
        .into_iter()
        .map(|row| row.order_id)
        .filter(|id| !id.is_empty() && id != "0")
        .collect())
}

pub async fn assert_no_new_orders_or_positions(
    rest: &mut GodarkRestClient,
    before_ids: &[String],
    ours: &[String],
) -> Result<(), String> {
    let mut last = String::from("account check did not run");
    for attempt in 0..5 {
        match check_once(rest, before_ids, ours).await {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = e;
                if attempt + 1 < 5 {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
    }
    Err(last)
}

async fn check_once(
    rest: &mut GodarkRestClient,
    before_ids: &[String],
    ours: &[String],
) -> Result<(), String> {
    let open = open_order_ids(rest).await?;
    for id in ours {
        if open.iter().any(|row| row == id) {
            return Err(format!("order {id} still open"));
        }
    }
    let newcomers: Vec<&String> = open.iter().filter(|id| !before_ids.contains(id)).collect();
    if !newcomers.is_empty() {
        return Err(format!("new open order(s) left behind: {newcomers:?}"));
    }
    let positions = rest
        .get_positions()
        .await
        .map_err(|e| format!("positions: {e}"))?;
    for row in &positions.rows {
        if position_open(&row.size) {
            return Err(format!(
                "position still open symbol={} side={:?} size={}",
                row.symbol_id, row.side, row.size
            ));
        }
    }
    Ok(())
}

/// Close a tiny position this process may have opened. Refuses sizes above 0.01.
pub async fn flatten_small_positions(
    rest: &mut GodarkRestClient,
    mark: Mark,
) -> Result<(), String> {
    let positions = rest
        .get_positions()
        .await
        .map_err(|e| format!("positions: {e}"))?;
    for row in positions.rows {
        if !position_open(&row.size) {
            continue;
        }
        if row.symbol_id != BTC_SYMBOL_ID {
            return Err(format!(
                "open position on symbol {} size {}; sample will not touch it",
                row.symbol_id, row.size
            ));
        }
        let qty = close_qty(&row.size)?;
        let (side, price) = match row.side {
            Side::Buy => (
                Side::Sell,
                buy_limit(mark).ok_or("mark too small to flatten a long")?,
            ),
            Side::Sell => (Side::Buy, sell_limit(mark)),
        };
        println!(
            "flatten reduce-only {:?} qty={qty} @ {price} (position was {:?} {})",
            side, row.side, row.size
        );
        let ack = rest
            .place_order_with_options(
                SYMBOL,
                side,
                OrderType::Limit,
                Some(&qty),
                Some(&price),
                TimeInForce::Gtc,
                false,
                None,
                None,
                None,
                reduce_only(),
            )
            .await
            .map_err(|e| format!("flatten place: {e}"))?;
        if !ack.success {
            return Err(format!(
                "flatten rejected: {:?}",
                ack.error.or(ack.error_code)
            ));
        }
        wait_before_cancel().await;
        if !ack.order_id.is_empty() && ack.order_id != "0" {
            // A fill makes cancel fail; the caller checks that the position is gone.
            let _ = rest.cancel_order(&ack.order_id, SYMBOL).await;
        }
    }
    Ok(())
}

fn close_qty(size: &str) -> Result<String, String> {
    let abs = size.trim().trim_start_matches(['+', '-']);
    if decimal_is_zero(abs) {
        return Err("empty position size".into());
    }
    if decimal_gt(abs, "0.01") {
        return Err(format!(
            "position size {size} is above the sample cap; not flattening"
        ));
    }
    truncate_4(abs).ok_or_else(|| format!("position size {size} is not a decimal"))
}

pub fn mark_from_decimal(raw: &str) -> Option<Mark> {
    let (mant, scale) = parse_pos(raw)?;
    if mant == 0 {
        return None;
    }
    let div = pow10(scale)?;
    let floor = mant / div;
    if floor == 0 || floor > u64::MAX as u128 {
        return None;
    }
    let floor = floor as u64;
    let ceil = if mant % div == 0 {
        floor
    } else {
        floor.checked_add(1)?
    };
    Some(Mark { floor, ceil })
}

pub fn mark_from_open_interest(v: &serde_json::Value) -> Option<Mark> {
    let rows = v
        .as_array()
        .or_else(|| v.get("data").and_then(|d| d.as_array()))?;
    for row in rows {
        let sid = row
            .get("symbol_id")
            .or_else(|| row.get("symbolId"))
            .and_then(|x| x.as_u64());
        let symbol = row
            .get("symbol")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        if sid != Some(BTC_SYMBOL_ID) && symbol != SYMBOL {
            continue;
        }
        let oi = json_num(row.get("open_interest")?)?;
        let ccy = json_num(row.get("oi_ccy").or_else(|| row.get("oiCcy"))?)?;
        if let Some(mark) = mark_from_quotient(&ccy, &oi) {
            return Some(mark);
        }
    }
    None
}

fn mark_from_quotient(numer: &str, denom: &str) -> Option<Mark> {
    let n = parse_pos(numer)?;
    let d = parse_pos(denom)?;
    if d.0 == 0 {
        return None;
    }
    let scale = n.1.max(d.1);
    let n = scale_up(n.0, n.1, scale)?;
    let d = scale_up(d.0, d.1, scale)?;
    let floor = n / d;
    if floor == 0 || floor > u64::MAX as u128 {
        return None;
    }
    let floor = floor as u64;
    let ceil = if n % d == 0 {
        floor
    } else {
        floor.checked_add(1)?
    };
    Some(Mark { floor, ceil })
}

fn json_num(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_pos(raw: &str) -> Option<(u128, u32)> {
    let s = raw.trim();
    if s.is_empty() || s.starts_with('-') {
        return None;
    }
    let (whole, frac) = match s.split_once('.') {
        Some((w, f)) => (w, f),
        None => (s, ""),
    };
    if !whole.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    if whole.is_empty() && frac.is_empty() {
        return None;
    }
    let digits = format!("{}{}", if whole.is_empty() { "0" } else { whole }, frac);
    let mant = digits.parse::<u128>().ok()?;
    Some((mant, frac.len() as u32))
}

fn scale_up(mant: u128, scale: u32, target: u32) -> Option<u128> {
    mant.checked_mul(pow10(target - scale)?)
}

fn pow10(scale: u32) -> Option<u128> {
    if scale > 38 {
        return None;
    }
    let mut v = 1u128;
    for _ in 0..scale {
        v = v.checked_mul(10)?;
    }
    Some(v)
}

fn decimal_is_zero(raw: &str) -> bool {
    let s = raw.trim().trim_start_matches(['+', '-']);
    s.is_empty() || s.chars().all(|c| c == '0' || c == '.')
}

fn decimal_gt(a: &str, b: &str) -> bool {
    let Some((am, as_)) = parse_pos(a) else {
        return false;
    };
    let Some((bm, bs)) = parse_pos(b) else {
        return false;
    };
    let scale = as_.max(bs);
    let Some(am) = scale_up(am, as_, scale) else {
        return false;
    };
    let Some(bm) = scale_up(bm, bs, scale) else {
        return false;
    };
    am > bm
}

fn truncate_4(abs: &str) -> Option<String> {
    let (mant, scale) = parse_pos(abs)?;
    let (mant, scale) = if scale > 4 {
        (mant / pow10(scale - 4)?, 4)
    } else {
        (mant, scale)
    };
    if mant == 0 {
        return None;
    }
    Some(format_dec(mant, scale))
}

fn format_dec(mant: u128, scale: u32) -> String {
    if scale == 0 {
        return mant.to_string();
    }
    let div = pow10(scale).unwrap_or(1);
    let whole = mant / div;
    let frac = mant % div;
    let mut frac_s = format!("{:0width$}", frac, width = scale as usize);
    while frac_s.ends_with('0') {
        frac_s.pop();
    }
    if frac_s.is_empty() {
        whole.to_string()
    } else {
        format!("{whole}.{frac_s}")
    }
}
