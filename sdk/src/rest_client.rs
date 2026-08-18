//! REST residual client for the GoDark DEX.
//!
//! Auth + plaintext account reads:
//!   1. `POST /api/v1/auth/token` (RFC 6749 client credentials).
//!   2. Plaintext `GET` helpers (`/me`, balance, leverage, order snapshots).
//!
//! Encrypted place/cancel/modify/update-leverage requires a WebSocket-bound
//! Noise XK session via [`crate::GodarkClient`]. REST cannot establish Noise XK
//! (legacy ECDH `session/setup` is retired).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use crate::config::{resolve_passphrase, Environment};
use crate::enums::{OrderType, Side, TimeInForce};
use crate::types::{Balance, LeverageSettings, MeProfile};

use crate::error::GodarkError;
use crate::proto_bridge;
use crate::rest_transport::RestTransport;
use crate::session::CryptoSession;
use crate::types::OrderAck;

const DEFAULT_SYMBOLS_JSON: &str = include_str!("../shared/symbols.json");

/// Translate a `ws[s]://host[:port][/...]` URL to its sibling `http[s]://`
/// URL so a single `EDGE_URL` env var can configure both the WebSocket and
/// REST clients (the localnet edge serves both protocols on the same
/// listener; matches the C++ / Python SDKs).
fn derive_http_from_ws(url: &str) -> Option<String> {
    if let Some(rest) = url.strip_prefix("wss://") {
        Some(format!("https://{rest}"))
    } else if let Some(rest) = url.strip_prefix("ws://") {
        Some(format!("http://{rest}"))
    } else if url.starts_with("http://") || url.starts_with("https://") {
        Some(url.to_string())
    } else {
        None
    }
}

/// Resolve the REST base URL by checking, in order:
///   1. The explicit `rest_base_url` builder field.
///   2. `GODARK_REST_URL` / `GDX_REST_URL`.
///   3. `GODARK_EDGE_URL` / `GDX_EDGE_URL` / `GODARK_BASE_URL` (with
///      `ws[s]://` rewritten to `http[s]://`).
///   4. The environment preset default (testnet unless overridden).
pub(crate) fn resolve_rest_base_url(explicit: Option<String>) -> String {
    resolve_rest_base_url_with_default(explicit, Environment::Testnet.rest_base_url())
}

