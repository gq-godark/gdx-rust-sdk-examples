// Client configuration — builder pattern for GodarkClient

use std::collections::HashMap;
use std::env;
use std::time::Duration;

use uuid::Uuid;

use crate::error::GodarkError;

/// Default edge base URL (host only). The transport appends `/ws/v1` at
/// connect time.
///
/// Public mainnet is not currently exposed; testnet is the live network for
/// SDK users today and is the SDK default. For local development, override
/// to a localnet edge (`ws://127.0.0.1:4000`) via the `base_url` builder,
/// `GODARK_EDGE_URL`, or `GDX_EDGE_URL`. Either `<host>` or `<host>/ws/v1`
/// resolve to the same endpoint.
const DEFAULT_EDGE_BASE_URL: &str = "wss://api.godark-dex.com";

/// Canonical default perps (shared across SDKs); see `shared/symbols.json`.
const DEFAULT_SYMBOLS_JSON: &str = include_str!("../shared/symbols.json");

/// Tunable WebSocket transport (TLS verify, headers, timeouts).
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// When `true`, accept invalid TLS certificates (dev only; not for production).
    pub tls_skip_verify: bool,
    pub extra_headers: HashMap<String, String>,
    pub connect_timeout: Duration,
    pub command_timeout: Duration,
    /// No inbound message for this duration triggers disconnect (see transport heartbeat).
    pub stale_timeout: Duration,
    pub heartbeat_interval: Duration,
    /// When true (default), send public-docs `{id, op, args}` frames and
    /// normalize inbound `{id, op, code, ...}` replies to legacy shapes.
    pub use_docs_wire: bool,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            tls_skip_verify: false,
            extra_headers: HashMap::new(),
            connect_timeout: Duration::from_secs(30),
            command_timeout: Duration::from_secs(30),
            stale_timeout: Duration::from_secs(60),
            heartbeat_interval: Duration::from_secs(30),
            use_docs_wire: true,
        }
    }
}

/// Resolved configuration for a GodarkClient.
#[derive(Debug, Clone)]
pub struct GodarkConfig {
    pub auth_token: String,
    pub base_url: String,
    pub auto_reconnect: bool,
    pub symbol_map: HashMap<String, u64>,
    pub transport: TransportConfig,
    /// Pre-configured user UUID. When the edge auth response does not include
    /// `user_uuid`, the SDK falls back to this value.  Resolved from
    /// `.user_uuid()` builder call or `GODARK_USER_UUID` / `GDX_USER_UUID`.
    pub user_uuid: Option<Uuid>,
}

/// Builder for GodarkClient configuration.
pub struct GodarkConfigBuilder {
    api_key: Option<String>,
    api_key_id: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
    base_url: Option<String>,
    auto_reconnect: bool,
    symbol_map: HashMap<String, u64>,
    transport: TransportConfig,
    user_uuid: Option<Uuid>,
}

impl GodarkConfigBuilder {
    pub fn new() -> Self {
        let symbols: HashMap<String, u64> =
            serde_json::from_str(DEFAULT_SYMBOLS_JSON).expect("default symbols.json must be valid");

        Self {
            api_key: None,
            api_key_id: None,
            api_secret: None,
            passphrase: None,
            base_url: None,
            auto_reconnect: true,
            symbol_map: symbols,
            transport: TransportConfig::default(),
            user_uuid: None,
        }
    }

    pub fn transport(mut self, transport: TransportConfig) -> Self {
        self.transport = transport;
        self
    }

    pub fn api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn api_key_id(mut self, id: impl Into<String>) -> Self {
        self.api_key_id = Some(id.into());
        self
    }

    pub fn api_secret(mut self, secret: impl Into<String>) -> Self {
        self.api_secret = Some(secret.into());
        self
    }

    /// User-chosen API key passphrase (required with key pair; also reads
    /// `GODARK_PASSPHRASE` / `GDX_PASSPHRASE`).
    pub fn passphrase(mut self, pp: impl Into<String>) -> Self {
        self.passphrase = Some(pp.into());
        self
    }

    pub fn base_url(mut self, url: impl Into<String>) -> Self {
        self.base_url = Some(url.into());
        self
    }

