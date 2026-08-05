//! Mirror of the canonical `OrderErrorCode` enum from
//! `gdx-protocol/src/order_error.rs` so the Rust SDK can produce informative
//! messages for protobuf-encoded ACK rejections (which carry only a numeric
//! `error_code` on the wire).
//!
//! The protocol crate is internal to the trading core; clients embed this
//! standalone copy so adding a new variant on the sequencer side requires
//! appending a row to [`ORDER_ERROR_CODES`] (preserving numeric codes; the
//! Rust enum in `gdx-protocol` is the source of truth).

use crate::error::GodarkError;

/// Static description of one canonical `OrderErrorCode` variant.
#[derive(Debug, Clone, Copy)]
pub struct OrderErrorEntry {
    /// Wire code from `gdx-protocol::OrderErrorCode::raw()`.
    pub code: u16,
    /// SCREAMING_SNAKE_CASE name (matches `OrderErrorCode::as_json_str()`).
    pub symbolic: &'static str,
    /// Human reason copied from the canonical `#[error(...)]` annotation.
    pub reason: &'static str,
}

/// All canonical order-error codes the sequencer can emit. Keep in sync with
/// `gdx-protocol/src/order_error.rs`.
pub const ORDER_ERROR_CODES: &[OrderErrorEntry] = &[
    // 1xxx -- Node / MPC
    OrderErrorEntry {
        code: 1001,
        symbolic: "TRIPLE_EXHAUSTED",
        reason: "Beaver triple store exhausted",
    },
    OrderErrorEntry {
        code: 1002,
        symbolic: "RANDOM_BIT_EXHAUSTED",
        reason: "random bit store exhausted",
    },
    OrderErrorEntry {
        code: 1003,
        symbolic: "MPC_PROTOCOL_ERROR",
        reason: "MPC protocol error",
    },
    OrderErrorEntry {
        code: 1004,
        symbolic: "MPC_TIMEOUT",
        reason: "MPC session timeout",
    },
    OrderErrorEntry {
        code: 1005,
        symbolic: "MPC_CONFIG_ERROR",
        reason: "MPC configuration error",
    },
    OrderErrorEntry {
        code: 1006,
        symbolic: "MPC_OPS_LIMIT_EXCEEDED",
        reason: "MPC ops limit exceeded",
    },
    // 2xxx -- Risk / validation
    OrderErrorEntry {
        code: 2001,
        symbolic: "RISK_CHECK_FAILED",
        reason: "pre-trade risk check failed",
    },
    OrderErrorEntry {
        code: 2002,
        symbolic: "INSUFFICIENT_COLLATERAL",
        reason: "insufficient collateral",
    },
    OrderErrorEntry {
        code: 2003,
        symbolic: "ORDER_NOT_FOUND",
        reason: "order not found in book",
    },
    OrderErrorEntry {
        code: 2004,
        symbolic: "DUPLICATE_ORDER_ID",
        reason: "duplicate order ID",
    },
    OrderErrorEntry {
        code: 2005,
        symbolic: "INSUFFICIENT_LIQUIDITY",
        reason: "insufficient liquidity",
    },
    OrderErrorEntry {
        code: 2006,
        symbolic: "POSITION_UNDER_LIQUIDATION",
        reason: "position is under active liquidation",
    },
    OrderErrorEntry {
        code: 2007,
        symbolic: "PRICE_DEVIATION_TOO_LARGE",
        reason: "order price too far from oracle price",
    },
    OrderErrorEntry {
        code: 2008,
        symbolic: "LEVERAGE_EXCEEDS_MAX",
        reason: "leverage exceeds instrument max",
    },
    OrderErrorEntry {
        code: 2009,
        symbolic: "INSTRUMENT_HALTED",
        reason: "instrument halted -- not currently accepting orders",
    },
    OrderErrorEntry {
        code: 2010,
        symbolic: "LIQUIDITY_POOL_WITHDRAW_COOLDOWN",
        reason: "withdrawal cooldown active",
    },
    OrderErrorEntry {
        code: 2011,
        symbolic: "LIQUIDITY_POOL_PAUSED",
        reason: "liquidity pool paused",
    },
    OrderErrorEntry {
        code: 2012,
        symbolic: "LIQUIDITY_POOL_ILLIQUID",
        reason: "insufficient pool liquidity for withdrawal",
    },
    OrderErrorEntry {
        code: 2013,
        symbolic: "BELOW_MIN_NOTIONAL",
        reason: "order notional below tier minimum",
    },
    OrderErrorEntry {
        code: 2014,
        symbolic: "ORDER_EXCEEDS_COLLATERAL",
        reason: "order size exceeds collateral value limits",
    },
    OrderErrorEntry {
        code: 2015,
        symbolic: "MARGIN_INSUFFICIENT",
        reason: "insufficient margin for this trade",
    },
    OrderErrorEntry {
        code: 2016,
        symbolic: "CANCEL_TOO_SOON",
        reason: "cancel rejected — order must rest before cancellation",
    },
    OrderErrorEntry {
        code: 2017,
        symbolic: "STP_AGGRESSOR_HALTED",
        reason: "self-trade prevention halted aggressor",
    },
    OrderErrorEntry {
        code: 2020,
        symbolic: "LEVERAGE_UPDATE_MARGIN_INSUFFICIENT",
        reason: "leverage update rejected — insufficient margin",
    },
    OrderErrorEntry {
        code: 2021,
        symbolic: "ADJUST_MARGIN_NO_POSITION",
        reason: "no position found for margin adjustment",
    },
    OrderErrorEntry {
        code: 2022,
        symbolic: "ADJUST_MARGIN_INSUFFICIENT_FREE",
        reason: "insufficient free collateral for margin deposit",
    },
    OrderErrorEntry {
        code: 2023,
        symbolic: "ADJUST_MARGIN_INSUFFICIENT_EXTRA",
        reason: "insufficient extra margin to withdraw",
    },
    OrderErrorEntry {
        code: 2024,
        symbolic: "ADJUST_MARGIN_BREACHES_WARNING",
        reason: "margin removal would breach safety threshold",
    },
    OrderErrorEntry {
        code: 2025,
        symbolic: "FOK_NOT_FILLED",
        reason: "FOK order not fully fillable",
    },
    OrderErrorEntry {
        code: 2026,
        symbolic: "GTD_EXPIRED",
        reason: "GTD order expired",
    },
    // 3xxx -- Sequencer
    OrderErrorEntry {
        code: 3001,
        symbolic: "ACK_TIMEOUT",
        reason: "ACK collection timed out",
    },
    OrderErrorEntry {
        code: 3002,
        symbolic: "ACK_THRESHOLD_NOT_MET",
        reason: "ACK threshold not met",
    },
    OrderErrorEntry {
        code: 3003,
        symbolic: "SEQUENCER_NOT_PRIMARY",
        reason: "sequencer is standby, not primary",
    },
    OrderErrorEntry {
        code: 3004,
        symbolic: "INSUFFICIENT_MASKS",
        reason: "insufficient input masks for authenticated split",
    },
    OrderErrorEntry {
        code: 3005,
        symbolic: "FANOUT_FAILED",
        reason: "fanout delivery failed",
    },
    OrderErrorEntry {
        code: 3006,
        symbolic: "DESERIALIZATION_FAILED",
        reason: "message deserialization failed",
    },
    OrderErrorEntry {
        code: 3007,
        symbolic: "ALL_NODES_EXHAUSTED",
        reason: "all MPC nodes have exhausted precompute pools",
    },
    OrderErrorEntry {
        code: 3008,
        symbolic: "SESSION_EXPIRED",
        reason: "E2E session expired or not established",
    },
    OrderErrorEntry {
        code: 3009,
        symbolic: "E2E_DECRYPTION_FAILED",
        reason: "E2E decryption failed (session key mismatch)",
    },
    OrderErrorEntry {
        code: 3010,
        symbolic: "SHIELD_SUBMIT_RPC_FAILED",
        reason: "shield transaction rejected by Solana RPC",
    },
    OrderErrorEntry {
        code: 3011,
        symbolic: "SEQUENCER_BUSY",
        reason: "sequencer busy -- try again",
    },
    OrderErrorEntry {
        code: 3012,
        symbolic: "SEQUENCE_GAP",
        reason: "order sequence gap -- missing preceding sequencer mutation",
    },
    OrderErrorEntry {
        code: 3013,
        symbolic: "MPC_UNAVAILABLE",
        reason: "MPC nodes unavailable -- system cannot process orders",
    },
    // 4xxx -- Fencing / hot standby
    OrderErrorEntry {
        code: 4001,
        symbolic: "EPOCH_STALE",
        reason: "fencing epoch is stale",
    },
    // 9xxx -- catch-all
    OrderErrorEntry {
        code: 9999,
        symbolic: "INTERNAL_ERROR",
        reason: "internal processing error",
    },
];

