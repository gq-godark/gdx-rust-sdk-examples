// Domain types for the public SDK surface.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{CancelReason, OrderStatus, OrderType, OrderUpdateType, Side, TimeInForce};

/// Lifecycle notifications for trading client reconnect (and market data reconnect).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconnectEvent {
    /// Transport reported disconnect; reconnect may follow if enabled.
    Disconnected,
    /// About to sleep and retry after a disconnect.
    Attempting {
        /// 1-based reconnect attempt counter for this disconnect episode.
        attempt: u32,
        delay: Duration,
    },
    /// Reconnect and HPKE session setup succeeded.
    Reconnected,
    /// A reconnect attempt failed (another attempt may follow).
    Failed { error: String },
}

/// One row from `GET /api/v1/leverage` — per-symbol leverage setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeverageSetting {
    pub symbol_id: u64,
    pub leverage: u32,
}

/// Cached leverage settings for the authenticated user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeverageSettings {
    #[serde(default)]
    pub settings: Vec<LeverageSetting>,
}

/// Confirmation boundary for high-level [`crate::GodarkClient::place_order`].
///
/// Mirrors the JS SDK `confirmation: "ack" | "book"` option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Confirmation {
    /// Return as soon as the sequencer acknowledges the order. Callers must
    /// consume the order-update stream themselves for rejects / fills.
    Ack,
    /// Wait for a definitive order update (`OPEN` / `REJECTED` / fill / cancel)
    /// and surface post-ack rejects as [`crate::GodarkError::Order`]. Default.
    #[default]
    Book,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAck {
    pub order_id: String,
    pub success: bool,
    pub sequence: String,
    pub error_code: Option<String>,
    pub error: Option<String>,
}

/// One cancel-replace leg of a mass quote. Mirrors the Python SDK leg dict.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MassQuoteLegInput {
    pub side: Side,
    pub price: f64,
    pub quantity: f64,
    /// Resting order to cancel-replace. `None`/`0` = pure place (no cancel target).
    #[serde(default)]
    pub cancel_order_id: Option<u64>,
    /// Defaults to GTC when `None`.
    #[serde(default)]
    pub time_in_force: Option<crate::enums::TimeInForce>,
    /// Required when `time_in_force` = GTD (nanoseconds).
    #[serde(default)]
    pub expiry_time: Option<u64>,
}

/// One amend leg of a batch modify. At least one of `new_price`/`new_quantity`
/// must be set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BatchModifyLegInput {
    pub order_id: u64,
    #[serde(default)]
    pub new_price: Option<f64>,
    #[serde(default)]
    pub new_quantity: Option<f64>,
}

/// Outcome of one cancel-replace leg in a mass-quote batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MassQuoteLegResult {
    pub leg_index: u32,
    /// "open" | "filled" | "failed" | "unspecified" | "unknown".
    pub status: String,
    /// `None` when no cancel target / cancel failed.
    pub cancelled_order_id: Option<String>,
    /// `None` when the replacement failed.
    pub new_order_id: Option<String>,
    pub error_code: Option<u32>,
    /// Number of taker fills this leg produced in relaxed (`post_only = false`)
    /// mode; 0 for a pure rest or a post-only leg.
    pub fill_count: u32,
}

/// Batch-level result of a mass quote: one entry per submitted leg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MassQuoteAck {
    pub success: bool,
    pub sequence: String,
    pub results: Vec<MassQuoteLegResult>,
}

/// Outcome of cancelling one order id in a batch-cancel request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchCancelLegResult {
    pub order_id: String,
    pub cancelled: bool,
    pub error_code: Option<u32>,
}

/// Batch-level result of a batch cancel: one entry per submitted order id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchCancelAck {
    pub success: bool,
    pub sequence: String,
    pub results: Vec<BatchCancelLegResult>,
}

/// Outcome of amending one resting order in a batch-modify request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchModifyLegResult {
    pub order_id: String,
    pub modified: bool,
    pub error_code: Option<u32>,
}

/// Batch-level result of a batch modify: one entry per submitted leg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BatchModifyAck {
    pub success: bool,
    pub sequence: String,
    pub results: Vec<BatchModifyLegResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OrderUpdate {
    pub order_id: String,
    pub user_uuid: Uuid,
    pub symbol_id: u64,
    pub side: Side,
    pub status: OrderStatus,
    pub update_type: OrderUpdateType,
    pub price: String,
    pub quantity: String,
    pub filled_qty: String,
    pub remaining_qty: String,
    pub cum_fill: String,
    pub cancel_reason: Option<CancelReason>,
    pub reject_reason: Option<String>,
    pub msg: Option<String>,
    pub reduce_only: bool,
    pub post_only: bool,
    pub correlation_id: u128,
    pub timestamp: u64,
}

