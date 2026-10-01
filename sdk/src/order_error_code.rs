//! Map protobuf ACK `error_code` values to rich SDK errors.
//!
//! Identity (code + symbolic name) comes from proto `OrderErrorCode`; reason
//! strings are hand-authored in `order_error_reasons.rs`.

use crate::error::GodarkError;
pub use crate::order_error_catalog::{OrderErrorEntry, ORDER_ERROR_CODES};

/// Look up an entry by its numeric wire code.
#[must_use]
pub fn find(code: u16) -> Option<OrderErrorEntry> {
    ORDER_ERROR_CODES.iter().copied().find(|e| e.code == code)
}

/// Look up an entry by its SCREAMING_SNAKE_CASE symbolic name.
#[must_use]
pub fn find_symbolic(symbolic: &str) -> Option<OrderErrorEntry> {
    ORDER_ERROR_CODES
        .iter()
        .copied()
        .find(|e| e.symbolic == symbolic)
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
            user_message: None,
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
                user_message: Some(entry.reason.to_string()),
            };
        }
    }
    GodarkError::Order {
        message: format!("order rejected{detail_suffix}"),
        error_code: Some(raw.to_string()),
        user_message: None,
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
            if reason.as_deref().unwrap_or("order rejected") == "order rejected" {
                final_reason =
                    format!("{} ({}, code={})", entry.reason, entry.symbolic, entry.code);
            }
        }
    }

    GodarkError::Order {
        message: final_reason,
        error_code: final_code,
        user_message: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_trade_catalog_omits_frozen_mpc_era_codes() {
        assert_eq!(ORDER_ERROR_CODES.len(), 39);
        for code in [1001, 1006, 3001, 3013] {
            assert!(
                find(code).is_none(),
                "frozen mpc-era {code} must not be in trade table"
            );
        }
        let mut seen = std::collections::HashSet::new();
        for entry in ORDER_ERROR_CODES.iter() {
            assert!(seen.insert(entry.code), "duplicate code {}", entry.code);
        }
    }

    #[test]
    fn find_resolves_every_retail_row() {
        for entry in ORDER_ERROR_CODES.iter() {
            let hit = find(entry.code).expect("missing retail row");
            assert_eq!(hit.symbolic, entry.symbolic);
            assert_eq!(hit.reason, entry.reason);
        }
    }

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
                user_message,
            } => {
                assert_eq!(error_code.as_deref(), Some("PRICE_DEVIATION_TOO_LARGE"));
                assert!(message.contains("oracle"));
                assert!(message.contains("PRICE_DEVIATION_TOO_LARGE"));
                assert!(message.contains("code=2007"));
                assert!(user_message.as_deref().unwrap().contains("oracle"));
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
                user_message,
            } => {
                assert_eq!(error_code.as_deref(), Some("CANCEL_TOO_SOON"));
                assert!(message.contains("cancel"));
                assert!(message.contains("code=2016"));
                assert!(user_message.is_some());
            }
            other => panic!("expected Order, got {other:?}"),
        }
    }
}
