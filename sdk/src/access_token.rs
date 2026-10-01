//! Helpers for GoDark REST access JWTs minted by `POST /api/v1/auth/token`.
//!
//! Verification is performed by the edge; the SDK only decodes the payload to
//! read stable claims such as `sub` (internal account).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;

use crate::types::AccountId;

/// Parse the internal account from a compact access JWT's `sub` claim.
///
/// Returns `None` when the token is not a three-part JWT or `sub` is missing /
/// not a valid 32-byte base58 account. Signature is not verified — callers should only use tokens
/// returned by the edge `auth/token` response.
pub fn account_from_access_token_jwt(token: &str) -> Option<AccountId> {
    let payload_b64 = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let sub = claims.get("sub")?.as_str()?;
    sub.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jwt_with_sub(sub: &str) -> String {
        let payload = format!(r#"{{"sub":"{sub}","scope":"trade"}}"#);
        let body = URL_SAFE_NO_PAD.encode(payload.as_bytes());
        format!("eyJhbGciOiJIUzI1NiJ9.{body}.sig")
    }

    #[test]
    fn parses_sub_claim() {
        let sub = "11111111111111111111111111111111";
        let got = account_from_access_token_jwt(&jwt_with_sub(sub)).unwrap();
        assert_eq!(got.to_string(), sub);
    }

    #[test]
    fn rejects_malformed_token() {
        assert!(account_from_access_token_jwt("not-a-jwt").is_none());
        assert!(account_from_access_token_jwt(&jwt_with_sub("not-an-account")).is_none());
    }
}
