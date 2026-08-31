//! REST trading client for the GoDark DEX.
//!
//! 1. `POST /api/v1/auth/token` (RFC 6749 client credentials).
//! 2. Encrypted orders: one-shot HPKE per request (`encapped_key` + `request_id`,
//!    `OrderHeader.conn_id = 0`) matching gdx-edge / gdx-sequencer.
//! 3. Plaintext `GET /api/v1/orders/{order_id}` for terminal-status polling.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::config::{resolve_passphrase, Environment};
use crate::enums::{OrderType, Side, TimeInForce};
use crate::error::GodarkError;
use crate::hpke::{self, parse_pinned_static_public_key};
use crate::order_error_code::{make_order_error_from_code, make_order_error_from_json};
use crate::proto_bridge;
use crate::rest_transport::RestTransport;
use crate::session::CryptoSession;
use crate::types::{
    AccountMarginUpdate, LeverageSettings, MeProfile, OpenOrdersSnapshot, OrderAck,
    PositionsSnapshot,
};

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

fn json_u128(value: &Value, key: &str) -> Option<u128> {
    value.get(key).and_then(|field| {
        field
            .as_u64()
            .map(u128::from)
            .or_else(|| field.as_str().and_then(|raw| raw.parse().ok()))
    })
}

fn json_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(|field| {
        field
            .as_u64()
            .or_else(|| field.as_str().and_then(|raw| raw.parse().ok()))
    })
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
    hpke_static_public_key_hex: Option<String>,
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
            hpke_static_public_key_hex: None,
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

    pub fn hpke_static_public_key_hex(mut self, key: impl Into<String>) -> Self {
        self.hpke_static_public_key_hex = Some(key.into());
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

        let hpke_static_public_key_hex = self.hpke_static_public_key_hex.or_else(|| {
            for key in &[
                "GDX_HPKE_STATIC_PUBLIC_KEY",
                "GDX_HPKE_STATIC_PUBKEY",
                "GODARK_HPKE_STATIC_PUBLIC_KEY",
                "VITE_GDX_HPKE_STATIC_PUBKEY",
            ] {
                if let Ok(value) = std::env::var(key) {
                    let value = value.trim();
                    if !value.is_empty() {
                        return Some(value.to_string());
                    }
                }
            }
            None
        });

        Ok(GodarkRestClient {
            http: RestTransport::new(base_url.clone()),
            rest_base_url: base_url,
            next_request_id: AtomicU64::new(1),
            hpke_static_public_key_hex,
            api_key_id,
            api_secret,
            passphrase,
            legacy_auth_token,
            symbol_map: self.symbol_map,
            explicit_symbol_map: self.explicit_symbol_map,
            bearer: None,
            user_uuid,
            token_scope: None,
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
    next_request_id: AtomicU64,
    hpke_static_public_key_hex: Option<String>,
    api_key_id: Option<String>,
    api_secret: Option<String>,
    passphrase: Option<String>,
    legacy_auth_token: Option<String>,
    symbol_map: HashMap<String, u64>,
    explicit_symbol_map: bool,
    bearer: Option<String>,
    user_uuid: Option<Uuid>,
    token_scope: Option<String>,
    /// Populated after decrypting successful place ACKs; drives cancel-by-coid without sentinel bodies.
    local_coid_index: HashMap<String, String>,
}

impl GodarkRestClient {
    pub fn builder() -> GodarkRestClientBuilder {
        GodarkRestClientBuilder::new()
    }

    pub fn user_uuid(&self) -> Option<Uuid> {
        self.user_uuid
    }

    pub fn token_scope(&self) -> Option<&str> {
        self.token_scope.as_deref()
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

    /// `auth/token` → optional edge instruments fetch. HPKE is one-shot per order.
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
        self.token_scope = auth_data
            .get("scope")
            .and_then(|v| v.as_str())
            .map(String::from);

        if self.user_uuid.is_none() {
            if let Some(u) = auth_data
                .get("user_uuid")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
            {
                self.user_uuid = Some(u);
            } else if let Some(u) = crate::access_token::user_uuid_from_access_token_jwt(&bearer)
            {
                self.user_uuid = Some(u);
            }
        }

        // Encrypted REST uses one-shot HPKE per request (no persistent session).
        Ok(())
    }

    /// Revoke bearer + reset session.
    pub async fn disconnect(&mut self) -> Result<(), GodarkError> {
        if let Some(b) = self.bearer.clone() {
            let _ = self.http.revoke_token(&b).await; // best-effort
        }
        self.bearer = None;
        self.token_scope = None;
        self.local_coid_index.clear();
        Ok(())
    }

    /// Place an encrypted order. `client_order_id` is sent additively (cleartext) only
    /// for the edge's `client_order_id → order_id` lookup index — it is also embedded
    /// inside the encrypted `OrderHeader` AAD so the sequencer can dedup.
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
                EncryptedCall::new("place", symbol_id, &plaintext, &corr_id)
                    .client_order_id(client_order_id),
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
            EncryptedCall::new("cancel", symbol_id, &plaintext, &corr_id)
                .route(EncryptedRoute::DeletePathId(order_id.to_string())),
        )
        .await
    }

    /// Resolves `(client_order_id → order_id)` via local decrypt cache or
    /// `GET /api/v1/orders?client_order_id=...`, then cancels via path id.
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
            EncryptedCall::new("modify", symbol_id, &plaintext, &corr_id)
                .route(EncryptedRoute::PatchPathId(order_id.to_string())),
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
            EncryptedCall::new("update_leverage", symbol_id, &plaintext, &corr_id)
                .route(EncryptedRoute::PostPath("/api/v1/leverage"))
                .leverage(lev),
        )
        .await
    }

    /// Live open orders via encrypted `POST /api/v1/openOrders`.
    pub async fn get_open_orders(&mut self) -> Result<OpenOrdersSnapshot, GodarkError> {
        match self
            .snapshot_rpc(
                "get_open_orders",
                proto_bridge::build_get_open_orders_proto,
                "/api/v1/openOrders",
            )
            .await?
        {
            proto_bridge::NodeResponseKind::OpenOrdersSnapshot(s) => Ok(s),
            other => Err(snapshot_rpc_error(other, "open_orders_snapshot")),
        }
    }

    /// Live positions via encrypted `POST /api/v1/positions`.
    pub async fn get_positions(&mut self) -> Result<PositionsSnapshot, GodarkError> {
        match self
            .snapshot_rpc(
                "get_positions",
                proto_bridge::build_get_positions_proto,
                "/api/v1/positions",
            )
            .await?
        {
            proto_bridge::NodeResponseKind::PositionsSnapshot(s) => Ok(s),
            other => Err(snapshot_rpc_error(other, "positions_snapshot")),
        }
    }

    /// Live account margin via encrypted `POST /api/v1/account`.
    pub async fn get_account(&mut self) -> Result<AccountMarginUpdate, GodarkError> {
        match self
            .snapshot_rpc(
                "get_account",
                proto_bridge::build_get_account_proto,
                "/api/v1/account",
            )
            .await?
        {
            proto_bridge::NodeResponseKind::AccountMarginUpdate(s) => Ok(s),
            other => Err(snapshot_rpc_error(other, "account_margin_update")),
        }
    }

    /// Bulk cancel-replace via encrypted `POST /api/v1/orders/massQuote`.
    pub async fn mass_quote(
        &mut self,
        symbol: &str,
        legs: &[crate::types::MassQuoteLegInput],
        post_only: Option<bool>,
    ) -> Result<crate::types::MassQuoteAck, GodarkError> {
        if legs.is_empty() || legs.len() > 20 {
            return Err(GodarkError::InvalidInput(
                "mass quote accepts 1..=20 legs".into(),
            ));
        }
        let symbol_id = self.resolve_symbol(symbol)?;
        let uuid = self.current_user_uuid()?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let plaintext = proto_bridge::build_mass_quote_proto(
            symbol_id,
            uuid.as_bytes(),
            legs,
            &corr_id,
            post_only,
        );
        let (sealed, raw) = self
            .send_encrypted(
                EncryptedCall::new("mass_quote", symbol_id, &plaintext, &corr_id)
                    .route(EncryptedRoute::PostPath("/api/v1/orders/massQuote")),
            )
            .await?;
        match self.decrypt_rest_node_response(&sealed, &raw)? {
            proto_bridge::NodeResponseKind::MassQuoteAck(ack) => Ok(ack),
            other => Err(snapshot_rpc_error(other, "mass_quote_ack")),
        }
    }

    /// Cancel up to 20 resting orders via encrypted `POST /api/v1/orders`.
    pub async fn batch_cancel(
        &mut self,
        symbol: &str,
        order_ids: &[u64],
    ) -> Result<crate::types::BatchCancelAck, GodarkError> {
        if order_ids.is_empty() || order_ids.len() > 20 {
            return Err(GodarkError::InvalidInput(
                "batch cancel accepts 1..=20 order ids".into(),
            ));
        }
        let symbol_id = self.resolve_symbol(symbol)?;
        let uuid = self.current_user_uuid()?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let plaintext = proto_bridge::build_batch_cancel_proto(
            symbol_id,
            uuid.as_bytes(),
            order_ids,
            &corr_id,
        );
        let (sealed, raw) = self
            .send_encrypted(
                EncryptedCall::new("batch_cancel", symbol_id, &plaintext, &corr_id),
            )
            .await?;
        match self.decrypt_rest_node_response(&sealed, &raw)? {
            proto_bridge::NodeResponseKind::BatchCancelAck(ack) => Ok(ack),
            other => Err(snapshot_rpc_error(other, "batch_cancel_ack")),
        }
    }

    /// Post-only amend up to 20 resting orders via encrypted `POST /api/v1/orders`.
    pub async fn batch_modify(
        &mut self,
        symbol: &str,
        legs: &[crate::types::BatchModifyLegInput],
    ) -> Result<crate::types::BatchModifyAck, GodarkError> {
        if legs.is_empty() || legs.len() > 20 {
            return Err(GodarkError::InvalidInput(
                "batch modify accepts 1..=20 legs".into(),
            ));
        }
        let symbol_id = self.resolve_symbol(symbol)?;
        let uuid = self.current_user_uuid()?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let plaintext = proto_bridge::build_batch_modify_proto(
            symbol_id,
            uuid.as_bytes(),
            legs,
            &corr_id,
        );
        let (sealed, raw) = self
            .send_encrypted(
                EncryptedCall::new("batch_modify", symbol_id, &plaintext, &corr_id),
            )
            .await?;
        match self.decrypt_rest_node_response(&sealed, &raw)? {
            proto_bridge::NodeResponseKind::BatchModifyAck(ack) => Ok(ack),
            other => Err(snapshot_rpc_error(other, "batch_modify_ack")),
        }
    }

    /// Public funding-rate snapshot (`GET /api/v1/market-data/funding-rates`).
    pub async fn get_funding_rates(&self) -> Result<Value, GodarkError> {
        self.http.get_funding_rates().await
    }

    /// Public open-interest snapshot (`GET /api/v1/market-data/open-interest`).
    pub async fn get_open_interest(&self) -> Result<Value, GodarkError> {
        self.http.get_open_interest().await
    }

    /// Public 24h volume snapshot (`GET /api/v1/market-data/volume`).
    pub async fn get_volume(&self) -> Result<Value, GodarkError> {
        self.http.get_volume().await
    }

    async fn snapshot_rpc(
        &mut self,
        request_type: &str,
        build: fn(&[u8], &[u8]) -> Vec<u8>,
        path: &'static str,
    ) -> Result<proto_bridge::NodeResponseKind, GodarkError> {
        let uuid = self.current_user_uuid()?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let plaintext = build(uuid.as_bytes(), &corr_id);
        // Sequencer ingress admission keys off header `symbol_id` gates. Symbol 0
        // has no gate and is shed as SEQUENCER_BUSY. Match the web client: pin BTC.
        let header_symbol_id = self
            .symbol_map
            .get("BTC-USDC-PERP")
            .copied()
            .or_else(|| self.symbol_map.values().copied().next())
            .unwrap_or(1);
        let (sealed, raw) = self
            .send_encrypted(
                EncryptedCall::new(request_type, header_symbol_id, &plaintext, &corr_id)
                    .route(EncryptedRoute::PostPath(path)),
            )
            .await?;
        self.decrypt_rest_node_response(&sealed, &raw)
    }

    /// Fetch browser session profile from `GET /api/v1/auth/me`.
    ///
    /// Requires a **session** JWT (Dynamic login). API-key tokens from
    /// `auth/token` are rejected; use [`Self::user_uuid()`] after [`Self::connect`]
    /// instead (parsed from the access JWT `sub` claim).
    pub async fn get_me(&mut self) -> Result<MeProfile, GodarkError> {
        let bearer = self.current_bearer()?.to_string();
        let data = self.http.get_auth_me(&bearer).await?;
        serde_json::from_value(data)
            .map_err(|e| GodarkError::Connection(format!("parse /auth/me: {e}")))
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

    async fn send_encrypted_order(
        &mut self,
        call: EncryptedCall<'_>,
    ) -> Result<OrderAck, GodarkError> {
        let (sealed, raw) = self.send_encrypted(call).await?;
        if raw
            .get("encrypted")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
            || raw.get("encrypted_body").is_some()
        {
            return self.decrypt_rest_ack(&sealed, &raw);
        }
        parse_order_ack(&raw)
    }

    async fn send_encrypted(
        &mut self,
        call: EncryptedCall<'_>,
    ) -> Result<(hpke::SealedSession, Value), GodarkError> {
        let EncryptedCall {
            request_type,
            symbol_id,
            plaintext,
            correlation_id,
            route,
            client_order_id,
            header_leverage,
        } = call;
        let bearer = self.current_bearer()?.to_string();
        let uuid = self.current_user_uuid()?;
        let pin_hex = self.hpke_static_public_key_hex.as_deref().ok_or_else(|| {
            GodarkError::Config(
                "HPKE static public key unset; pass .hpke_static_public_key_hex() or set GDX_HPKE_STATIC_PUBLIC_KEY".into(),
            )
        })?;
        let recipient = parse_pinned_static_public_key(pin_hex)?;
        let request_id = self.next_request_id.fetch_add(1, Ordering::Relaxed);
        let (encapped, sealed) = CryptoSession::setup_rest(&recipient, uuid, request_id)?;
        let nonce = 0u64;
        let body_length = CryptoSession::body_length_for_plaintext(plaintext.len())?;

        let aad = proto_bridge::build_order_header_aad(
            uuid.as_bytes(),
            symbol_id,
            request_type,
            nonce,
            body_length,
            correlation_id,
            0,
        );

        let ciphertext = sealed
            .seal_c2s(&hpke::nonce_from_u64(nonce), &aad, plaintext)
            .map_err(|e| GodarkError::Encryption(format!("encrypt: {e}")))?;
        let mut header = json!({
            "symbol_id": symbol_id,
            "request_type": request_type,
            "nonce": nonce,
            "body_length": body_length,
        });
        if correlation_id.len() == 16 {
            let mut arr = [0u8; 16];
            arr.copy_from_slice(correlation_id);
            let value = u128::from_be_bytes(arr);
            if value != 0 {
                header["correlation_id"] = Value::String(format!("{value:032x}"));
            }
        }
        if let Some(lev) = header_leverage {
            header["leverage"] = json!(lev);
        }
        let mut body = json!({
            "header": header,
            "encrypted_body": BASE64.encode(&ciphertext),
            "encapped_key": BASE64.encode(&encapped),
            "request_id": request_id,
        });
        if let Some(coid) = client_order_id {
            body["client_order_id"] = Value::String(coid);
        }

        let raw = match route {
            EncryptedRoute::PostOrders => self.http.post_orders_encrypted(&bearer, body).await?,
            EncryptedRoute::PostPath(path) => self.http.post_encrypted(path, &bearer, body).await?,
            EncryptedRoute::DeletePathId(id) => {
                self.http
                    .delete_orders_encrypted(&bearer, &id, body)
                    .await?
            }
            EncryptedRoute::PatchPathId(id) => {
                self.http.patch_orders_encrypted(&bearer, &id, body).await?
            }
        };
        Ok((sealed, raw))
    }

    /// Decrypt the encrypted ACK returned by REST encrypted-order endpoints
    /// (Mradul's Zone A: edge never decrypts; SDK decrypts with session key).
    fn decrypt_rest_ack(
        &self,
        sealed: &hpke::SealedSession,
        raw: &Value,
    ) -> Result<OrderAck, GodarkError> {
        match self.decrypt_rest_node_response(sealed, raw)? {
            proto_bridge::NodeResponseKind::Ack {
                order_id,
                success,
                sequence,
                error_code,
                reject_text,
                ..
            } => {
                if !success {
                    return Err(make_order_error_from_code(
                        error_code,
                        reject_text.as_deref(),
                    ));
                }
                Ok(OrderAck {
                    order_id: order_id.to_string(),
                    success: true,
                    sequence: sequence.to_string(),
                    error_code: None,
                    error: None,
                })
            }
            _ => Err(GodarkError::Order {
                message: "Expected ack response".to_string(),
                error_code: None,
            }),
        }
    }

    fn decrypt_rest_node_response(
        &self,
        sealed: &hpke::SealedSession,
        raw: &Value,
    ) -> Result<proto_bridge::NodeResponseKind, GodarkError> {
        let ct_b64 = raw
            .get("encrypted_body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ct = BASE64
            .decode(ct_b64)
            .map_err(|e| GodarkError::Encryption(format!("invalid encrypted_body b64: {e}")))?;
        let nonce = raw.get("nonce").and_then(|v| v.as_u64()).unwrap_or(0);
        let message_type = raw
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or("ack");
        let fencing_epoch = raw
            .get("fencing_epoch")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let uuid = self.current_user_uuid()?;
        let aad = proto_bridge::build_response_header_aad(
            uuid.as_bytes(),
            message_type,
            ct.len() as u32,
            nonce,
            fencing_epoch,
            &json_u128(raw, "correlation_id")
                .filter(|value| *value != 0)
                .map(|value| value.to_be_bytes().to_vec())
                .unwrap_or_default(),
            json_u64(raw, "session_seq").unwrap_or_default(),
            0,
        );
        let plaintext = sealed
            .open_s2c(&hpke::nonce_from_u64(nonce), &aad, &ct)
            .map_err(|e| GodarkError::Encryption(format!("Failed to decrypt REST reply: {e}")))?;
        proto_bridge::parse_node_response(&plaintext)
    }
}

enum EncryptedRoute {
    PostOrders,
    PostPath(&'static str),
    DeletePathId(String),
    PatchPathId(String),
}

struct EncryptedCall<'a> {
    request_type: &'a str,
    symbol_id: u64,
    plaintext: &'a [u8],
    correlation_id: &'a [u8],
    route: EncryptedRoute,
    client_order_id: Option<String>,
    header_leverage: Option<u32>,
}

impl<'a> EncryptedCall<'a> {
    fn new(
        request_type: &'a str,
        symbol_id: u64,
        plaintext: &'a [u8],
        correlation_id: &'a [u8],
    ) -> Self {
        Self {
            request_type,
            symbol_id,
            plaintext,
            correlation_id,
            route: EncryptedRoute::PostOrders,
            client_order_id: None,
            header_leverage: None,
        }
    }

    fn route(mut self, route: EncryptedRoute) -> Self {
        self.route = route;
        self
    }

    fn client_order_id(mut self, client_order_id: Option<String>) -> Self {
        self.client_order_id = client_order_id;
        self
    }

    fn leverage(mut self, leverage: u32) -> Self {
        self.header_leverage = Some(leverage);
        self
    }
}

fn snapshot_rpc_error(kind: proto_bridge::NodeResponseKind, expected: &str) -> GodarkError {
    match kind {
        proto_bridge::NodeResponseKind::Ack {
            success: false,
            error_code,
            reject_text,
            ..
        } => make_order_error_from_code(error_code, reject_text.as_deref()),
        other => GodarkError::Order {
            message: format!("expected {expected}, got {other:?}"),
            error_code: None,
        },
    }
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

fn parse_order_ack(v: &Value) -> Result<OrderAck, GodarkError> {
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
        assert_eq!(c.user_uuid(), None);
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