fn resolve_rest_base_url_with_default(explicit: Option<String>, default: &str) -> String {
    if let Some(url) = explicit {
        let trimmed = url.trim_end_matches('/').to_string();
        if !trimmed.is_empty() {
            return trimmed;
        }
    }
    for key in &["GODARK_REST_URL", "GDX_REST_URL"] {
        if let Ok(v) = std::env::var(key) {
            let trimmed = v.trim().trim_end_matches('/').to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }
    for key in &["GODARK_EDGE_URL", "GDX_EDGE_URL", "GODARK_BASE_URL"] {
        if let Ok(v) = std::env::var(key) {
            if let Some(http_url) = derive_http_from_ws(v.trim()) {
                let trimmed = http_url.trim_end_matches('/').to_string();
                if !trimmed.is_empty() {
                    return trimmed;
                }
            }
        }
    }
    default.trim_end_matches('/').to_string()
}

/// Builder for [`GodarkRestClient`] — separate from `GodarkConfigBuilder` (WS) to
/// keep the WS surface unchanged and allow REST-only consumers to compose a client
/// without touching WS-specific knobs (TLS, heartbeat, etc.).
pub struct GodarkRestClientBuilder {
    api_key: Option<String>,
    api_key_id: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
    rest_base_url: Option<String>,
    environment: Environment,
    user_uuid: Option<Uuid>,
    symbol_map: HashMap<String, u64>,
    explicit_symbol_map: bool,
}

impl GodarkRestClientBuilder {
    pub fn new() -> Self {
        let symbol_map: HashMap<String, u64> =
            serde_json::from_str(DEFAULT_SYMBOLS_JSON).expect("default symbols.json");
        Self {
            api_key: None,
            api_key_id: None,
            api_secret: None,
            passphrase: None,
            rest_base_url: None,
            environment: Environment::Testnet,
            user_uuid: None,
            symbol_map,
            explicit_symbol_map: false,
        }
    }

    /// Select a named deployment. Defaults to [`Environment::Testnet`].
    /// Explicit `.rest_base_url(...)` / env vars still win over the preset.
    pub fn environment(mut self, environment: Environment) -> Self {
        self.environment = environment;
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

    pub fn passphrase(mut self, pp: impl Into<String>) -> Self {
        self.passphrase = Some(pp.into());
        self
    }

    pub fn rest_base_url(mut self, url: impl Into<String>) -> Self {
        self.rest_base_url = Some(url.into());
        self
    }

    /// Fallback user UUID when the edge auth response omits `user_uuid` (e.g. localnet).
    pub fn user_uuid(mut self, id: impl Into<String>) -> Self {
        self.user_uuid = Uuid::parse_str(&id.into()).ok();
        self
    }

    pub fn symbol(mut self, name: impl Into<String>, id: u64) -> Self {
        self.explicit_symbol_map = true;
        self.symbol_map.insert(name.into(), id);
        self
    }

    pub fn build(self) -> Result<GodarkRestClient, GodarkError> {
        let (api_key_id, api_secret, passphrase, legacy_auth_token) =
            match (self.api_key_id, self.api_secret, self.api_key) {
                (Some(id), Some(secret), None) => {
                    let pp = resolve_passphrase(self.passphrase.as_deref()).ok_or_else(|| {
                        GodarkError::Config(
                            "passphrase is required when using api_key_id and api_secret".into(),
                        )
                    })?;
                    (Some(id), Some(secret), Some(pp), None)
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
                    (None, None, None, Some(key))
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

        let base_url = resolve_rest_base_url_with_default(
            self.rest_base_url,
            self.environment.rest_base_url(),
        );

        let user_uuid = self.user_uuid.or_else(|| {
            for k in &["GODARK_USER_UUID", "GDX_USER_UUID"] {
                if let Ok(v) = std::env::var(k) {
                    if let Ok(u) = Uuid::parse_str(v.trim()) {
                        return Some(u);
                    }
                }
            }
            None
        });

        Ok(GodarkRestClient {
            http: RestTransport::new(base_url.clone()),
            rest_base_url: base_url,
            session: CryptoSession::new(),
            api_key_id,
            api_secret,
            passphrase,
            legacy_auth_token,
            symbol_map: self.symbol_map,
            explicit_symbol_map: self.explicit_symbol_map,
            bearer: None,
            user_uuid,
            wallet_address: None,
            local_coid_index: HashMap::new(),
        })
    }
}

impl Default for GodarkRestClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// REST trading orchestrator. Encrypts every order client-side; the edge only forwards.
pub struct GodarkRestClient {
    http: RestTransport,
    rest_base_url: String,
    session: CryptoSession,
    api_key_id: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
    legacy_auth_token: Option<String>,
    symbol_map: HashMap<String, u64>,
    explicit_symbol_map: bool,
    bearer: Option<String>,
    user_uuid: Option<Uuid>,
    /// Cached wallet address from `/auth/me` — avoids repeated lookups in `get_my_balance`.
    wallet_address: Option<String>,
    /// Populated after decrypting successful place ACKs; drives cancel-by-coid without sentinel bodies.
    local_coid_index: HashMap<String, String>,
}

impl GodarkRestClient {
    pub fn builder() -> GodarkRestClientBuilder {
        GodarkRestClientBuilder::new()
    }

    pub fn is_session_established(&self) -> bool {
        self.session.is_established()
    }

    pub fn user_uuid(&self) -> Option<Uuid> {
        self.user_uuid
    }

    fn resolve_symbol(&self, symbol: &str) -> Result<u64, GodarkError> {
        self.symbol_map.get(symbol).copied().ok_or_else(|| {
            GodarkError::Config(format!(
                "Unknown symbol '{symbol}'. Known: {:?}",
                self.symbol_map.keys().collect::<Vec<_>>()
            ))
        })
    }

    fn current_user_uuid(&self) -> Result<Uuid, GodarkError> {
        self.user_uuid.ok_or_else(|| {
            GodarkError::Session("user_uuid missing — set via builder or env".into())
        })
    }

    fn current_bearer(&self) -> Result<&str, GodarkError> {
        self.bearer
            .as_deref()
            .ok_or_else(|| GodarkError::Session("Not connected — call .connect() first".into()))
    }

    /// Authenticate REST (`auth/token`) and optionally refresh the edge symbol map.
    ///
    /// Does not establish Noise XK; encrypted order verbs refuse until the
    /// caller uses [`crate::GodarkClient`].
    pub async fn connect(&mut self) -> Result<(), GodarkError> {
        if !self.explicit_symbol_map {
            self.symbol_map =
                crate::instruments::load_symbol_map_from_edge(&self.rest_base_url).await;
        }
        let auth_data = if let (Some(id), Some(sec), Some(pp)) =
            (&self.api_key_id, &self.api_secret, &self.passphrase)
        {
            self.http
                .auth_token_document_body("client_credentials", id, sec, pp)
                .await?
        } else if let Some(token) = &self.legacy_auth_token {
            self.http.auth_token_legacy_token(token).await?
        } else {
            return Err(GodarkError::Config("invalid auth credentials".into()));
        };

        let bearer = auth_data
            .get("access_token")
            .or_else(|| auth_data.get("token"))
            .and_then(|v| v.as_str())
            .map(String::from)
            .ok_or_else(|| GodarkError::Authentication("auth/token missing token".into()))?;
        self.bearer = Some(bearer.clone());

        if self.user_uuid.is_none() {
            if let Some(u) = auth_data
                .get("user_uuid")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                self.user_uuid = Some(u);
            }
        }

        // Noise XK is bound to a WebSocket connection and cannot be established
        // over REST. Public REST reads remain available; encrypted trading is
        // rejected below until a caller uses `GodarkClient`.
        Ok(())
    }

    /// Revoke bearer + reset session.
    pub async fn disconnect(&mut self) -> Result<(), GodarkError> {
        if let Some(b) = self.bearer.clone() {
            let _ = self.http.revoke_token(&b).await; // best-effort
        }
        self.bearer = None;
        self.wallet_address = None;
        self.session.reset();
        self.local_coid_index.clear();
        Ok(())
    }

    /// Place is refused: encrypted REST trading requires WebSocket Noise XK
    /// ([`crate::GodarkClient`]).
    #[allow(clippy::too_many_arguments)]
    pub async fn place_order(
        &mut self,
        symbol: &str,
        side: Side,
        order_type: OrderType,
        quantity: f64,
        price: Option<f64>,
        time_in_force: TimeInForce,
        aon: bool,
        min_fill_size: Option<f64>,
        expiry_time: Option<u64>,
        client_order_id: Option<String>,
    ) -> Result<OrderAck, GodarkError> {
        let symbol_id = self.resolve_symbol(symbol)?;
        let uuid = self.current_user_uuid()?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();

        let plaintext = proto_bridge::build_place_order_proto(
            symbol_id,
            side,
            order_type,
            quantity,
            uuid.as_bytes(),
            price,
            time_in_force,
            aon,
            min_fill_size,
            expiry_time,
            &corr_id,
            timestamp_ns(),
        );

        let coid_for_register = client_order_id.clone();
        let ack = self
            .send_encrypted_order(
                "place",
                symbol_id,
                &plaintext,
                &corr_id,
                client_order_id,
                None,
                None,
            )
            .await?;

        // Phase B (Zone A): edge stays stateless and never decrypts. After we
        // decrypt the encrypted place ACK locally we must populate the edge's
        // `(client_order_id → order_id)` index so subsequent coid-based
        // resolution works (`cancel_order_by_client_id`, `?client_order_id=`).
        // Registration failures must NEVER bubble up — they don't invalidate
        // the placed order.
        if let Some(coid) = coid_for_register {
            if ack.success && !ack.order_id.is_empty() {
                self.local_coid_index
                    .insert(coid.clone(), ack.order_id.clone());
                let bearer = self.current_bearer()?.to_string();
                if let Err(err) = self
                    .http
                    .register_client_order_mapping(&bearer, &coid, &ack.order_id)
                    .await
                {
                    tracing::warn!(
                        client_order_id = %coid,
                        order_id = %ack.order_id,
                        error = %err,
                        "register_client_order_mapping failed; coid lookups may not resolve until next place"
                    );
                }
            }
        }
        Ok(ack)
    }

    pub async fn cancel_order(
        &mut self,
        order_id: &str,
        symbol: &str,
    ) -> Result<OrderAck, GodarkError> {
        let symbol_id = self.resolve_symbol(symbol)?;
        let uuid = self.current_user_uuid()?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let oid: u64 = order_id
            .parse()
            .map_err(|_| GodarkError::Config(format!("Invalid order_id: {order_id}")))?;
        let plaintext =
            proto_bridge::build_cancel_order_proto(oid, uuid.as_bytes(), symbol_id, &corr_id);
        self.send_encrypted_order(
            "cancel",
            symbol_id,
            &plaintext,
            &corr_id,
            None,
            Some(EncryptedRoute::DeletePathId(order_id.to_string())),
            None,
        )
        .await
    }

    /// Resolves `(client_order_id → order_id)` via local decrypt cache or
    /// [`RestTransport::get_order_by_client_order_id`], then cancels via path id.
    pub async fn cancel_order_by_client_id(
        &mut self,
        client_order_id: &str,
        symbol: &str,
    ) -> Result<OrderAck, GodarkError> {
        if let Some(real_id) = self.local_coid_index.get(client_order_id).cloned() {
            return self.cancel_order(&real_id, symbol).await;
        }
        let bearer = self.current_bearer()?.to_string();
        let row = self
            .http
            .get_order_by_client_order_id(&bearer, client_order_id)
            .await?;
        let oid = resolve_order_id_from_lookup(&row).ok_or_else(|| GodarkError::Order {
            message: "unknown client_order_id".into(),
            error_code: None,
        })?;
        self.local_coid_index
            .insert(client_order_id.to_string(), oid.clone());
        self.cancel_order(&oid, symbol).await
    }

    pub async fn modify_order(
        &mut self,
        order_id: &str,
        symbol: &str,
        new_price: Option<f64>,
        new_quantity: Option<f64>,
    ) -> Result<OrderAck, GodarkError> {
        let symbol_id = self.resolve_symbol(symbol)?;
        let uuid = self.current_user_uuid()?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let oid: u64 = order_id
            .parse()
            .map_err(|_| GodarkError::Config(format!("Invalid order_id: {order_id}")))?;
        let plaintext = proto_bridge::build_modify_order_proto(
            oid,
            uuid.as_bytes(),
            symbol_id,
            new_price,
            new_quantity,
            &corr_id,
        );
        self.send_encrypted_order(
            "modify",
            symbol_id,
            &plaintext,
            &corr_id,
            None,
            Some(EncryptedRoute::PatchPathId(order_id.to_string())),
            None,
        )
        .await
    }

    pub async fn get_order(&self, order_id: &str) -> Result<Value, GodarkError> {
        let bearer = self.current_bearer()?;
        self.http.get_order(bearer, order_id).await
    }

    pub async fn get_order_by_client_id(&self, coid: &str) -> Result<Value, GodarkError> {
        let bearer = self.current_bearer()?;
        self.http.get_order_by_client_order_id(bearer, coid).await
    }

    /// Fetch cached per-symbol leverage settings via `GET /api/v1/leverage`.
    pub async fn get_leverage(&self) -> Result<LeverageSettings, GodarkError> {
        let bearer = self.current_bearer()?.to_string();
        let data = self.http.get_leverage(&bearer).await?;
        serde_json::from_value(data)
            .map_err(|e| GodarkError::Connection(format!("parse leverage settings: {e}")))
    }

    /// Update leverage for `symbol` via encrypted `POST /api/v1/leverage`.
    pub async fn update_leverage(
        &mut self,
        symbol: &str,
        leverage: u32,
    ) -> Result<OrderAck, GodarkError> {
        let symbol_id = self.resolve_symbol(symbol)?;
        let uuid = self.current_user_uuid()?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let lev = leverage.max(1);
        let plaintext =
            proto_bridge::build_update_leverage_proto(uuid.as_bytes(), symbol_id, lev, &corr_id);
        self.send_encrypted_order(
            "update_leverage",
            symbol_id,
            &plaintext,
            &corr_id,
            None,
            Some(EncryptedRoute::PostLeverage),
            Some(lev),
        )
        .await
    }

    /// Fetch the authenticated user's profile from `GET /api/v1/auth/me`.
    /// Caches the wallet address for subsequent [`Self::get_my_balance`] calls.
    pub async fn get_me(&mut self) -> Result<MeProfile, GodarkError> {
        let bearer = self.current_bearer()?.to_string();
        let data = self.http.get_auth_me(&bearer).await?;
        let me: MeProfile = serde_json::from_value(data)
            .map_err(|e| GodarkError::Connection(format!("parse /auth/me: {e}")))?;
        if !me.wallet_address.is_empty() {
            self.wallet_address = Some(me.wallet_address.clone());
        }
        Ok(me)
    }

    /// Fetch the on-chain balance snapshot for `owner` (Solana base58 wallet pubkey)
    /// via `GET /api/v1/shielded-pool/balances/{owner}`.
    pub async fn get_balance(&self, owner: &str) -> Result<Balance, GodarkError> {
        if owner.trim().is_empty() {
            return Err(GodarkError::Config(
                "get_balance: owner pubkey is required (use get_my_balance to auto-resolve)".into(),
            ));
        }
        let bearer = self.current_bearer()?.to_string();
        let data = self.http.get_shielded_pool_balances(&bearer, owner).await?;
        let bal: Balance = serde_json::from_value(data)
            .map_err(|e| GodarkError::Connection(format!("parse balance: {e}")))?;
        Ok(bal)
    }

    /// Convenience: resolves the user's wallet address via [`Self::get_me`] (cached
    /// after first call), then fetches the shielded-pool balance snapshot.
    pub async fn get_my_balance(&mut self) -> Result<Balance, GodarkError> {
        let owner = match self.wallet_address.clone() {
            Some(addr) => addr,
            None => {
                let me = self.get_me().await?;
                if me.wallet_address.is_empty() {
                    return Err(GodarkError::Session(
                        "get_my_balance: /auth/me returned empty wallet_address".into(),
                    ));
                }
                me.wallet_address
            }
        };
        self.get_balance(&owner).await
    }

    /// `GET /api/v1/market-data/funding-rates` — public; no `connect` required.
    pub async fn get_funding_rates(&self) -> Result<Value, GodarkError> {
        self.http.get_funding_rates().await
    }

    /// `GET /api/v1/market-data/open-interest` — public; no `connect` required.
    pub async fn get_open_interest(&self) -> Result<Value, GodarkError> {
        self.http.get_open_interest().await
    }

    /// `GET /api/v1/market-data/volume` — public; no `connect` required.
    pub async fn get_volume(&self) -> Result<Value, GodarkError> {
        self.http.get_volume().await
    }

    /// Poll [`Self::get_order`] until status is one of `FILLED`, `CANCELLED`, `REJECTED`.
    pub async fn await_terminal_status(
        &self,
        order_id: &str,
        timeout: Duration,
    ) -> Result<Value, GodarkError> {
        let deadline = Instant::now() + timeout;
        let terminal = ["FILLED", "CANCELLED", "REJECTED"];
        loop {
            let row = self.get_order(order_id).await?;
            if let Some(s) = row.get("status").and_then(|v| v.as_str()) {
                if terminal.contains(&s) {
                    return Ok(row);
                }
            }
            if Instant::now() >= deadline {
                return Err(GodarkError::Timeout(format!(
                    "order {order_id} did not reach terminal status within {timeout:?}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn send_encrypted_order(
        &mut self,
        _request_type: &str,
        _symbol_id: u64,
        _plaintext: &[u8],
        _correlation_id: &[u8],
        _place_client_order_id: Option<String>,
        _route: Option<EncryptedRoute>,
        _header_leverage: Option<u32>,
    ) -> Result<OrderAck, GodarkError> {
        // Auth-only REST: Noise XK is WebSocket-bound (matches other SDKs).
        Err(GodarkError::Session(
            "encrypted REST trading is unsupported with Noise XK; use GodarkClient over WebSocket instead"
                .into(),
        ))
    }
}

#[allow(dead_code)] // retained so place/cancel/modify call sites stay typed; encrypt path refuses
enum EncryptedRoute {
    PostLeverage,
    DeletePathId(String),
    PatchPathId(String),
}

fn resolve_order_id_from_lookup(v: &Value) -> Option<String> {
    v.get("order_id")
        .and_then(|x| {
            x.as_str()
                .map(String::from)
                .or_else(|| x.as_u64().map(|n| n.to_string()))
        })
        .filter(|s| !s.is_empty())
}

// Plaintext ACK parser retained for tests; encrypted REST verbs now refuse
// before any ACK is produced (Noise XK is WebSocket-only).
#[cfg(test)]
fn parse_order_ack(v: &Value) -> Result<OrderAck, GodarkError> {
    use crate::order_error_code::make_order_error_from_json;
    let success = v.get("success").and_then(|x| x.as_bool()).unwrap_or(false);
    if !success {
        let reason = v.get("error").and_then(|x| x.as_str()).map(String::from);
        let code = v
            .get("error_code")
            .and_then(|x| x.as_str())
            .map(String::from);
        return Err(make_order_error_from_json(reason, code));
    }
    let order_id = v
        .get("order_id")
        .and_then(|x| {
            x.as_str()
                .map(String::from)
                .or_else(|| x.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_default();
    let sequence = v
        .get("sequence")
        .and_then(|x| {
            x.as_str()
                .map(String::from)
                .or_else(|| x.as_u64().map(|n| n.to_string()))
        })
        .unwrap_or_default();
    Ok(OrderAck {
        order_id,
        success: true,
        sequence,
        error_code: None,
        error: None,
    })
}

fn timestamp_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn clear_env(keys: &[&str]) -> Vec<(String, Option<String>)> {
        keys.iter()
            .map(|k| {
                let prev = std::env::var(k).ok();
                std::env::remove_var(k);
                (k.to_string(), prev)
            })
            .collect()
    }

    fn restore_env(saved: Vec<(String, Option<String>)>) {
        for (k, v) in saved {
            if let Some(val) = v {
                std::env::set_var(&k, val);
            } else {
                std::env::remove_var(&k);
            }
        }
    }

    #[test]
    fn builder_rejects_no_credentials() {
        let res = GodarkRestClient::builder().build();
        assert!(matches!(res, Err(GodarkError::Config(_))));
    }

    #[test]
    fn builder_accepts_legacy_api_key() {
        let c = GodarkRestClient::builder()
            .api_key("k")
            .rest_base_url("http://localhost:4000")
            .build()
            .unwrap();
        assert!(!c.is_session_established());
    }

    #[test]
    fn builder_devnet_environment_sets_rest_base_url() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = clear_env(&[
            "GODARK_REST_URL",
            "GDX_REST_URL",
            "GODARK_EDGE_URL",
            "GDX_EDGE_URL",
            "GODARK_BASE_URL",
        ]);
        let c = GodarkRestClient::builder()
            .environment(Environment::Devnet)
            .api_key("k")
            .build()
            .unwrap();
        assert_eq!(c.rest_base_url, "http://18.143.165.149:13300");
        restore_env(saved);
    }

    #[test]
    fn builder_accepts_id_secret_with_passphrase() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = clear_env(&["GODARK_USER_UUID", "GDX_USER_UUID"]);
        let c = GodarkRestClient::builder()
            .api_key_id("id")
            .api_secret("sec")
            .passphrase("pp")
            .build()
            .unwrap();
        assert!(c.user_uuid().is_none());
        restore_env(saved);
    }

    #[test]
    fn builder_id_secret_requires_passphrase() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = clear_env(&["GODARK_PASSPHRASE", "GDX_PASSPHRASE"]);
        let res = GodarkRestClient::builder()
            .api_key_id("id")
            .api_secret("sec")
            .build();
        assert!(matches!(
            res,
            Err(GodarkError::Config(ref msg)) if msg.contains("passphrase")
        ));
        restore_env(saved);
    }

    #[test]
    fn builder_legacy_rejects_passphrase() {
        let res = GodarkRestClient::builder()
            .api_key("k")
            .passphrase("pp")
            .build();
        assert!(matches!(
            res,
            Err(GodarkError::Config(ref msg)) if msg.contains("passphrase")
        ));
    }

    #[test]
    fn builder_resolves_user_uuid_explicit() {
        let id = "00000000-0000-4000-8000-000000000042";
        let c = GodarkRestClient::builder()
            .api_key("k")
            .user_uuid(id)
            .build()
            .unwrap();
        assert_eq!(c.user_uuid().unwrap().to_string(), id);
    }

    #[test]
    fn parse_ack_failure_surfaces_error_code() {
        let v = json!({
            "success": false,
            "error": "rate limit",
            "error_code": "RL"
        });
        let err = parse_order_ack(&v).unwrap_err();
        match err {
            GodarkError::Order {
                message,
                error_code,
            } => {
                assert_eq!(message, "rate limit");
                assert_eq!(error_code.as_deref(), Some("RL"));
            }
            other => panic!("expected Order error: {other:?}"),
        }
    }

    #[test]
    fn parse_ack_success_returns_ids_as_strings() {
        let v = json!({
            "success": true,
            "order_id": "9999",
            "sequence": "7"
        });
        let ack = parse_order_ack(&v).unwrap();
        assert_eq!(ack.order_id, "9999");
        assert_eq!(ack.sequence, "7");
    }

    #[test]
    fn get_leverage_requires_connect() {
        let client = GodarkRestClient::builder()
            .api_key("k")
            .rest_base_url("http://localhost:4000")
            .build()
            .unwrap();
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(client.get_leverage())
            .unwrap_err();
        assert!(matches!(err, GodarkError::Session(_)));
    }

    #[test]
    fn update_leverage_requires_connect() {
        let mut client = GodarkRestClient::builder()
            .api_key("k")
            .rest_base_url("http://localhost:4000")
            .build()
            .unwrap();
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(client.update_leverage("BTC-USDC-PERP", 5))
            .unwrap_err();
        assert!(matches!(err, GodarkError::Session(_)));
    }

    #[test]
    fn update_leverage_rejects_unknown_symbol() {
        let mut client = GodarkRestClient::builder()
            .api_key("k")
            .user_uuid("00000000-0000-4000-8000-000000000001")
            .rest_base_url("http://localhost:4000")
            .build()
            .unwrap();
        let err = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(client.update_leverage("NO-SUCH-SYMBOL", 5))
            .unwrap_err();
        assert!(matches!(err, GodarkError::Config(_)));
    }
}