/// Authenticated user profile (`GET /api/v1/auth/me`, session JWT only).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeProfile {
    #[serde(default, deserialize_with = "deserialize_null_string")]
    pub id: String,
    #[serde(default, deserialize_with = "deserialize_null_string")]
    pub dynamic_user_id: String,
    #[serde(default, deserialize_with = "deserialize_null_string")]
    pub email: String,
    #[serde(default, deserialize_with = "deserialize_null_string")]
    pub wallet_address: String,
    #[serde(default, deserialize_with = "deserialize_null_string")]
    pub referral_code: String,
    #[serde(default, deserialize_with = "deserialize_null_string")]
    pub tier: String,
}

fn deserialize_null_string<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// Sequencer trading-collateral snapshot (`BalanceUpdateMessage`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceUpdate {
    pub user_uuid: Uuid,
    /// Collateral in SPL raw token units (6 dp).
    pub balance_raw: u64,
    pub timestamp: u64,
    /// Human-readable internal USDT collateral.
    pub balance: String,
    pub signed_balance_8dp: i64,
    pub free_collateral_8dp: u64,
}

/// Why a [`PositionsSnapshot`] was emitted by the sequencer.
///
/// Mirrors `gdx.common.v1.PositionsSnapshotSource`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionsSnapshotSource {
    Unspecified,
    /// First snapshot delivered after `SubscribePositions`.
    Initial,
    /// Background periodic sweep (default 5s cadence).
    Periodic,
    /// Position-changing fill / flip / close (debounced on the sequencer).
    Event,
}

/// One row of a [`PositionsSnapshot`] — a single open position with the
/// sequencer's mark price and unrealized PnL at snapshot time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionRow {
    pub symbol_id: u64,
    pub side: Side,
    pub size: String,
    pub entry_price: String,
    pub leverage: u32,
    /// Server-computed mark price (Pyth Hermes). `None` when no Pyth tick
    /// has been observed yet for this symbol.
    pub mark_price: Option<String>,
    /// Sign-preserved unrealized PnL at `2 * decimal_places`. Absent iff
    /// `mark_price` is absent.
    pub unrealized_pnl: Option<String>,
    /// Notional (`|size| * mark_price`). Absent iff `mark_price` is absent.
    pub notional: Option<String>,
    /// Pyth `publish_time` for `mark_price` (seconds since epoch).
    pub mark_publish_time_sec: Option<u64>,
}

/// Full per-user positions batch sent by the sequencer (initial /
/// periodic / event-triggered).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PositionsSnapshot {
    pub user_uuid: Uuid,
    pub rows: Vec<PositionRow>,
    /// Sequencer wall-clock (ns) when the batch was assembled.
    pub server_timestamp: u64,
    pub source: PositionsSnapshotSource,
    /// Echoed from the original `SubscribePositions` request — present on
    /// `Initial` snapshots only.
    pub correlation_id: Option<u128>,
}

/// Unified component health report routed via the trading WS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemHealthUpdate {
    pub component_id: String,
    pub state: i32,
    pub serving: bool,
    pub cause: String,
    pub updated_at_nanos: u64,
    pub sequence: u64,
    pub schema_version: u32,
}

/// Funding rate tick for a symbol.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FundingRateUpdate {
    pub symbol_id: u64,
    pub current_rate: String,
    pub predicted_rate: String,
    pub next_funding_time: u64,
    pub timestamp: u64,
}

/// One working order from a [`OpenOrdersSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenOrderRow {
    pub order_id: String,
    pub symbol_id: u64,
    pub side: Side,
    pub order_type: OrderType,
    pub price: String,
    pub quantity: String,
    pub filled_qty: String,
    pub remaining_qty: String,
    pub order_status: OrderStatus,
    pub time_in_force: TimeInForce,
    pub leverage: u32,
    pub timestamp: u64,
    pub correlation_id: u128,
    pub expiry_time: Option<u64>,
    pub reduce_only: bool,
    pub post_only: bool,
    pub take_profit: Option<String>,
    pub stop_loss: Option<String>,
}

