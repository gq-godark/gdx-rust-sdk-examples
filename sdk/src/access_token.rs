//! Helpers for GoDark REST access JWTs minted by `POST /api/v1/auth/token`.
//!
//! Verification is performed by the edge; the SDK only decodes the payload to
//! read stable claims such as `sub` (internal user UUID).

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::Value;
use uuid::Uuid;

/// Parse the internal user UUID from a compact access JWT's `sub` claim.
///
/// Returns `None` when the token is not a three-part JWT or `sub` is missing /
/// not a valid UUID. Signature is not verified — callers should only use tokens
/// returned by the edge `auth/token` response.
pub fn user_uuid_from_access_token_jwt(token: &str) -> Option<Uuid> {
    let payload_b64 = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let sub = claims.get("sub")?.as_str()?;
    Uuid::parse_str(sub).ok()
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
        let sub = "3b026a17-fd27-4a7d-bb93-048e30e4900e";
        let got = user_uuid_from_access_token_jwt(&jwt_with_sub(sub)).unwrap();
        assert_eq!(got.to_string(), sub);
    }

    #[test]
    fn rejects_malformed_token() {
        assert!(user_uuid_from_access_token_jwt("not-a-jwt").is_none());
        assert!(user_uuid_from_access_token_jwt(&jwt_with_sub("not-a-uuid")).is_none());
    }
}
