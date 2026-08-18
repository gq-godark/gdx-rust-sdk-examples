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
/// to a localnet edge (`ws://127.0.0.1:4000`) via [`Environment::Localnet`],
/// the `base_url` builder, `GODARK_EDGE_URL`, or `GDX_EDGE_URL`. Either
/// `<host>` or `<host>/ws/v1` resolve to the same endpoint.
const DEFAULT_EDGE_BASE_URL: &str = "wss://api.godark-dex.com";
const DEVNET_EDGE_BASE_URL: &str = "ws://18.143.165.149:13300";
const LOCALNET_EDGE_BASE_URL: &str = "ws://127.0.0.1:4000";

/// Sequencer Noise XK static public key for public testnet (64 hex).
/// This is a public pin, not a user secret.
const TESTNET_NOISE_STATIC_PUBLIC_KEY_HEX: &str =
    "a9fdd7f26c0de36d82811e9fe1df2509960cd5b25eef037355e209b9222bea7d";

/// Sequencer Noise XK static public key for public devnet (64 hex).
/// Distinct from the testnet pin. This is a public pin, not a user secret.
const DEVNET_NOISE_STATIC_PUBLIC_KEY_HEX: &str =
    "a6807e2f6cd04b54cc19be2fd4faea2a1239f1e2896912d91222678ab54cdd45";

/// Canonical default perps (shared across SDKs); see `shared/symbols.json`.
const DEFAULT_SYMBOLS_JSON: &str = include_str!("../shared/symbols.json");

/// Named deployment target. Selects the default edge URL and, when known,
/// a baked-in sequencer Noise XK public key pin.
///
/// Explicit `.base_url(...)` / `.noise_static_public_key_hex(...)` and the
/// corresponding environment variables still win over these presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum Environment {
    /// Public testnet (`wss://api.godark-dex.com`) with the published Noise pin.
    #[default]
    Testnet,
    /// Public devnet (`ws://18.143.165.149:13300`) with its own Noise pin.
    Devnet,
    /// Local edge (`ws://127.0.0.1:4000`). No baked-in Noise pin — set via
    /// `.noise_static_public_key_hex(...)` or `GODARK_NOISE_STATIC_PUBLIC_KEY`.
    Localnet,
}

impl Environment {
    /// Default edge base URL for this environment (host only).
    #[must_use]
    pub const fn edge_base_url(self) -> &'static str {
        match self {
            Self::Testnet => DEFAULT_EDGE_BASE_URL,
            Self::Devnet => DEVNET_EDGE_BASE_URL,
            Self::Localnet => LOCALNET_EDGE_BASE_URL,
        }
    }

    /// Default REST base URL for this environment.
    #[must_use]
    pub const fn rest_base_url(self) -> &'static str {
        match self {
            Self::Testnet => "https://api.godark-dex.com",
            Self::Devnet => "http://18.143.165.149:13300",
            Self::Localnet => "http://127.0.0.1:4000",
        }
    }

    /// Baked-in sequencer Noise XK static public key (64 hex chars), when known.
    #[must_use]
    pub const fn noise_static_public_key_hex(self) -> Option<&'static str> {
        match self {
            Self::Testnet => Some(TESTNET_NOISE_STATIC_PUBLIC_KEY_HEX),
            Self::Devnet => Some(DEVNET_NOISE_STATIC_PUBLIC_KEY_HEX),
            Self::Localnet => None,
        }
    }
}

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
///
/// Construct via [`GodarkConfigBuilder`]. New fields are `pub(crate)` so
/// external struct-literal construction is not required to keep pace.
#[derive(Debug, Clone)]
#[non_exhaustive]
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
    /// Pinned sequencer Noise XK static X25519 public key, encoded as hex.
    /// Resolved from the builder or `GODARK_NOISE_STATIC_PUBLIC_KEY`,
    /// `GDX_NOISE_STATIC_PUBLIC_KEY`, or `GDX_NOISE_STATIC_PUBKEY`.
    pub noise_static_public_key_hex: Option<String>,
    /// How long [`Confirmation::Book`](crate::types::Confirmation::Book)
    /// waits for an OPEN/reject/fill/cancel update after the fast ack.
    /// Always positive; use [`Confirmation::Ack`](crate::types::Confirmation::Ack)
    /// to skip waiting. Set via
    /// [`GodarkConfigBuilder::place_order_terminal_timeout`].
    pub(crate) place_order_terminal_timeout: Duration,
    /// When true, caller supplied custom symbols via builder; skip edge fetch.
    pub(crate) explicit_symbol_map: bool,
}

