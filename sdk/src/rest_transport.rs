//! HTTP transport for docs-shaped REST (`/api/v1/*`).

use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde_json::{json, Value};

use crate::error::{GodarkError, Result};

/// Non-zero `code` in a docs REST envelope.
#[derive(Debug)]
pub struct RestEnvelopeError {
    pub code: u16,
    pub message: Option<String>,
}

fn unwrap_envelope(val: &Value) -> std::result::Result<&Value, RestEnvelopeError> {
    let code = val.get("code").and_then(|c| c.as_u64()).unwrap_or(1) as u16;
    if code != 0 {
        return Err(RestEnvelopeError {
            code,
            message: val
                .get("message")
                .and_then(|m| m.as_str())
                .map(String::from),
        });
    }
    val.get("data").ok_or(RestEnvelopeError {
        code: 1500,
        message: Some("missing data".into()),
    })
}

/// Thin [`reqwest::Client`] wrapper.
pub struct RestTransport {
    client: reqwest::Client,
    base: String,
}

impl RestTransport {
    pub fn new(base_url: impl Into<String>) -> Self {
        Self {
            client: reqwest::Client::new(),
            base: base_url.into().trim_end_matches('/').to_string(),
        }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    pub async fn time_public(&self) -> Result<Value> {
        let r = self
            .client
            .get(self.url("/api/v1/time"))
            .send()
            .await
            .map_err(|e| GodarkError::Connection(format!("GET /time: {e}")))?;
        let v = parse_ok_json(r).await?;
        data_clone_from_env(&v)
    }

    pub async fn auth_token_document_body(
        &self,
        grant_type: &str,
        client_id: &str,
        client_secret: &str,
        passphrase: &str,
    ) -> Result<Value> {
        let body = json!({
            "grant_type": grant_type,
            "client_id": client_id,
            "client_secret": client_secret,
            "passphrase": passphrase,
        });
        post_json_envelope(&self.client, self.url("/api/v1/auth/token"), None, body).await
    }

    pub async fn auth_token_legacy_token(&self, token: &str) -> Result<Value> {
        let body = json!({ "token": token });
        post_json_envelope(&self.client, self.url("/api/v1/auth/token"), None, body).await
    }

    pub async fn session_setup(&self, bearer: &str, client_ecdh_pubkey: &str) -> Result<Value> {
        let body = json!({ "client_ecdh_pubkey": client_ecdh_pubkey });
        post_json_envelope(
            &self.client,
            self.url("/api/v1/session/setup"),
            Some(bearer),
            body,
        )
        .await
    }

    pub async fn post_orders_encrypted(&self, bearer: &str, body: Value) -> Result<Value> {
        post_json_envelope(&self.client, self.url("/api/v1/orders"), Some(bearer), body).await
    }

    pub async fn delete_orders_encrypted(
        &self,
        bearer: &str,
        order_id: &str,
        body: Value,
    ) -> Result<Value> {
        let url = format!("{}/api/v1/orders/{}", self.base, order_id);
        signed_delete_json(&self.client, url, bearer, body).await
    }

    pub async fn patch_orders_encrypted(
        &self,
        bearer: &str,
        order_id: &str,
        body: Value,
    ) -> Result<Value> {
        let url = format!("{}/api/v1/orders/{}", self.base, order_id);
        signed_patch_json(&self.client, url, bearer, body).await
    }

    /// Phase A1 docs alias: `DELETE /api/v1/orders?client_order_id=...` (encrypted body).
    pub async fn delete_orders_by_client_order_id(
        &self,
        bearer: &str,
        client_order_id: &str,
        body: Value,
    ) -> Result<Value> {
        let url = format!(
            "{}/api/v1/orders?client_order_id={}",
            self.base,
            urlencode(client_order_id)
        );
        signed_delete_json(&self.client, url, bearer, body).await
    }

    /// Docs alias: `GET /api/v1/orders?client_order_id=...` (plaintext lookup).
    pub async fn get_order_by_client_order_id(
        &self,
        bearer: &str,
        client_order_id: &str,
    ) -> Result<Value> {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|e| GodarkError::Connection(e.to_string()))?,
        );
        let r = self
            .client
            .get(format!("{}/api/v1/orders", self.base))
            .query(&[("client_order_id", client_order_id)])
            .headers(h)
            .send()
            .await
            .map_err(|e| GodarkError::Connection(format!("GET order by coid: {e}")))?;
        let v = parse_ok_json(r).await?;
        data_clone_from_env(&v)
    }

    pub async fn get_order(&self, bearer: &str, order_id: &str) -> Result<Value> {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|e| GodarkError::Connection(e.to_string()))?,
        );
        let r = self
            .client
            .get(format!("{}/api/v1/orders/{}", self.base, order_id))
            .headers(h)
            .send()
            .await
            .map_err(|e| GodarkError::Connection(format!("GET order: {e}")))?;
        let v = parse_ok_json(r).await?;
        data_clone_from_env(&v)
    }

    /// Phase B (Zone A): edge stays stateless and never decrypts. After the SDK
    /// decrypts the encrypted place ACK locally, it posts the
    /// `(client_order_id, order_id)` mapping here so subsequent coid-based
    /// resolution (`?client_order_id=` lookups, `cancel_order_by_client_id`)
    /// works on the edge index.
    pub async fn register_client_order_mapping(
        &self,
        bearer: &str,
        client_order_id: &str,
        order_id: &str,
    ) -> Result<Value> {
        let body = json!({
            "client_order_id": client_order_id,
            "order_id": order_id,
        });
        post_json_envelope(
            &self.client,
            self.url("/api/v1/orders/_register_coid"),
            Some(bearer),
            body,
        )
        .await
    }

    pub async fn revoke_token(&self, bearer: &str) -> Result<Value> {
        post_json_envelope(
            &self.client,
            self.url("/api/v1/auth/token/revoke"),
            Some(bearer),
            json!({}),
        )
        .await
    }

    /// `GET /api/v1/auth/me` — returns the authenticated user's profile.
    /// The response is a flat JSON object (no envelope wrapper).
    pub async fn get_auth_me(&self, bearer: &str) -> Result<Value> {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|e| GodarkError::Connection(e.to_string()))?,
        );
        let r = self
            .client
            .get(self.url("/api/v1/auth/me"))
            .headers(h)
            .send()
            .await
            .map_err(|e| GodarkError::Connection(format!("GET /auth/me: {e}")))?;
        parse_ok_json(r).await
    }

    /// `GET /api/v1/shielded-pool/balances/{owner}` — returns on-chain balance
    /// snapshot. The response is NOT envelope-wrapped; fields are at the JSON root.
    pub async fn get_shielded_pool_balances(&self, bearer: &str, owner: &str) -> Result<Value> {
        let mut h = HeaderMap::new();
        h.insert(
            AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {bearer}"))
                .map_err(|e| GodarkError::Connection(e.to_string()))?,
        );
        let r = self
            .client
            .get(format!(
                "{}/api/v1/shielded-pool/balances/{}",
                self.base, owner
            ))
            .headers(h)
            .send()
            .await
            .map_err(|e| GodarkError::Connection(format!("GET /shielded-pool/balances: {e}")))?;
        parse_ok_json(r).await
    }
}

