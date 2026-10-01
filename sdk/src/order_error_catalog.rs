//! Build order error catalog rows from proto identity + hand-authored reasons.

use std::sync::LazyLock;

use crate::generated::common::v1::OrderErrorCode;
use crate::order_error_reasons::ORDER_ERROR_REASONS;

/// Static description of one canonical `OrderErrorCode` variant.
#[derive(Debug, Clone, Copy)]
pub struct OrderErrorEntry {
    /// Wire code from proto `OrderErrorCode`.
    pub code: u16,
    /// SCREAMING_SNAKE_CASE name (matches JSON wire symbolic strings).
    pub symbolic: &'static str,
    /// Human reason for SDK consumers.
    pub reason: &'static str,
}

pub(crate) fn build_order_error_codes() -> Vec<OrderErrorEntry> {
    ORDER_ERROR_REASONS
        .iter()
        .filter_map(|(symbolic, reason)| {
            let proto_name = format!("ORDER_ERROR_CODE_{symbolic}");
            let variant = OrderErrorCode::from_str_name(&proto_name)?;
            Some(OrderErrorEntry {
                code: variant as u16,
                symbolic,
                reason,
            })
        })
        .collect()
}

pub static ORDER_ERROR_CODES: LazyLock<Vec<OrderErrorEntry>> =
    LazyLock::new(build_order_error_codes);