impl GodarkConfig {
    /// Timeout used when `place_order` confirmation is [`Confirmation::Book`](crate::types::Confirmation::Book).
    #[must_use]
    pub fn place_order_terminal_timeout(&self) -> Duration {
        self.place_order_terminal_timeout
    }
}

/// Builder for GodarkClient configuration.
pub struct GodarkConfigBuilder {
    api_key: Option<String>,
    api_key_id: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
    base_url: Option<String>,
    environment: Environment,
    auto_reconnect: bool,
    symbol_map: HashMap<String, u64>,
    transport: TransportConfig,
    user_uuid: Option<Uuid>,
    noise_static_public_key_hex: Option<String>,
    place_order_terminal_timeout: Option<Duration>,
    explicit_symbol_map: bool,
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
            environment: Environment::Testnet,
            auto_reconnect: true,
            symbol_map: symbols,
            transport: TransportConfig::default(),
            user_uuid: None,
            noise_static_public_key_hex: None,
            place_order_terminal_timeout: None,
            explicit_symbol_map: false,
        }
    }

    /// Select a named deployment. Defaults to [`Environment::Testnet`], which
    /// supplies the public testnet edge URL and Noise XK pin when those are
    /// not set explicitly or via environment variables.
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = environment;
        self
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

    /// How long book confirmation waits after the fast ack (must be > 0).
    /// Defaults to the transport `command_timeout`.
    pub fn place_order_terminal_timeout(mut self, timeout: Duration) -> Self {
        self.place_order_terminal_timeout = Some(timeout);
        self
    }

    /// Set the user UUID explicitly. Required when the edge auth response does
    /// not return `user_uuid` (e.g. localnet / static-key auth). Falls back to
    /// `GODARK_USER_UUID` / `GDX_USER_UUID` environment variables at build time.
    pub fn user_uuid(mut self, uuid: impl Into<String>) -> Self {
        self.user_uuid = Uuid::parse_str(&uuid.into()).ok();
        self
    }

    /// Pin the sequencer Noise XK static X25519 public key (64 hex characters).
    pub fn noise_static_public_key_hex(mut self, key: impl Into<String>) -> Self {
        self.noise_static_public_key_hex = Some(key.into());
        self
    }

    pub fn symbol(mut self, name: impl Into<String>, id: u64) -> Self {
        self.explicit_symbol_map = true;
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

        let base_url = resolve_edge_base_url_with_default(
            self.base_url.as_deref(),
            self.environment.edge_base_url(),
        );

        let user_uuid = self.user_uuid.or_else(resolve_user_uuid_env);
        let noise_static_public_key_hex = self
            .noise_static_public_key_hex
            .or_else(resolve_noise_static_public_key_env)
            .or_else(|| {
                self.environment
                    .noise_static_public_key_hex()
                    .map(str::to_string)
            });

        let place_order_terminal_timeout = self
            .place_order_terminal_timeout
            .unwrap_or(self.transport.command_timeout);
        if place_order_terminal_timeout.is_zero() {
            return Err(GodarkError::Config(
                "place_order_terminal_timeout must be greater than zero".into(),
            ));
        }
        Ok(GodarkConfig {
            auth_token,
            base_url,
            auto_reconnect: self.auto_reconnect,
            symbol_map: self.symbol_map,
            transport: self.transport,
            user_uuid,
            noise_static_public_key_hex,
            place_order_terminal_timeout,
            explicit_symbol_map: self.explicit_symbol_map,
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

fn resolve_noise_static_public_key_env() -> Option<String> {
    for key in &[
        "GODARK_NOISE_STATIC_PUBLIC_KEY",
        "GDX_NOISE_STATIC_PUBLIC_KEY",
        "GDX_NOISE_STATIC_PUBKEY",
    ] {
        if let Ok(value) = env::var(key) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }
    None
}

/// Resolve edge base URL: explicit arg > env vars > testnet default.
pub fn resolve_edge_base_url(explicit: Option<&str>) -> String {
    resolve_edge_base_url_with_default(explicit, DEFAULT_EDGE_BASE_URL)
}

/// Resolve edge base URL: explicit arg > env vars > `default`.
fn resolve_edge_base_url_with_default(explicit: Option<&str>, default: &str) -> String {
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
    default.to_string()
}

/// Resolve a base URL to the canonical edge WebSocket endpoint `<base>/ws/v1`.
///
/// Rewrite `http(s)://` to `ws(s)://`.
fn rewrite_http_scheme(url: &str) -> String {
    if let Some(rest) = url.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = url.strip_prefix("https://") {
        format!("wss://{rest}")
    } else {
        url.to_string()
    }
}

/// True when the resolved URL is the public-docs `/ws/v1` path (ignores trailing slash / query).
pub fn is_docs_wire_url(url: &str) -> bool {
    let cut = url.split(['?', '#']).next().unwrap_or(url);
    cut.trim_end_matches('/').ends_with("/ws/v1")
}

/// Trailing slashes are stripped first, then:
/// - `http(s)://` is rewritten to `ws(s)://`;
/// - if the input already ends with `/ws/v1` it is returned unchanged;
/// - if the input ends with the legacy `/ws` suffix it is upgraded to `/ws/v1`;
/// - otherwise `/ws/v1` is appended.
pub fn ws_url(base_url: &str) -> String {
    let url = rewrite_http_scheme(base_url.trim_end_matches('/'));
    if url.ends_with("/ws/v1") {
        url
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
/// `<host>/ws/v1` resolve to `<host>/ws/gomarket`. Also converts
/// `http(s)://` to `ws(s)://`.
pub fn gomarket_url(base_url: &str) -> String {
    let mut url = base_url.trim_end_matches('/').to_string();
    if let Some(stripped) = url.strip_suffix("/ws/v1") {
        url = stripped.to_string();
    } else if let Some(stripped) = url.strip_suffix("/ws") {
        url = stripped.to_string();
    }
    if let Some(rest) = url.strip_prefix("http://") {
        url = format!("ws://{rest}");
    } else if let Some(rest) = url.strip_prefix("https://") {
        url = format!("wss://{rest}");
    }
    format!("{url}/ws/gomarket")
}

fn env_first(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(v) = env::var(key) {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                return Some(trimmed);
            }
        }
    }
    None
}

fn env_truthy(keys: &[&str]) -> bool {
    for key in keys {
        if let Ok(v) = env::var(key) {
            let raw = v.trim().to_lowercase();
            if matches!(raw.as_str(), "1" | "true" | "yes" | "on") {
                return true;
            }
        }
    }
    false
}

/// Resolve the market-data WebSocket URL.
///
/// Hosted edges default to `/ws/v1`. Override with `GODARK_MARKET_DATA_WS_URL`,
/// or set `GODARK_MARKET_DATA_USE_GOMARKET=1` for `/ws/gomarket`.
pub fn resolve_market_data_ws_url(base_url: &str) -> String {
    if let Some(override_url) = env_first(&["GODARK_MARKET_DATA_WS_URL", "GDX_MARKET_DATA_WS_URL"])
    {
        return override_url;
    }
    if env_truthy(&[
        "GODARK_MARKET_DATA_USE_GOMARKET",
        "GDX_MARKET_DATA_USE_GOMARKET",
    ]) {
        return gomarket_url(base_url.trim());
    }
    ws_url(base_url.trim())
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::{
        gomarket_url, resolve_edge_base_url, resolve_market_data_ws_url, resolve_passphrase,
        ws_url, Environment, GodarkConfigBuilder, GodarkError, DEVNET_NOISE_STATIC_PUBLIC_KEY_HEX,
        TESTNET_NOISE_STATIC_PUBLIC_KEY_HEX,
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
        let _guard = ENV_LOCK.lock().unwrap();
        let old_g = std::env::var("GODARK_PASSPHRASE").ok();
        let old_x = std::env::var("GDX_PASSPHRASE").ok();
        std::env::remove_var("GODARK_PASSPHRASE");
        std::env::remove_var("GDX_PASSPHRASE");

        let err = GodarkConfigBuilder::new()
            .api_key_id("id")
            .api_secret("secret")
            .build()
            .unwrap_err();
        assert!(matches!(err, GodarkError::Config(ref msg) if msg.contains("passphrase")));

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
    fn test_builder_place_order_terminal_timeout_default() {
        let cfg = GodarkConfigBuilder::new().api_key("k").build().unwrap();
        assert_eq!(
            cfg.place_order_terminal_timeout(),
            cfg.transport.command_timeout
        );
    }

    #[test]
    fn test_builder_place_order_terminal_timeout_custom() {
        use std::time::Duration;
        let cfg = GodarkConfigBuilder::new()
            .api_key("k")
            .place_order_terminal_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        assert_eq!(cfg.place_order_terminal_timeout(), Duration::from_secs(5));
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
    fn test_builder_default_environment_testnet_noise_pin() {
        let _guard = ENV_LOCK.lock().unwrap();
        for key in [
            "GDX_NOISE_STATIC_PUBLIC_KEY",
            "GDX_NOISE_STATIC_PUBKEY",
            "GODARK_NOISE_STATIC_PUBLIC_KEY",
            "GODARK_EDGE_URL",
            "GDX_EDGE_URL",
        ] {
            std::env::remove_var(key);
        }

        let cfg = GodarkConfigBuilder::new()
            .api_key_id("id")
            .api_secret("secret")
            .passphrase("pp")
            .build()
            .unwrap();
        assert_eq!(cfg.base_url, "wss://api.godark-dex.com");
        assert_eq!(
            cfg.noise_static_public_key_hex.as_deref(),
            Some(TESTNET_NOISE_STATIC_PUBLIC_KEY_HEX)
        );
    }

    #[test]
    fn test_builder_devnet_uses_distinct_noise_pin() {
        let _guard = ENV_LOCK.lock().unwrap();
        for key in [
            "GDX_NOISE_STATIC_PUBLIC_KEY",
            "GDX_NOISE_STATIC_PUBKEY",
            "GODARK_NOISE_STATIC_PUBLIC_KEY",
            "GODARK_EDGE_URL",
            "GDX_EDGE_URL",
        ] {
            std::env::remove_var(key);
        }

        let cfg = GodarkConfigBuilder::new()
            .environment(Environment::Devnet)
            .api_key_id("id")
            .api_secret("secret")
            .passphrase("pp")
            .build()
            .unwrap();
        assert_eq!(cfg.base_url, "ws://18.143.165.149:13300");
        assert_eq!(
            cfg.noise_static_public_key_hex.as_deref(),
            Some(DEVNET_NOISE_STATIC_PUBLIC_KEY_HEX)
        );
        assert_ne!(
            DEVNET_NOISE_STATIC_PUBLIC_KEY_HEX,
            TESTNET_NOISE_STATIC_PUBLIC_KEY_HEX
        );
    }

    #[test]
    fn test_builder_localnet_has_no_baked_noise_pin() {
        let _guard = ENV_LOCK.lock().unwrap();
        for key in [
            "GDX_NOISE_STATIC_PUBLIC_KEY",
            "GDX_NOISE_STATIC_PUBKEY",
            "GODARK_NOISE_STATIC_PUBLIC_KEY",
            "GODARK_EDGE_URL",
            "GDX_EDGE_URL",
        ] {
            std::env::remove_var(key);
        }

        let cfg = GodarkConfigBuilder::new()
            .environment(Environment::Localnet)
            .api_key_id("id")
            .api_secret("secret")
            .passphrase("pp")
            .build()
            .unwrap();
        assert_eq!(cfg.base_url, "ws://127.0.0.1:4000");
        assert_eq!(cfg.noise_static_public_key_hex, None);
    }

    #[test]
    fn test_builder_explicit_noise_overrides_environment() {
        let cfg = GodarkConfigBuilder::new()
            .environment(Environment::Testnet)
            .noise_static_public_key_hex("11".repeat(32))
            .api_key_id("id")
            .api_secret("secret")
            .passphrase("pp")
            .build()
            .unwrap();
        assert_eq!(
            cfg.noise_static_public_key_hex.as_deref(),
            Some("1111111111111111111111111111111111111111111111111111111111111111")
        );
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
    fn test_ws_url_rewrites_https() {
        assert_eq!(ws_url("https://api.example"), "wss://api.example/ws/v1");
        assert_eq!(ws_url("http://localhost:4000"), "ws://localhost:4000/ws/v1");
    }

    #[test]
    fn test_is_docs_wire_url_ignores_slash_and_query() {
        assert!(super::is_docs_wire_url("wss://x.com/ws/v1/"));
        assert!(super::is_docs_wire_url("wss://x.com/ws/v1?x=1"));
        assert!(!super::is_docs_wire_url("wss://x.com/ws/gomarket"));
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

    #[test]
    fn test_resolve_market_data_ws_url_defaults_to_ws_v1() {
        let _guard = ENV_LOCK.lock().unwrap();
        for key in &[
            "GODARK_MARKET_DATA_WS_URL",
            "GDX_MARKET_DATA_WS_URL",
            "GODARK_MARKET_DATA_USE_GOMARKET",
            "GDX_MARKET_DATA_USE_GOMARKET",
        ] {
            std::env::remove_var(key);
        }
        assert_eq!(
            resolve_market_data_ws_url("wss://api.example"),
            "wss://api.example/ws/v1"
        );
    }

    #[test]
    fn test_resolve_market_data_ws_url_gomarket_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        for key in &[
            "GODARK_MARKET_DATA_WS_URL",
            "GDX_MARKET_DATA_WS_URL",
            "GDX_MARKET_DATA_USE_GOMARKET",
        ] {
            std::env::remove_var(key);
        }
        std::env::set_var("GODARK_MARKET_DATA_USE_GOMARKET", "1");
        assert_eq!(
            resolve_market_data_ws_url("wss://api.example/ws/v1"),
            "wss://api.example/ws/gomarket"
        );
        std::env::remove_var("GODARK_MARKET_DATA_USE_GOMARKET");
    }
}