fn data_clone_from_env(v: &Value) -> Result<Value> {
    let data = unwrap_envelope(v).map_err(|e| {
        GodarkError::Authentication(format!("REST {:?}", e.message.unwrap_or_default()))
    })?;
    Ok(data.clone())
}

/// Minimal percent-encoder for query values (avoids dragging the `url` crate as a runtime dep).
fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        let c = b as char;
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

async fn parse_ok_json(r: reqwest::Response) -> Result<Value> {
    let txt = r
        .text()
        .await
        .map_err(|e| GodarkError::Connection(format!("read body: {e}")))?;
    serde_json::from_str(&txt).map_err(|e| GodarkError::Connection(format!("json: {e}")))
}

async fn post_json_envelope(
    client: &reqwest::Client,
    url: String,
    bearer: Option<&str>,
    body: Value,
) -> Result<Value> {
    let mut req = client
        .post(url)
        .header(CONTENT_TYPE, "application/json")
        .json(&body);
    if let Some(b) = bearer {
        req = req.bearer_auth(b);
    }
    let r = req
        .send()
        .await
        .map_err(|e| GodarkError::Connection(format!("post: {e}")))?;
    let v = parse_ok_json(r).await?;
    data_clone_from_env(&v)
}

async fn signed_delete_json(
    client: &reqwest::Client,
    url: String,
    bearer: &str,
    body: Value,
) -> Result<Value> {
    let r = client
        .delete(url)
        .header(CONTENT_TYPE, "application/json")
        .bearer_auth(bearer)
        .json(&body)
        .send()
        .await
        .map_err(|e| GodarkError::Connection(format!("delete order: {e}")))?;
    let v = parse_ok_json(r).await?;
    data_clone_from_env(&v)
}

async fn signed_patch_json(
    client: &reqwest::Client,
    url: String,
    bearer: &str,
    body: Value,
) -> Result<Value> {
    let r = client
        .patch(url)
        .header(CONTENT_TYPE, "application/json")
        .bearer_auth(bearer)
        .json(&body)
        .send()
        .await
        .map_err(|e| GodarkError::Connection(format!("patch order: {e}")))?;
    let v = parse_ok_json(r).await?;
    data_clone_from_env(&v)
}