/// Reply to encrypted `POST /api/v1/openOrders` (`GetOpenOrders`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenOrdersSnapshot {
    pub rows: Vec<OpenOrderRow>,
    pub server_timestamp: u64,
    pub correlation_id: u128,
}

/// Authoritative account-level margin summary. All amounts are decimal strings.
/// `free_collateral` is the server-computed amount available to open new orders
/// (already deducts margin reserved by resting orders), so render it directly
/// rather than recomputing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountMarginSummary {
    pub total_collateral: String,
    pub position_margin: String,
    pub reserved_order_margin: String,
    pub free_collateral: String,
}

/// Dedicated account-margin push for a user. Emitted whenever collateral,
/// positions, or resting-order holds change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccountMarginUpdate {
    pub user_uuid: Uuid,
    /// Sequencer wall-clock timestamp when the summary was computed, ns.
    pub server_timestamp: u64,
    /// Absent if the sequencer did not include a summary.
    pub account: Option<AccountMarginSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_ack_construction() {
        let ack = OrderAck {
            order_id: "oid-1".to_string(),
            success: true,
            sequence: "seq-42".to_string(),
            error_code: Some("E1".to_string()),
            error: Some("oops".to_string()),
        };
        assert_eq!(ack.order_id, "oid-1");
        assert!(ack.success);
        assert_eq!(ack.sequence, "seq-42");
        assert_eq!(ack.error_code.as_deref(), Some("E1"));
        assert_eq!(ack.error.as_deref(), Some("oops"));
    }

    #[test]
    fn test_order_update_all_fields() {
        let u = OrderUpdate {
            order_id: "o1".to_string(),
            user_uuid: Uuid::nil(),
            symbol_id: 200,
            side: Side::Sell,
            status: OrderStatus::PartiallyFilled,
            update_type: OrderUpdateType::PartiallyFilled,
            price: "1.5".to_string(),
            quantity: "10".to_string(),
            filled_qty: "3".to_string(),
            remaining_qty: "7".to_string(),
            cum_fill: "4.5".to_string(),
            cancel_reason: Some(CancelReason::UserRequested),
            reject_reason: Some("bad".to_string()),
            msg: Some("detail".to_string()),
            reduce_only: true,
            post_only: false,
            correlation_id: 999,
            timestamp: 1_700_000_000,
        };
        assert_eq!(u.order_id, "o1");
        assert_eq!(u.user_uuid, Uuid::nil());
        assert_eq!(u.symbol_id, 200);
        assert_eq!(u.side, Side::Sell);
        assert_eq!(u.status, OrderStatus::PartiallyFilled);
        assert_eq!(u.update_type, OrderUpdateType::PartiallyFilled);
        assert_eq!(u.price, "1.5");
        assert_eq!(u.quantity, "10");
        assert_eq!(u.filled_qty, "3");
        assert_eq!(u.remaining_qty, "7");
        assert_eq!(u.cum_fill, "4.5");
        assert_eq!(u.cancel_reason, Some(CancelReason::UserRequested));
        assert_eq!(u.reject_reason.as_deref(), Some("bad"));
        assert_eq!(u.correlation_id, 999);
        assert_eq!(u.timestamp, 1_700_000_000);
    }

    #[test]
    fn test_types_clone_and_debug() {
        let ack = OrderAck {
            order_id: "a".into(),
            success: false,
            sequence: "s".into(),
            error_code: None,
            error: None,
        };
        let _ = ack.clone();
        let _ = format!("{:?}", ack);

        let ou = OrderUpdate {
            order_id: "o".into(),
            user_uuid: Uuid::nil(),
            symbol_id: 0,
            side: Side::Buy,
            status: OrderStatus::New,
            update_type: OrderUpdateType::Open,
            price: "0".into(),
            quantity: "0".into(),
            filled_qty: "0".into(),
            remaining_qty: "0".into(),
            cum_fill: "0".into(),
            cancel_reason: None,
            reject_reason: None,
            msg: None,
            reduce_only: false,
            post_only: false,
            correlation_id: 0,
            timestamp: 0,
        };
        let _ = ou.clone();
        let _ = format!("{:?}", ou);
    }

    #[test]
    fn test_types_serde_roundtrip() {
        let ack = OrderAck {
            order_id: "ord".into(),
            success: true,
            sequence: "1".into(),
            error_code: Some("ec".into()),
            error: Some("e".into()),
        };
        let json = serde_json::to_string(&ack).expect("serialize");
        let back: OrderAck = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, ack);
    }
}
