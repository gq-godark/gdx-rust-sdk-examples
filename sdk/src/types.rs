// Domain types — mirrors Python SDK types.py

use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::enums::{CancelReason, OrderStatus, OrderUpdateType, PositionUpdateType, Side};

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
    /// Reconnect and session setup succeeded.
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OrderAck {
    pub order_id: String,
    pub success: bool,
    pub sequence: String,
    pub error_code: Option<String>,
    pub error: Option<String>,
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
    pub correlation_id: u128,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PositionUpdate {
    pub user_uuid: Uuid,
    pub symbol_id: u64,
    pub side: Side,
    pub update_type: PositionUpdateType,
    pub size: String,
    pub entry_price: String,
    pub previous_size: String,
    pub fill_price: String,
    pub fill_qty: String,
    pub correlation_id: u128,
    pub timestamp: u64,
}

/// User profile returned by `GET /api/v1/auth/me`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeProfile {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub dynamic_user_id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub wallet_address: String,
    #[serde(default)]
    pub referral_code: String,
    #[serde(default)]
    pub tier: String,
}

/// On-chain balance snapshot from `GET /api/v1/shielded-pool/balances/{owner}`.
///
/// The wire format uses camelCase keys; raw u64 amounts arrive as JSON strings
/// to avoid precision loss in JavaScript clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Balance {
    #[serde(deserialize_with = "deserialize_string_u64")]
    pub wallet_usdt_raw: u64,
    #[serde(deserialize_with = "deserialize_string_u64")]
    pub pending_deposits_raw: u64,
    #[serde(deserialize_with = "deserialize_string_u64")]
    pub shielded_balance_raw: u64,
    pub wallet_usdt_ui: f64,
}

fn deserialize_string_u64<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringU64Visitor;

    impl<'de> de::Visitor<'de> for StringU64Visitor {
        type Value = u64;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a u64 as a string or number")
        }

        fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<u64, E> {
            Ok(v)
        }

        fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<u64, E> {
            u64::try_from(v).map_err(|_| E::custom("negative value"))
        }

        fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<u64, E> {
            Ok(v as u64)
        }

        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<u64, E> {
            v.parse::<u64>().map_err(de::Error::custom)
        }
    }

    deserializer.deserialize_any(StringU64Visitor)
}

/// Why a [`PositionsSnapshot`] was emitted by the sequencer.
///
/// Mirrors `gdx.common.v1.PositionsSnapshotSource`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// Sequencer / MPC node health pulse routed via the trading WS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemHealthUpdate {
    pub total_nodes: u32,
    pub accepting_orders: bool,
    pub ready: u32,
    pub degraded: u32,
    pub exhausted: u32,
    pub warming: u32,
    pub draining: u32,
    pub waiting: u32,
}

/// Updated shielded balance for the authenticated user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BalanceUpdate {
    pub user_uuid: Uuid,
    pub shielded_balance_raw: u64,
    pub timestamp: u64,
}

/// Margin tier transition / recovery for `(owner, symbol_id)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarginAlert {
    pub owner: Uuid,
    pub symbol_id: u64,
    pub tier: u32,
    pub margin_ratio_bps: u32,
    pub mark_price_bps: u64,
    pub liquidation_price_bps: u64,
    pub ts: i64,
    pub state_version: u64,
    /// True when the position recovered to `Healthy` — UI clears the tier
    /// badge for this `(owner, symbol_id)`. `tier` is `Unspecified` here.
    pub recovered: bool,
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

/// Status of a settlement batch tx.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettlementBatchStatus {
    Unspecified,
    Submitted,
    Confirmed,
    Failed,
}

/// Settlement batch lifecycle update from the sequencer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettlementUpdate {
    pub batch_id: u64,
    pub status: SettlementBatchStatus,
    pub tx_signature: String,
    pub timestamp: u64,
    pub affected_user_uuids: Vec<Uuid>,
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
    fn test_position_update_all_fields() {
        let p = PositionUpdate {
            user_uuid: Uuid::nil(),
            symbol_id: 2,
            side: Side::Buy,
            update_type: PositionUpdateType::Increase,
            size: "5".to_string(),
            entry_price: "2.0".to_string(),
            previous_size: "3".to_string(),
            fill_price: "2.1".to_string(),
            fill_qty: "2".to_string(),
            correlation_id: 42,
            timestamp: 12345,
        };
        assert_eq!(p.user_uuid, Uuid::nil());
        assert_eq!(p.symbol_id, 2);
        assert_eq!(p.side, Side::Buy);
        assert_eq!(p.update_type, PositionUpdateType::Increase);
        assert_eq!(p.size, "5");
        assert_eq!(p.entry_price, "2.0");
        assert_eq!(p.previous_size, "3");
        assert_eq!(p.fill_price, "2.1");
        assert_eq!(p.fill_qty, "2");
        assert_eq!(p.correlation_id, 42);
        assert_eq!(p.timestamp, 12345);
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
            correlation_id: 0,
            timestamp: 0,
        };
        let _ = ou.clone();
        let _ = format!("{:?}", ou);

        let pu = PositionUpdate {
            user_uuid: Uuid::nil(),
            symbol_id: 0,
            side: Side::Buy,
            update_type: PositionUpdateType::Snapshot,
            size: "0".into(),
            entry_price: "0".into(),
            previous_size: "0".into(),
            fill_price: "0".into(),
            fill_qty: "0".into(),
            correlation_id: 0,
            timestamp: 0,
        };
        let _ = pu.clone();
        let _ = format!("{:?}", pu);
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