    pub fn auto_reconnect(mut self, enabled: bool) -> Self {
        self.auto_reconnect = enabled;
        self
    }

    /// Set the user UUID explicitly. Required when the edge auth response does
    /// not return `user_uuid` (e.g. localnet / static-key auth). Falls back to
    /// `GODARK_USER_UUID` / `GDX_USER_UUID` environment variables at build time.
    pub fn user_uuid(mut self, uuid: impl Into<String>) -> Self {
        self.user_uuid = Uuid::parse_str(&uuid.into()).ok();
        self
    }

    pub fn symbol(mut self, name: impl Into<String>, id: u64) -> Self {
        self.symbol_map.insert(name.into(), id);
        self
    }

    pub fn build(self) -> Result<GodarkConfig, GodarkError> {
        let auth_token = match (self.api_key_id, self.api_secret, self.api_key) {
            (Some(id), Some(secret), None) => {
                let pp = resolve_passphrase(self.passphrase.as_deref()).ok_or_else(|| {
                    GodarkError::Config(
                        "passphrase is required when using api_key_id and api_secret".into(),
                    )
                })?;
                format!("{id}:{secret}:{pp}")
            }
            (None, None, Some(key)) => {
                if self
                    .passphrase
                    .as_ref()
                    .is_some_and(|pp| !pp.trim().is_empty())
                {
                    return Err(GodarkError::Config(
                        "passphrase must not be set when using legacy api_key".into(),
                    ));
                }
                key
            }
            (Some(_), None, _) | (None, Some(_), _) => {
                return Err(GodarkError::Config(
                    "api_key_id and api_secret must be provided together".into(),
                ));
            }
            (Some(_), Some(_), Some(_)) => {
                return Err(GodarkError::Config(
                    "use either api_key or (api_key_id, api_secret), not both".into(),
                ));
            }
            (None, None, None) => {
                return Err(GodarkError::Config(
                    "provide api_key or both api_key_id and api_secret".into(),
                ));
            }
        };

        let base_url = resolve_edge_base_url(self.base_url.as_deref());

        let user_uuid = self.user_uuid.or_else(resolve_user_uuid_env);

        Ok(GodarkConfig {
            auth_token,
            base_url,
            auto_reconnect: self.auto_reconnect,
            symbol_map: self.symbol_map,
            transport: self.transport,
            user_uuid,
        })
    }
}

