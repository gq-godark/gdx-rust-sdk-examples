// Trading enums with protobuf integer conversions — mirrors Python SDK enums.py

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderType {
    Market,
    Limit,
    PegToMid,
    PegToBid,
    PegToAsk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TimeInForce {
    Gtc,
    Ioc,
    Fok,
    Gtd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderStatus {
    New,
    PartiallyFilled,
    Filled,
    Cancelled,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum OrderUpdateType {
    Open,
    Filled,
    PartiallyFilled,
    Cancelled,
    Rejected,
    Modified,
    CancelRejected,
    ModifyRejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PositionUpdateType {
    Snapshot,
    Open,
    Increase,
    Decrease,
    Close,
    FundingApplied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CancelReason {
    UserRequested,
    IocRemainder,
    FokNotFilled,
    Expired,
    System,
}

// Proto i32 -> enum conversions

impl Side {
    pub fn from_proto(v: i32) -> Self {
        match v {
            1 => Self::Buy,
            2 => Self::Sell,
            _ => Self::Buy,
        }
    }

    pub fn to_proto(self) -> i32 {
        match self {
            Self::Buy => 1,
            Self::Sell => 2,
        }
    }
}

impl OrderType {
    pub fn from_proto(v: i32) -> Self {
        match v {
            1 => Self::Market,
            2 => Self::Limit,
            3 => Self::PegToMid,
            4 => Self::PegToBid,
            5 => Self::PegToAsk,
            _ => Self::Limit,
        }
    }

    pub fn to_proto(self) -> i32 {
        match self {
            Self::Market => 1,
            Self::Limit => 2,
            Self::PegToMid => 3,
            Self::PegToBid => 4,
            Self::PegToAsk => 5,
        }
    }
}

impl TimeInForce {
    pub fn from_proto(v: i32) -> Self {
        match v {
            1 => Self::Gtc,
            2 => Self::Ioc,
            3 => Self::Fok,
            4 => Self::Gtd,
            _ => Self::Gtc,
        }
    }

    pub fn to_proto(self) -> i32 {
        match self {
            Self::Gtc => 1,
            Self::Ioc => 2,
            Self::Fok => 3,
            Self::Gtd => 4,
        }
    }
}

impl OrderStatus {
    pub fn from_proto(v: i32) -> Self {
        match v {
            1 => Self::New,
            2 => Self::PartiallyFilled,
            3 => Self::Filled,
            4 => Self::Cancelled,
            5 => Self::Rejected,
            _ => Self::New,
        }
    }

    pub fn to_proto(self) -> i32 {
        match self {
            Self::New => 1,
            Self::PartiallyFilled => 2,
            Self::Filled => 3,
            Self::Cancelled => 4,
            Self::Rejected => 5,
        }
    }
}

impl OrderUpdateType {
    pub fn from_proto(v: i32) -> Self {
        match v {
            1 => Self::Open,
            2 => Self::Filled,
            3 => Self::PartiallyFilled,
            4 => Self::Cancelled,
            5 => Self::Rejected,
            6 => Self::Modified,
            7 => Self::CancelRejected,
            8 => Self::ModifyRejected,
            _ => Self::Open,
        }
    }

    pub fn to_proto(self) -> i32 {
        match self {
            Self::Open => 1,
            Self::Filled => 2,
            Self::PartiallyFilled => 3,
            Self::Cancelled => 4,
            Self::Rejected => 5,
            Self::Modified => 6,
            Self::CancelRejected => 7,
            Self::ModifyRejected => 8,
        }
    }
}

impl PositionUpdateType {
    pub fn from_proto(v: i32) -> Self {
        match v {
            1 => Self::Snapshot,
            2 => Self::Open,
            3 => Self::Increase,
            4 => Self::Decrease,
            5 => Self::Close,
            6 => Self::FundingApplied,
            _ => Self::Snapshot,
        }
    }

    pub fn to_proto(self) -> i32 {
        match self {
            Self::Snapshot => 1,
            Self::Open => 2,
            Self::Increase => 3,
            Self::Decrease => 4,
            Self::Close => 5,
            Self::FundingApplied => 6,
        }
    }
}

impl CancelReason {
    pub fn from_proto(v: i32) -> Option<Self> {
        match v {
            1 => Some(Self::UserRequested),
            2 => Some(Self::IocRemainder),
            3 => Some(Self::FokNotFilled),
            4 => Some(Self::Expired),
            5 => Some(Self::System),
            _ => None,
        }
    }

    pub fn to_proto(self) -> i32 {
        match self {
            Self::UserRequested => 1,
            Self::IocRemainder => 2,
            Self::FokNotFilled => 3,
            Self::Expired => 4,
            Self::System => 5,
        }
    }
}

/// Maps a request type string to its proto integer.
pub fn request_type_to_proto(s: &str) -> i32 {
    match s {
        "place" => 1,
        "cancel" => 2,
        "modify" => 3,
        "subscribe" => 4,
        "signing" => 5,
        "update_leverage" => 8,
        "mass_quote" => 10,
        "batch_cancel" => 11,
        "batch_modify" => 12,
        "spline_place" => 17,
        "spline_anchor_update" => 18,
        _ => 0,
    }
}

/// Maps a response message type string to its proto integer.
///
/// Source of truth: `gdx-core/crates/gdx-wire/src/convert/common.rs`
/// (`response_message_type_to_proto`). The discriminant `2` is reserved
/// (retired `PositionUpdate`); positions now flow as `PositionsSnapshot = 7`.
///
/// **CRITICAL**: this mapping is used to build the AAD (Additional
/// Authenticated Data) that protects every encrypted response from the
/// sequencer. If a string maps to the wrong integer (or to the default
/// `0` for an unknown type) the SDK will rebuild a different AAD than
/// the sequencer used and AES-GCM authentication will fail with
/// `aead::Error`. Keep this list in sync with the proto enum on the
/// sequencer side and with the Python/JS/C++ SDKs.
pub fn response_message_type_to_proto(s: &str) -> i32 {
    match s {
        "order_update" => 1,
        "system_health" => 3,
        "ack" => 4,
        "open_orders_snapshot" => 5,
        "order_history_snapshot" => 6,
        "positions_snapshot" => 7,
        "balance_and_position" => 8,
        "account_margin_update" => 9,
        "mass_quote_ack" => 10,
        "batch_cancel_ack" => 11,
        "batch_modify_ack" => 12,
        "spline_order_ack" => 15,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_side_from_proto_roundtrip() {
        assert_eq!(Side::Buy.to_proto(), 1);
        assert_eq!(Side::from_proto(1), Side::Buy);
        assert_eq!(Side::Sell.to_proto(), 2);
        assert_eq!(Side::from_proto(2), Side::Sell);
    }

    #[test]
    fn test_order_type_from_proto_roundtrip() {
        let variants = [
            OrderType::Market,
            OrderType::Limit,
            OrderType::PegToMid,
            OrderType::PegToBid,
            OrderType::PegToAsk,
        ];
        for v in variants {
            assert_eq!(OrderType::from_proto(v.to_proto()), v);
        }
    }

    #[test]
    fn test_time_in_force_from_proto_roundtrip() {
        let variants = [
            TimeInForce::Gtc,
            TimeInForce::Ioc,
            TimeInForce::Fok,
            TimeInForce::Gtd,
        ];
        for v in variants {
            assert_eq!(TimeInForce::from_proto(v.to_proto()), v);
        }
    }

    #[test]
    fn test_order_status_from_proto_roundtrip() {
        let variants = [
            OrderStatus::New,
            OrderStatus::PartiallyFilled,
            OrderStatus::Filled,
            OrderStatus::Cancelled,
            OrderStatus::Rejected,
        ];
        for v in variants {
            assert_eq!(OrderStatus::from_proto(v.to_proto()), v);
        }
    }

    #[test]
    fn test_order_update_type_from_proto_all_variants() {
        let variants = [
            OrderUpdateType::Open,
            OrderUpdateType::Filled,
            OrderUpdateType::PartiallyFilled,
            OrderUpdateType::Cancelled,
            OrderUpdateType::Rejected,
            OrderUpdateType::Modified,
            OrderUpdateType::CancelRejected,
            OrderUpdateType::ModifyRejected,
        ];
        for v in variants {
            assert_eq!(OrderUpdateType::from_proto(v.to_proto()), v);
        }
    }

    #[test]
    fn test_position_update_type_from_proto_all_variants() {
        let variants = [
            PositionUpdateType::Snapshot,
            PositionUpdateType::Open,
            PositionUpdateType::Increase,
            PositionUpdateType::Decrease,
            PositionUpdateType::Close,
            PositionUpdateType::FundingApplied,
        ];
        for v in variants {
            assert_eq!(PositionUpdateType::from_proto(v.to_proto()), v);
        }
    }

    #[test]
    fn test_position_update_type_funding_applied_wire_value() {
        assert_eq!(PositionUpdateType::FundingApplied.to_proto(), 6);
        assert_eq!(
            PositionUpdateType::from_proto(6),
            PositionUpdateType::FundingApplied
        );
    }

    #[test]
    fn test_cancel_reason_from_proto_all_variants() {
        let variants = [
            CancelReason::UserRequested,
            CancelReason::IocRemainder,
            CancelReason::FokNotFilled,
            CancelReason::Expired,
            CancelReason::System,
        ];
        for v in variants {
            assert_eq!(CancelReason::from_proto(v.to_proto()), Some(v));
        }
        assert_eq!(CancelReason::from_proto(0), None);
        assert_eq!(CancelReason::from_proto(99), None);
    }

    #[test]
    fn test_unknown_proto_value_returns_default() {
        assert_eq!(Side::from_proto(99), Side::Buy);
    }

    #[test]
    fn test_request_type_to_proto() {
        assert_eq!(request_type_to_proto("place"), 1);
        assert_eq!(request_type_to_proto("cancel"), 2);
        assert_eq!(request_type_to_proto("modify"), 3);
        assert_eq!(request_type_to_proto("update_leverage"), 8);
        assert_eq!(request_type_to_proto("spline_place"), 17);
        assert_eq!(request_type_to_proto("spline_anchor_update"), 18);
        assert_eq!(request_type_to_proto("unknown"), 0);
    }

    #[test]
    fn test_response_message_type_to_proto() {
        // Names must match the wire-side enum
        // (`gdx-core/crates/gdx-wire/src/convert/common.rs`). A regression
        // here causes `aead::Error` decrypts on the affected push type
        // because the SDK's reconstructed AAD diverges from the sequencer's.
        assert_eq!(response_message_type_to_proto("order_update"), 1);
        // 2 reserved (retired position_update)
        assert_eq!(response_message_type_to_proto("system_health"), 3);
        assert_eq!(response_message_type_to_proto("ack"), 4);
        assert_eq!(response_message_type_to_proto("open_orders_snapshot"), 5);
        assert_eq!(response_message_type_to_proto("order_history_snapshot"), 6);
        assert_eq!(response_message_type_to_proto("positions_snapshot"), 7);
        assert_eq!(response_message_type_to_proto("spline_order_ack"), 15);
        assert_eq!(response_message_type_to_proto("unknown"), 0);
    }

    #[test]
    fn test_enum_serde_roundtrip() {
        let json = serde_json::to_string(&Side::Buy).expect("serialize");
        let back: Side = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, Side::Buy);
    }

    #[test]
    fn test_enum_clone_debug() {
        let s = Side::Buy;
        let c = s;
        assert_eq!(c, s);
        let _ = format!("{:?}", Side::Sell);
    }
}
