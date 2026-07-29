//! Shared env helpers for in-repo examples (edge URL, Noise XK pin, JWT login).

use std::env;
use std::time::Duration;

use godark::{GodarkConfig, GodarkConfigBuilder, GodarkError, TransportConfig};

pub fn env_first(primary: &str, fallback: &str) -> Option<String> {
    env::var(primary)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| env::var(fallback).ok().filter(|s| !s.trim().is_empty()))
}

pub fn env_first_many(names: &[&str]) -> Option<String> {
    names
        .iter()
        .filter_map(|n| env::var(n).ok())
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
}

pub fn default_edge_url() -> String {
    env_first("GODARK_EDGE_URL", "GDX_EDGE_URL")
        .unwrap_or_else(|| "wss://api.godark-dex.com".into())
}

pub fn noise_pin() -> Option<String> {
    env_first_many(&[
        "GDX_NOISE_STATIC_PUBLIC_KEY",
        "GODARK_NOISE_STATIC_PUBLIC_KEY",
        "GDX_NOISE_STATIC_PUBKEY",
    ])
}

pub fn rest_base_from_edge(edge_ws: &str) -> String {
    let u = edge_ws.trim_end_matches('/');
    if let Some(rest) = u.strip_prefix("wss://") {
        format!("https://{}", rest.split('/').next().unwrap_or(rest))
    } else if let Some(rest) = u.strip_prefix("ws://") {
        format!("http://{}", rest.split('/').next().unwrap_or(rest))
    } else {
        u.to_string()
    }
}

pub async fn issue_ws_token(
    edge_url: &str,
    api_key_id: &str,
    api_secret: &str,
    passphrase: &str,
) -> Result<String, GodarkError> {
    let rest_url = rest_base_from_edge(edge_url);
    let response = reqwest::Client::new()
        .post(format!("{rest_url}/api/v1/auth/token"))
        .json(&serde_json::json!({
            "grant_type": "client_credentials",
            "client_id": api_key_id,
            "client_secret": api_secret,
            "passphrase": passphrase,
        }))
        .send()
        .await
        .map_err(|e| GodarkError::Authentication(format!("auth/token request failed: {e}")))?;
    let status = response.status();
    let body: serde_json::Value = response
        .json()
        .await
        .map_err(|e| GodarkError::Authentication(format!("invalid auth/token response: {e}")))?;
    let data = body.get("data").unwrap_or(&body);
    data.get("access_token")
        .or_else(|| data.get("token"))
        .and_then(serde_json::Value::as_str)
        .filter(|token| !token.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            GodarkError::Authentication(format!(
                "auth/token returned {status} without access_token"
            ))
        })
}

pub fn apply_noise_pin(mut builder: GodarkConfigBuilder) -> GodarkConfigBuilder {
    if let Some(pin) = noise_pin() {
        builder = builder.noise_static_public_key_hex(pin);
    }
    builder
}

pub async fn local_trading_config(transport: TransportConfig) -> Result<GodarkConfig, GodarkError> {
    let edge = default_edge_url();
    let mut builder = GodarkConfigBuilder::new()
        .base_url(&edge)
        .auto_reconnect(false)
        .transport(transport);
    builder = apply_noise_pin(builder);

    let api_key_id = env_first("GODARK_API_KEY_ID", "GDX_API_KEY_ID");
    let api_secret = env_first("GODARK_API_SECRET", "GDX_API_SECRET");
    let passphrase = env_first("GODARK_PASSPHRASE", "GDX_PASSPHRASE");
    if let (Some(id), Some(sec), Some(pass)) = (api_key_id, api_secret, passphrase) {
        match issue_ws_token(&edge, &id, &sec, &pass).await {
            Ok(token) => return builder.api_key(token).build(),
            Err(_) => {
                return builder
                    .api_key_id(id)
                    .api_secret(sec)
                    .passphrase(pass)
                    .build();
            }
        }
    }

    if let Some(legacy) = env_first_many(&["GODARK_API_KEY", "GDX_API_KEY"]) {
        if let Some(uid) = env_first_many(&["GODARK_USER_UUID", "GDX_USER_UUID"]) {
            builder = builder.user_uuid(uid);
        }
        return builder.api_key(legacy).build();
    }

    Err(GodarkError::Config(
        "set GODARK_API_KEY_ID, GODARK_API_SECRET, GODARK_PASSPHRASE (or GODARK_API_KEY)".into(),
    ))
}

#[allow(dead_code)]
pub fn is_transient_order_err(e: &GodarkError) -> bool {
    match e {
        GodarkError::Order { message, .. } => {
            let m = message.to_lowercase();
            m.contains("busy")
                || m.contains("unavailable")
                || m.contains("out of sync")
                || m.contains("refresh")
        }
        _ => false,
    }
}

#[allow(dead_code)]
pub async fn cancel_with_retry(
    client: &mut godark::GodarkClient,
    order_id: &str,
    symbol: &str,
) -> Result<godark::OrderAck, GodarkError> {
    for attempt in 0..8 {
        match client.cancel_order(order_id, symbol).await {
            Ok(ack) => return Ok(ack),
            Err(e) => {
                if !is_transient_order_err(&e) || attempt == 7 {
                    return Err(e);
                }
                tokio::time::sleep(Duration::from_millis(750 * (attempt as u64 + 1))).await;
            }
        }
    }
    unreachable!()
}