/// Look up an entry by its numeric wire code.
#[must_use]
pub fn find(code: u16) -> Option<&'static OrderErrorEntry> {
    ORDER_ERROR_CODES.iter().find(|e| e.code == code)
}

/// Look up an entry by its SCREAMING_SNAKE_CASE symbolic name.
#[must_use]
pub fn find_symbolic(symbolic: &str) -> Option<&'static OrderErrorEntry> {
    ORDER_ERROR_CODES.iter().find(|e| e.symbolic == symbolic)
}

/// Build a rich `GodarkError::Order` from a numeric code (typically a protobuf
/// `AckMessage.error_code`). Mapped codes get their symbolic name in
/// `error_code` and the human reason in the message; unknown codes fall back
/// to a numeric string + the generic "order rejected" message.
///
/// When `detail` is set (e.g. `AckMessage.reject_text` / order-update `msg`),
/// it is appended after the canonical reason.
pub(crate) fn make_order_error_from_code(
    numeric: Option<u32>,
    detail: Option<&str>,
) -> GodarkError {
    let detail_suffix = detail
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| format!(": {s}"))
        .unwrap_or_default();
    let Some(raw) = numeric else {
        return GodarkError::Order {
            message: format!("order rejected{detail_suffix}"),
            error_code: None,
        };
    };
    if let Ok(narrow) = u16::try_from(raw) {
        if let Some(entry) = find(narrow) {
            return GodarkError::Order {
                message: format!(
                    "{} ({}, code={}){detail_suffix}",
                    entry.reason, entry.symbolic, entry.code
                ),
                error_code: Some(entry.symbolic.to_string()),
            };
        }
    }
    GodarkError::Order {
        message: format!("order rejected{detail_suffix}"),
        error_code: Some(raw.to_string()),
    }
}