impl Default for GodarkConfigBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve passphrase: constructor arg wins, then env vars.
pub fn resolve_passphrase(explicit: Option<&str>) -> Option<String> {
    if let Some(v) = explicit {
        let trimmed = v.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    for key in &["GODARK_PASSPHRASE", "GDX_PASSPHRASE"] {
        if let Ok(v) = env::var(key) {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

/// Resolve user UUID from `GODARK_USER_UUID` or `GDX_USER_UUID` environment variables.
fn resolve_user_uuid_env() -> Option<Uuid> {
    for key in &["GODARK_USER_UUID", "GDX_USER_UUID"] {
        if let Ok(v) = env::var(key) {
            if let Ok(u) = Uuid::parse_str(v.trim()) {
                return Some(u);
            }
        }
    }
    None
}

/// Resolve edge base URL: explicit arg > env vars > production default.
pub fn resolve_edge_base_url(explicit: Option<&str>) -> String {
    if let Some(url) = explicit {
        let trimmed = url.trim();
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    for key in &["GODARK_EDGE_URL", "GDX_EDGE_URL"] {
        if let Ok(v) = env::var(key) {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    DEFAULT_EDGE_BASE_URL.to_string()
}

/// Resolve a base URL to the canonical edge WebSocket endpoint `<base>/ws/v1`.
///
/// Trailing slashes are stripped first, then:
/// - if the input already ends with `/ws/v1` it is returned unchanged;
/// - if the input ends with the legacy `/ws` suffix it is upgraded to `/ws/v1`;
/// - otherwise `/ws/v1` is appended.
pub fn ws_url(base_url: &str) -> String {
    let url = base_url.trim_end_matches('/');
    if url.ends_with("/ws/v1") {
        url.to_string()
    } else if let Some(stripped) = url.strip_suffix("/ws") {
        format!("{stripped}/ws/v1")
    } else {
        format!("{url}/ws/v1")
    }
}

/// Construct the GoMarket WebSocket URL from a base URL.
///
/// Strips a trailing `/ws/v1` or legacy `/ws` suffix from the base before
/// appending `/ws/gomarket`, so that any of `<host>`, `<host>/ws`, or
/// `<host>/ws/v1` resolve to `<host>/ws/gomarket`.
pub fn gomarket_url(base_url: &str) -> String {
    let mut url = base_url.trim_end_matches('/').to_string();
    if let Some(stripped) = url.strip_suffix("/ws/v1") {
        url = stripped.to_string();
    } else if let Some(stripped) = url.strip_suffix("/ws") {
        url = stripped.to_string();
    }
    format!("{url}/ws/gomarket")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::{
        gomarket_url, resolve_edge_base_url, resolve_passphrase, ws_url, GodarkConfigBuilder,
        GodarkError,
    };

    /// Serialize tests that mutate process environment variables.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_builder_api_key_pair() {
        let cfg = GodarkConfigBuilder::new()
            .api_key_id("id")
            .api_secret("secret")
            .passphrase("pp")
            .build()
            .unwrap();
        assert_eq!(cfg.auth_token, "id:secret:pp");
    }

    #[test]
    fn test_builder_api_key_pair_requires_passphrase() {
        let err = GodarkConfigBuilder::new()
            .api_key_id("id")
            .api_secret("secret")
            .build()
            .unwrap_err();
        assert!(matches!(err, GodarkError::Config(ref msg) if msg.contains("passphrase")));
    }

    #[test]
    fn test_builder_legacy_rejects_passphrase() {
        let err = GodarkConfigBuilder::new()
            .api_key("k")
            .passphrase("pp")
            .build()
            .unwrap_err();
        assert!(matches!(err, GodarkError::Config(ref msg) if msg.contains("passphrase")));
    }

    #[test]
    fn test_builder_api_key_pair_passphrase_from_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_g = std::env::var("GODARK_PASSPHRASE").ok();
        let old_x = std::env::var("GDX_PASSPHRASE").ok();
        std::env::remove_var("GODARK_PASSPHRASE");
        std::env::set_var("GDX_PASSPHRASE", "env-pp");

        let cfg = GodarkConfigBuilder::new()
            .api_key_id("id")
            .api_secret("secret")
            .build()
            .unwrap();
        assert_eq!(cfg.auth_token, "id:secret:env-pp");

        if let Some(v) = old_g {
            std::env::set_var("GODARK_PASSPHRASE", v);
        } else {
            std::env::remove_var("GODARK_PASSPHRASE");
        }
        if let Some(v) = old_x {
            std::env::set_var("GDX_PASSPHRASE", v);
        } else {
            std::env::remove_var("GDX_PASSPHRASE");
        }
    }

    #[test]
    fn test_resolve_passphrase_prefers_constructor() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("GDX_PASSPHRASE", "from-env");
        assert_eq!(
            resolve_passphrase(Some("explicit")),
            Some("explicit".into())
        );
        std::env::remove_var("GDX_PASSPHRASE");
    }

    #[test]
    fn test_resolve_passphrase_from_godark_env() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("GODARK_PASSPHRASE", "godark-pw");
        std::env::set_var("GDX_PASSPHRASE", "gdx-pw");
        assert_eq!(resolve_passphrase(None), Some("godark-pw".into()));
        std::env::remove_var("GODARK_PASSPHRASE");
        std::env::remove_var("GDX_PASSPHRASE");
    }

    #[test]
    fn test_builder_legacy_api_key() {
        let cfg = GodarkConfigBuilder::new().api_key("mykey").build().unwrap();
        assert_eq!(cfg.auth_token, "mykey");
    }

    #[test]
    fn test_builder_both_auth_modes_errors() {
        let err = GodarkConfigBuilder::new()
            .api_key("k")
            .api_key_id("id")
            .api_secret("secret")
            .build()
            .unwrap_err();
        assert!(matches!(err, GodarkError::Config(ref msg) if msg.contains("not both")));
    }

    #[test]
    fn test_builder_no_auth_errors() {
        let err = GodarkConfigBuilder::new().build().unwrap_err();
        assert!(matches!(err, GodarkError::Config(ref msg) if msg.contains("provide api_key")));
    }

    #[test]
    fn test_builder_incomplete_pair_errors() {
        let err = GodarkConfigBuilder::new()
            .api_key_id("id")
            .build()
            .unwrap_err();
        assert!(matches!(err, GodarkError::Config(ref msg) if msg.contains("together")));
    }

    #[test]
    fn test_builder_default_symbols() {
        let cfg = GodarkConfigBuilder::new().api_key("k").build().unwrap();
        assert_eq!(cfg.symbol_map.get("BTC-USDC-PERP"), Some(&1));
        assert_eq!(cfg.symbol_map.get("ETH-USDC-PERP"), Some(&2));
        // SOL is id 5 in prod (id 3 is BNB); see new-sdks parity audit.
        assert_eq!(cfg.symbol_map.get("SOL-USDC-PERP"), Some(&5));
    }

    #[test]
    fn test_default_symbols_from_shared_json() {
        let raw: HashMap<String, u64> =
            serde_json::from_str(super::DEFAULT_SYMBOLS_JSON).expect("parse");
        assert_eq!(raw.len(), 3);
    }

    #[test]
    fn test_builder_custom_symbol() {
        let cfg = GodarkConfigBuilder::new()
            .api_key("k")
            .symbol("DOGE-USDC-PERP", 8)
            .build()
            .unwrap();
        assert_eq!(cfg.symbol_map.get("DOGE-USDC-PERP"), Some(&8));
    }

    #[test]
    fn test_builder_auto_reconnect_default() {
        let cfg = GodarkConfigBuilder::new().api_key("k").build().unwrap();
        assert!(cfg.auto_reconnect);
    }

    #[test]
    fn test_resolve_edge_base_url_explicit() {
        assert_eq!(
            resolve_edge_base_url(Some("wss://custom.example")),
            "wss://custom.example"
        );
    }

    #[test]
    fn test_resolve_edge_base_url_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_godark = std::env::var("GODARK_EDGE_URL").ok();
        let old_gdx = std::env::var("GDX_EDGE_URL").ok();
        std::env::remove_var("GODARK_EDGE_URL");
        std::env::remove_var("GDX_EDGE_URL");

        assert_eq!(resolve_edge_base_url(None), "wss://api.godark-dex.com");

        if let Some(v) = old_godark {
            std::env::set_var("GODARK_EDGE_URL", v);
        }
        if let Some(v) = old_gdx {
            std::env::set_var("GDX_EDGE_URL", v);
        }
    }

    #[test]
    fn test_ws_url_appends_ws() {
        assert_eq!(ws_url("wss://example.com"), "wss://example.com/ws/v1");
    }

    #[test]
    fn test_ws_url_no_double_ws() {
        assert_eq!(ws_url("wss://example.com/ws"), "wss://example.com/ws/v1");
    }

    #[test]
    fn test_ws_url_idempotent_v1() {
        assert_eq!(ws_url("wss://x.com/ws/v1"), "wss://x.com/ws/v1");
    }

    #[test]
    fn test_ws_url_trailing_slash_v1() {
        assert_eq!(ws_url("wss://x.com/ws/v1/"), "wss://x.com/ws/v1");
    }

    #[test]
    fn test_gomarket_url_strips_ws() {
        assert_eq!(
            gomarket_url("wss://example.com/ws"),
            "wss://example.com/ws/gomarket"
        );
    }

    #[test]
    fn test_gomarket_url_strips_ws_v1() {
        assert_eq!(gomarket_url("wss://x.com/ws/v1"), "wss://x.com/ws/gomarket");
    }

    #[test]
    fn test_gomarket_url_plain() {
        assert_eq!(
            gomarket_url("wss://example.com"),
            "wss://example.com/ws/gomarket"
        );
    }
}
