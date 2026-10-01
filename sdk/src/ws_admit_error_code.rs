//! Edge WebSocket admit errors (`7xxx`) — sibling to trade ACK tables.

pub use crate::ws_admit_error_catalog::{WsAdmitErrorEntry, WS_ADMIT_ERROR_CODES};

#[must_use]
pub fn find(code: u16) -> Option<WsAdmitErrorEntry> {
    WS_ADMIT_ERROR_CODES
        .iter()
        .copied()
        .find(|e| e.code == code)
}

#[must_use]
pub fn resolve_message(error_code: Option<u16>, fallback: &str) -> String {
    error_code
        .and_then(find)
        .map(|e| e.reason.to_string())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_known_admit_code() {
        assert_eq!(
            resolve_message(Some(7001), "not authenticated"),
            "Sign in before sending requests on this connection."
        );
    }

    #[test]
    fn resolve_message_only_fallback() {
        assert_eq!(
            resolve_message(None, "legacy english only"),
            "legacy english only"
        );
    }
}