/// Build a rich `GodarkError::Order` for the JSON ack path. The wire JSON may
/// carry a reason string and either a symbolic or numeric `error_code`; we
/// keep the caller-supplied reason but upgrade it (and the code) to the
/// canonical name when only a numeric code is present.
pub(crate) fn make_order_error_from_json(
    reason: Option<String>,
    code: Option<String>,
) -> GodarkError {
    let mut final_reason = reason
        .clone()
        .unwrap_or_else(|| "order rejected".to_string());
    let mut final_code = code.clone();

    if let Some(raw_code) = code {
        // First, try numeric lookup so the symbolic name is preserved when
        // the wire format is a protobuf-style decimal string.
        if let Ok(parsed) = raw_code.parse::<u32>() {
            if let Ok(narrow) = u16::try_from(parsed) {
                if let Some(entry) = find(narrow) {
                    final_code = Some(entry.symbolic.to_string());
                    if reason.as_deref().unwrap_or("order rejected") == "order rejected" {
                        final_reason =
                            format!("{} ({}, code={})", entry.reason, entry.symbolic, entry.code);
                    }
                }
            }
        } else if let Some(entry) = find_symbolic(&raw_code) {
            // Already symbolic — only enrich the reason if the caller didn't
            // supply a meaningful one.
            if reason.as_deref().unwrap_or("order rejected") == "order rejected" {
                final_reason =
                    format!("{} ({}, code={})", entry.reason, entry.symbolic, entry.code);
            }
        }
    }

    GodarkError::Order {
        message: final_reason,
        error_code: final_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_known_code() {
        let entry = find(2007).expect("price deviation entry");
        assert_eq!(entry.symbolic, "PRICE_DEVIATION_TOO_LARGE");
        assert!(entry.reason.contains("oracle"));
    }

    #[test]
    fn unknown_code_returns_none() {
        assert!(find(7777).is_none());
    }

    #[test]
    fn make_from_numeric_known() {
        let err = make_order_error_from_code(Some(2007), None);
        match err {
            GodarkError::Order {
                message,
                error_code,
            } => {
                assert_eq!(error_code.as_deref(), Some("PRICE_DEVIATION_TOO_LARGE"));
                assert!(message.contains("oracle"));
                assert!(message.contains("PRICE_DEVIATION_TOO_LARGE"));
                assert!(message.contains("code=2007"));
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }

    #[test]
    fn cancel_too_soon_is_mapped() {
        let entry = find(2016).expect("CANCEL_TOO_SOON entry");
        assert_eq!(entry.symbolic, "CANCEL_TOO_SOON");
        let err = make_order_error_from_code(Some(2016), None);
        match err {
            GodarkError::Order {
                message,
                error_code,
            } => {
                assert_eq!(error_code.as_deref(), Some("CANCEL_TOO_SOON"));
                assert!(message.contains("rest"));
                assert!(message.contains("code=2016"));
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }

    #[test]
    fn make_from_numeric_unknown() {
        let err = make_order_error_from_code(Some(7777), None);
        match err {
            GodarkError::Order {
                message,
                error_code,
            } => {
                assert_eq!(error_code.as_deref(), Some("7777"));
                assert_eq!(message, "order rejected");
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }

    #[test]
    fn make_from_numeric_none() {
        let err = make_order_error_from_code(None, None);
        match err {
            GodarkError::Order {
                message,
                error_code,
            } => {
                assert!(error_code.is_none());
                assert_eq!(message, "order rejected");
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }

    #[test]
    fn make_from_numeric_appends_detail() {
        let err = make_order_error_from_code(Some(2007), Some("far from mark"));
        match err {
            GodarkError::Order { message, .. } => {
                assert!(message.ends_with(": far from mark"));
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }

    #[test]
    fn make_from_json_numeric_string_upgrades_to_symbolic() {
        let err = make_order_error_from_json(None, Some("2015".into()));
        match err {
            GodarkError::Order {
                message,
                error_code,
            } => {
                assert_eq!(error_code.as_deref(), Some("MARGIN_INSUFFICIENT"));
                assert!(message.contains("insufficient margin"));
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }

    #[test]
    fn make_from_json_keeps_caller_reason() {
        let err = make_order_error_from_json(Some("custom: too much".into()), Some("2015".into()));
        match err {
            GodarkError::Order {
                message,
                error_code,
            } => {
                assert_eq!(error_code.as_deref(), Some("MARGIN_INSUFFICIENT"));
                assert_eq!(message, "custom: too much");
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }

    #[test]
    fn make_from_json_symbolic_passthrough() {
        let err = make_order_error_from_json(None, Some("PRICE_DEVIATION_TOO_LARGE".into()));
        match err {
            GodarkError::Order {
                message,
                error_code,
            } => {
                assert_eq!(error_code.as_deref(), Some("PRICE_DEVIATION_TOO_LARGE"));
                assert!(message.contains("oracle"));
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }
}
