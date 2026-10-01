//! Build WS admit error catalog rows from proto identity + hand-authored reasons.

use std::sync::LazyLock;

use crate::generated::common::v1::WsAdmitErrorCode;
use crate::ws_admit_error_reasons::WS_ADMIT_ERROR_REASONS;

#[derive(Debug, Clone, Copy)]
pub struct WsAdmitErrorEntry {
    pub code: u16,
    pub symbolic: &'static str,
    pub reason: &'static str,
}

pub(crate) fn build_ws_admit_error_codes() -> Vec<WsAdmitErrorEntry> {
    WS_ADMIT_ERROR_REASONS
        .iter()
        .filter_map(|(symbolic, reason)| {
            let proto_name = format!("WS_ADMIT_ERROR_CODE_{symbolic}");
            let variant = WsAdmitErrorCode::from_str_name(&proto_name)?;
            Some(WsAdmitErrorEntry {
                code: variant as u16,
                symbolic,
                reason,
            })
        })
        .collect()
}

pub static WS_ADMIT_ERROR_CODES: LazyLock<Vec<WsAdmitErrorEntry>> =
    LazyLock::new(build_ws_admit_error_codes);
