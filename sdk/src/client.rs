//! `GodarkClient` — WebSocket trading client.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc, Mutex,
};
use std::time::Duration;

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::config::{self, GodarkConfig, GodarkConfigBuilder};
use crate::enums::{CancelReason, OrderStatus, OrderType, OrderUpdateType, Side, TimeInForce};
use crate::error::GodarkError;
use crate::generated::edge::v1 as edge;
use crate::hpke::parse_pinned_static_public_key;
use crate::proto_bridge::{self, EdgeMessage};
use crate::session::CryptoSession;
use crate::transport::{EdgeTransport, TransportEvent};
use crate::types::{
    AccountMarginUpdate, BalanceUpdate, Confirmation, FundingRateUpdate, OrderAck, OrderUpdate,
    PositionsSnapshot, ReconnectEvent, SystemHealthUpdate,
};
use crate::wire;

const HPKE_SETUP_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_BACKOFF: Duration = Duration::from_secs(15);
struct PlaceOutcomeWaiter {
    token: u64,
    order_id: Option<String>,
    sender: Option<oneshot::Sender<Result<OrderUpdate, GodarkError>>>,
}

#[derive(Default)]
struct PlaceOutcomeState {
    next_token: u64,
    waiters: Vec<PlaceOutcomeWaiter>,
    recent: VecDeque<OrderUpdate>,
}

pub struct GodarkClient {
    config: GodarkConfig,
    transport: Arc<AsyncMutex<EdgeTransport>>,
    session: Arc<Mutex<CryptoSession>>,
    user_uuid: Arc<Mutex<Option<Uuid>>>,
    connected: Arc<AtomicBool>,
    desired_channels: Arc<Mutex<HashSet<String>>>,
    order_tx: mpsc::Sender<OrderUpdate>,
    order_rx: Option<mpsc::Receiver<OrderUpdate>>,
    positions_snapshot_tx: mpsc::Sender<PositionsSnapshot>,
    positions_snapshot_rx: Option<mpsc::Receiver<PositionsSnapshot>>,
    system_health_tx: mpsc::Sender<SystemHealthUpdate>,
    system_health_rx: Option<mpsc::Receiver<SystemHealthUpdate>>,
    balance_tx: mpsc::Sender<BalanceUpdate>,
    balance_rx: Option<mpsc::Receiver<BalanceUpdate>>,
    funding_rate_tx: mpsc::Sender<FundingRateUpdate>,
    funding_rate_rx: Option<mpsc::Receiver<FundingRateUpdate>>,
    account_margin_tx: mpsc::Sender<AccountMarginUpdate>,
    account_margin_rx: Option<mpsc::Receiver<AccountMarginUpdate>>,
    error_tx: mpsc::Sender<GodarkError>,
    error_rx: Option<mpsc::Receiver<GodarkError>>,
    event_handle: Option<JoinHandle<()>>,
    reconnect_attempts: Arc<AtomicU32>,
    intentional_close: Arc<AtomicBool>,
    reconnect_tx: mpsc::Sender<ReconnectEvent>,
    reconnect_rx: Option<mpsc::Receiver<ReconnectEvent>>,
    place_outcomes: Arc<Mutex<PlaceOutcomeState>>,
    /// Correlation-keyed waiters for encrypted command acks (web parity).
    encrypted_ack_waiters: Arc<Mutex<HashMap<u128, oneshot::Sender<Value>>>>,
    /// Serializes HPKE encrypt + WS send so ciphertext nonces hit the wire
    /// in order under concurrent place/cancel/modify.
    encrypted_send_lock: Arc<AsyncMutex<()>>,
}

impl GodarkClient {
    pub fn builder() -> GodarkConfigBuilder {
        GodarkConfigBuilder::new()
    }

    pub fn new(config: GodarkConfig) -> Self {
        let ws_url = config::ws_url(&config.base_url);
        let (order_tx, order_rx) = mpsc::channel(256);
        let (positions_snapshot_tx, positions_snapshot_rx) = mpsc::channel(64);
        let (system_health_tx, system_health_rx) = mpsc::channel(64);
        let (balance_tx, balance_rx) = mpsc::channel(64);
        let (funding_rate_tx, funding_rate_rx) = mpsc::channel(64);
        let (account_margin_tx, account_margin_rx) = mpsc::channel(64);
        let (error_tx, error_rx) = mpsc::channel(256);
        let (reconnect_tx, reconnect_rx) = mpsc::channel(256);
        Self {
            transport: Arc::new(AsyncMutex::new(EdgeTransport::new(
                &ws_url,
                config.transport.clone(),
            ))),
            session: Arc::new(Mutex::new(CryptoSession::new())),
            user_uuid: Arc::new(Mutex::new(None)),
            connected: Arc::new(AtomicBool::new(false)),
            desired_channels: Arc::new(Mutex::new(HashSet::new())),
            order_tx,
            order_rx: Some(order_rx),
            positions_snapshot_tx,
            positions_snapshot_rx: Some(positions_snapshot_rx),
            system_health_tx,
            system_health_rx: Some(system_health_rx),
            balance_tx,
            balance_rx: Some(balance_rx),
            funding_rate_tx,
            funding_rate_rx: Some(funding_rate_rx),
            account_margin_tx,
            account_margin_rx: Some(account_margin_rx),
            error_tx,
            error_rx: Some(error_rx),
            event_handle: None,
            reconnect_attempts: Arc::new(AtomicU32::new(0)),
            intentional_close: Arc::new(AtomicBool::new(false)),
            reconnect_tx,
            reconnect_rx: Some(reconnect_rx),
            place_outcomes: Arc::new(Mutex::new(PlaceOutcomeState::default())),
            encrypted_ack_waiters: Arc::new(Mutex::new(HashMap::new())),
            encrypted_send_lock: Arc::new(AsyncMutex::new(())),
            config,
        }
    }

    pub fn user_uuid(&self) -> Option<Uuid> {
        self.user_uuid.lock().ok().and_then(|guard| *guard)
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    pub fn take_order_receiver(&mut self) -> Option<mpsc::Receiver<OrderUpdate>> {
        self.order_rx.take()
    }

    /// Receive full per-user [`PositionsSnapshot`] batches (initial / periodic /
    /// event-triggered) from the sequencer.
    pub fn take_positions_snapshot_receiver(
        &mut self,
    ) -> Option<mpsc::Receiver<PositionsSnapshot>> {
        self.positions_snapshot_rx.take()
    }

    /// Receive sequencer / MPC node health pulses.
    pub fn take_system_health_receiver(&mut self) -> Option<mpsc::Receiver<SystemHealthUpdate>> {
        self.system_health_rx.take()
    }

    /// Receive sequencer trading-collateral updates.
    pub fn take_balance_receiver(&mut self) -> Option<mpsc::Receiver<BalanceUpdate>> {
        self.balance_rx.take()
    }

    /// Receive per-symbol funding rate ticks.
    pub fn take_funding_rate_receiver(&mut self) -> Option<mpsc::Receiver<FundingRateUpdate>> {
        self.funding_rate_rx.take()
    }

    /// Receive account-level margin summaries.
    pub fn take_account_margin_receiver(&mut self) -> Option<mpsc::Receiver<AccountMarginUpdate>> {
        self.account_margin_rx.take()
    }

    /// Receive non-fatal background errors (rekey failures, push decrypt/parse failures).
    pub fn take_error_receiver(&mut self) -> Option<mpsc::Receiver<GodarkError>> {
        self.error_rx.take()
    }

    /// Receive reconnect lifecycle events (disconnect, backoff, success, failed attempt).
    pub fn take_reconnect_receiver(&mut self) -> Option<mpsc::Receiver<ReconnectEvent>> {
        self.reconnect_rx.take()
    }

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------

    pub async fn connect(&mut self) -> Result<(), GodarkError> {
        self.intentional_close.store(false, Ordering::SeqCst);
        if !self.config.explicit_symbol_map {
            let rest =
                crate::rest_client::resolve_rest_base_url(Some(self.config.base_url.clone()));
            self.config.symbol_map = crate::instruments::load_symbol_map_from_edge(&rest).await;
        }
        if let Some(h) = self.event_handle.take() {
            h.abort();
        }
        let rx = establish_transport_connection(
            &self.config,
            &self.transport,
            &self.session,
            &self.user_uuid,
        )
        .await?;
        self.connected.store(true, Ordering::SeqCst);
        self.reconnect_attempts.store(0, Ordering::SeqCst);

        self.start_event_loop(rx);

        tracing::info!("GodarkClient connected and authenticated");
        Ok(())
    }

    pub async fn disconnect(&mut self) {
        self.intentional_close.store(true, Ordering::SeqCst);
        self.connected.store(false, Ordering::SeqCst);
        fail_place_outcome_waiters(
            &self.place_outcomes,
            "disconnected while waiting for order confirmation",
        );
        fail_encrypted_ack_waiters(
            &self.encrypted_ack_waiters,
            "disconnected while waiting for encrypted command ack",
        );
        if let Some(h) = self.event_handle.take() {
            h.abort();
        }
        self.transport.lock().await.disconnect().await;
        if let Ok(mut session) = self.session.lock() {
            session.reset();
        }
        if let Ok(mut guard) = self.user_uuid.lock() {
            *guard = None;
        }
    }

    pub async fn logout(&mut self) -> Result<(), GodarkError> {
        self.intentional_close.store(true, Ordering::SeqCst);
        let result = async {
            if self.connected.load(Ordering::SeqCst) {
                let payload = serde_json::json!({
                    "id": Uuid::new_v4().to_string(),
                    "op": "logout",
                    "args": serde_json::json!({})
                });
                self.transport.lock().await.send_command(&payload).await?;
            }
            Ok(())
        }
        .await;
        self.disconnect().await;
        result
    }

    // ------------------------------------------------------------------
    // Trading
    // ------------------------------------------------------------------

    /// Place an order and wait for book confirmation (`OPEN` / reject / fill / cancel).
    ///
    /// Equivalent to [`Self::place_order_with_confirmation`] with
    /// [`Confirmation::Book`]. Use that method with [`Confirmation::Ack`] when
    /// you only want the sequencer fast-ack.
    #[allow(clippy::too_many_arguments)]
    pub async fn place_order(
        &self,
        symbol: &str,
        side: Side,
        order_type: OrderType,
        quantity: f64,
        price: Option<f64>,
        time_in_force: TimeInForce,
        aon: bool,
        min_fill_size: Option<f64>,
        expiry_time: Option<u64>,
    ) -> Result<OrderAck, GodarkError> {
        self.place_order_with_confirmation(
            symbol,
            side,
            order_type,
            quantity,
            price,
            time_in_force,
            aon,
            min_fill_size,
            expiry_time,
            Confirmation::Book,
        )
        .await
    }

    /// Place an order with an explicit confirmation boundary.
    ///
    /// * [`Confirmation::Book`] (safe default) — after a successful ack, wait for
    ///   a matching terminal order update and map `REJECTED` to
    ///   [`GodarkError::Order`] with code + `reject_text`/`msg`.
    /// * [`Confirmation::Ack`] — return as soon as the sequencer acknowledges;
    ///   the caller must consume order updates for later rejects/fills.
    #[allow(clippy::too_many_arguments)]
    pub async fn place_order_with_confirmation(
        &self,
        symbol: &str,
        side: Side,
        order_type: OrderType,
        quantity: f64,
        price: Option<f64>,
        time_in_force: TimeInForce,
        aon: bool,
        min_fill_size: Option<f64>,
        expiry_time: Option<u64>,
        confirmation: Confirmation,
    ) -> Result<OrderAck, GodarkError> {
        self.ensure_ready()?;
        let symbol_id = self.resolve_symbol(symbol)?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let uuid = self.current_user_uuid()?;

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

        // Register before send so a terminal push that races the ack is not lost.
        let outcome = if confirmation == Confirmation::Book {
            Some(self.register_place_outcome_waiter()?)
        } else {
            None
        };
        let ack = match self
            .send_encrypted_order("place", symbol_id, &plaintext, &corr_id)
            .await
        {
            Ok(ack) => ack,
            Err(err) => {
                if let Some((token, _)) = outcome {
                    self.cancel_place_outcome_waiter(token);
                }
                return Err(err);
            }
        };
        let Some((token, receiver)) = outcome else {
            return Ok(ack);
        };
        // Timeout starts after ack (order_id is known).
        let update = self
            .await_place_outcome(&ack.order_id, token, receiver)
            .await?;
        if update.update_type == OrderUpdateType::Rejected || update.status == OrderStatus::Rejected
        {
            let detail = update.msg;
            return Err(match update.reject_reason {
                Some(code) if code.parse::<u32>().is_ok() => {
                    let parsed = code.parse::<u32>().ok();
                    crate::order_error_code::make_order_error_from_code(parsed, detail.as_deref())
                }
                code => crate::order_error_code::make_order_error_from_json(detail, code),
            });
        }
        Ok(ack)
    }

    pub async fn cancel_order(
        &self,
        order_id: &str,
        symbol: &str,
    ) -> Result<OrderAck, GodarkError> {
        self.ensure_ready()?;
        let symbol_id = self.resolve_symbol(symbol)?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let uuid = self.current_user_uuid()?;

        let oid: u64 = order_id
            .parse()
            .map_err(|_| GodarkError::Config(format!("Invalid order_id: {order_id}")))?;

        let plaintext =
            proto_bridge::build_cancel_order_proto(oid, uuid.as_bytes(), symbol_id, &corr_id);

        self.send_encrypted_order("cancel", symbol_id, &plaintext, &corr_id)
            .await
    }

    pub async fn modify_order(
        &self,
        order_id: &str,
        symbol: &str,
        new_price: Option<f64>,
        new_quantity: Option<f64>,
    ) -> Result<OrderAck, GodarkError> {
        self.ensure_ready()?;
        let symbol_id = self.resolve_symbol(symbol)?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let uuid = self.current_user_uuid()?;

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

        self.send_encrypted_order("modify", symbol_id, &plaintext, &corr_id)
            .await
    }

    // ------------------------------------------------------------------
    // Mass quote / batch operations
    // ------------------------------------------------------------------

    /// Maximum legs / ids per mass-quote, batch-cancel, or batch-modify request.
    /// The node fans batches out at ~constant MPC cost only up to this bound.
    const MAX_BATCH_LEGS: usize = 20;

    /// Reject empty or oversized batches client-side before hitting the wire.
    fn validate_batch_len(op: &str, len: usize) -> Result<(), GodarkError> {
        if len == 0 {
            return Err(GodarkError::InvalidInput(format!(
                "{op} requires at least one leg"
            )));
        }
        if len > Self::MAX_BATCH_LEGS {
            return Err(GodarkError::InvalidInput(format!(
                "{op} accepts at most {} legs, got {len}",
                Self::MAX_BATCH_LEGS
            )));
        }
        Ok(())
    }

    /// Bulk cancel-replace (market-maker mass quote) on one symbol.
    ///
    /// Up to 20 legs per batch, fused into one MPC round. `post_only` selects
    /// the batch matching mode: `None` keeps the node default (post-only), where
    /// a leg that would cross is rejected as `failed`; `Some(false)` enables the
    /// relaxed path, where a crossing leg takes liquidity up to its limit and
    /// rests the remainder (per-leg taker fills are surfaced as `fill_count`).
    /// Returns one result per leg.
    pub async fn mass_quote(
        &self,
        symbol: &str,
        legs: &[crate::types::MassQuoteLegInput],
        post_only: Option<bool>,
    ) -> Result<crate::types::MassQuoteAck, GodarkError> {
        Self::validate_batch_len("mass quote", legs.len())?;
        self.ensure_ready()?;
        let symbol_id = self.resolve_symbol(symbol)?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let uuid = self.current_user_uuid()?;

        let plaintext = proto_bridge::build_mass_quote_proto(
            symbol_id,
            uuid.as_bytes(),
            legs,
            &corr_id,
            post_only,
        );
        let response = self
            .send_encrypted_command("mass_quote", symbol_id, &plaintext, &corr_id)
            .await?;
        self.parse_mass_quote_response(&response)
    }

    /// Cancel multiple resting orders on one symbol in a single fanned-out
    /// request (up to 20 ids). Cancels are pure index removals (zero online MPC
    /// rounds). An id that is not resting is reported `cancelled=false`
    /// (error_code 2003) and never aborts the rest of the batch.
    pub async fn batch_cancel(
        &self,
        symbol: &str,
        order_ids: &[u64],
    ) -> Result<crate::types::BatchCancelAck, GodarkError> {
        Self::validate_batch_len("batch cancel", order_ids.len())?;
        self.ensure_ready()?;
        let symbol_id = self.resolve_symbol(symbol)?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let uuid = self.current_user_uuid()?;

        let plaintext =
            proto_bridge::build_batch_cancel_proto(symbol_id, uuid.as_bytes(), order_ids, &corr_id);
        let response = self
            .send_encrypted_command("batch_cancel", symbol_id, &plaintext, &corr_id)
            .await?;
        self.parse_batch_cancel_response(&response)
    }

    /// Amend multiple resting orders on one symbol in a single fanned-out
    /// post-only request (up to 20 legs). Each leg sets `new_price` and/or
    /// `new_quantity` (at least one). A leg whose amended order would cross is
    /// rejected (`modified=false`, error_code 2018) rather than taking
    /// liquidity; a missing order id is reported `modified=false`
    /// (error_code 2003). Neither aborts the rest of the batch.
    pub async fn batch_modify(
        &self,
        symbol: &str,
        legs: &[crate::types::BatchModifyLegInput],
    ) -> Result<crate::types::BatchModifyAck, GodarkError> {
        Self::validate_batch_len("batch modify", legs.len())?;
        if let Some(i) = legs
            .iter()
            .position(|l| l.new_price.is_none() && l.new_quantity.is_none())
        {
            return Err(GodarkError::InvalidInput(format!(
                "batch modify leg {i} must set new_price and/or new_quantity"
            )));
        }
        self.ensure_ready()?;
        let symbol_id = self.resolve_symbol(symbol)?;
        let corr_id = Uuid::new_v4().into_bytes().to_vec();
        let uuid = self.current_user_uuid()?;

        let plaintext =
            proto_bridge::build_batch_modify_proto(symbol_id, uuid.as_bytes(), legs, &corr_id);
        let response = self
            .send_encrypted_command("batch_modify", symbol_id, &plaintext, &corr_id)
            .await?;
        self.parse_batch_modify_response(&response)
    }

    // ------------------------------------------------------------------
    // Subscriptions
    // ------------------------------------------------------------------

    pub async fn subscribe(&mut self, channels: &[&str]) -> Result<(), GodarkError> {
        self.ensure_ready()?;
        let ch_list: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
        {
            let mut desired = self
                .desired_channels
                .lock()
                .map_err(|_| GodarkError::Connection("Desired channel mutex poisoned".into()))?;
            for c in &ch_list {
                desired.insert(c.clone());
            }
        }
        self.transport
            .lock()
            .await
            .send_subscribe(&ch_list, "subscribe")
            .await
    }

    pub async fn unsubscribe(&mut self, channels: &[&str]) -> Result<(), GodarkError> {
        let ch_list: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
        {
            let mut desired = self
                .desired_channels
                .lock()
                .map_err(|_| GodarkError::Connection("Desired channel mutex poisoned".into()))?;
            for c in &ch_list {
                desired.remove(c);
            }
        }
        let transport = self.transport.lock().await;
        if transport.is_connected() {
            transport.send_subscribe(&ch_list, "unsubscribe").await?;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // Internals: encrypted order pipeline
    // ------------------------------------------------------------------

    async fn send_encrypted_order(
        &self,
        request_type: &str,
        symbol_id: u64,
        plaintext: &[u8],
        correlation_id: &[u8],
    ) -> Result<OrderAck, GodarkError> {
        let response = self
            .send_encrypted_command(request_type, symbol_id, plaintext, correlation_id)
            .await?;
        self.parse_order_response(&response)
    }

    /// Encrypts `plaintext`, sends it over the wire with the appropriate op for
    /// `request_type`, and returns the raw response `Value`. Shared by the
    /// single-order pipeline and the mass-quote / batch pipelines.
    async fn send_encrypted_command(
        &self,
        request_type: &str,
        symbol_id: u64,
        plaintext: &[u8],
        correlation_id: &[u8],
    ) -> Result<Value, GodarkError> {
        let body_length = CryptoSession::body_length_for_plaintext(plaintext.len())?;
        let uuid = self.current_user_uuid()?;
        let corr_u128 = if correlation_id.len() == 16 {
            let arr: [u8; 16] = correlation_id.try_into().unwrap();
            let v = u128::from_be_bytes(arr);
            if v == 0 {
                None
            } else {
                Some(v)
            }
        } else {
            None
        };
        let corr_key = corr_u128.ok_or_else(|| {
            GodarkError::Config("encrypted command requires non-zero correlation_id".into())
        })?;
        let (tx, rx) = oneshot::channel();

        // Encrypt + register waiter + send under one lock so concurrent callers
        // cannot interleave HPKE send nonces on the wire.
        {
            let _send_guard = self.encrypted_send_lock.lock().await;
            let (actual_nonce, ciphertext, conn_id) = {
                let mut session = self
                    .session
                    .lock()
                    .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?;
                let nonce_counter = session.next_nonce();
                let conn_id = session
                    .conn_id()
                    .ok_or_else(|| GodarkError::Session("HPKE session not established".into()))?;

                let aad = proto_bridge::build_order_header_aad(
                    uuid.as_bytes(),
                    symbol_id,
                    request_type,
                    nonce_counter,
                    body_length,
                    correlation_id,
                    conn_id,
                );

                let (nonce, ct) = session.encrypt_order(&aad, plaintext).map_err(|e| {
                    GodarkError::Encryption(format!("Failed to encrypt order: {e}"))
                })?;
                (nonce, ct, conn_id)
            };

            let header = edge::OrderHeader {
                user_uuid: uuid.as_bytes().to_vec(),
                symbol_id,
                request_type: crate::enums::request_type_to_proto(request_type),
                nonce: actual_nonce,
                body_length,
                correlation_id: correlation_id.to_vec(),
                conn_id,
            };
            let frame =
                wire::encode_encrypted_order(wire::encrypted_order_request(header, ciphertext));

            {
                let mut waiters = self.encrypted_ack_waiters.lock().map_err(|_| {
                    GodarkError::Session("encrypted ack waiter mutex poisoned".into())
                })?;
                waiters.insert(corr_key, tx);
            }

            if let Err(err) = self.transport.lock().await.send_binary(frame).await {
                let _ = self
                    .encrypted_ack_waiters
                    .lock()
                    .ok()
                    .and_then(|mut w| w.remove(&corr_key));
                return Err(err);
            }
        }

        let cmd_to = self.config.transport.command_timeout;
        match tokio::time::timeout(cmd_to, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(GodarkError::Connection(
                "encrypted command ack cancelled".into(),
            )),
            Err(_) => {
                let _ = self
                    .encrypted_ack_waiters
                    .lock()
                    .ok()
                    .and_then(|mut w| w.remove(&corr_key));
                Err(GodarkError::Timeout(format!(
                    "Command timed out after {cmd_to:?}"
                )))
            }
        }
    }

    fn parse_order_response(&self, msg: &Value) -> Result<OrderAck, GodarkError> {
        let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "error" => {
                let message = msg
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown error");
                Err(GodarkError::Order {
                    message: message.to_string(),
                    error_code: None,
                })
            }
            "ack" => {
                if msg.get("success").and_then(|v| v.as_bool()) != Some(true) {
                    let reason = msg.get("error").and_then(|v| v.as_str()).map(String::from);
                    let code = msg
                        .get("error_code")
                        .and_then(|v| v.as_str())
                        .map(String::from);
                    return Err(crate::order_error_code::make_order_error_from_json(
                        reason, code,
                    ));
                }
                Ok(OrderAck {
                    order_id: msg
                        .get("order_id")
                        .and_then(|v| {
                            v.as_str()
                                .map(String::from)
                                .or_else(|| v.as_u64().map(|n| n.to_string()))
                        })
                        .unwrap_or_default(),
                    success: true,
                    sequence: msg
                        .get("sequence")
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    error_code: None,
                    error: None,
                })
            }
            "encrypted_push" => self.decrypt_ack_push(msg),
            _ => Err(GodarkError::Order {
                message: format!("Unexpected response type: {msg_type}"),
                error_code: None,
            }),
        }
    }

    fn decrypt_ack_push(&self, msg: &Value) -> Result<OrderAck, GodarkError> {
        if let Some(err) = msg.get("_decrypt_error").and_then(|v| v.as_str()) {
            return Err(GodarkError::Encryption(err.to_string()));
        }
        let plaintext = if let Some(b64) = msg.get("_decrypted_plaintext").and_then(|v| v.as_str())
        {
            BASE64
                .decode(b64)
                .map_err(|e| GodarkError::Encryption(format!("cached ack plaintext: {e}")))?
        } else {
            let ct_b64 = msg
                .get("encrypted_body")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let ct = BASE64
                .decode(ct_b64)
                .map_err(|e| GodarkError::Encryption(format!("base64 decode: {e}")))?;
            let nonce = msg.get("nonce").and_then(|v| v.as_u64()).unwrap_or(0);
            let user_uuid_bytes = self.current_user_uuid_bytes();
            let message_type = msg
                .get("message_type")
                .and_then(|v| v.as_str())
                .unwrap_or("ack");
            let fencing_epoch = msg
                .get("fencing_epoch")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let aad = proto_bridge::build_response_header_aad(
                &user_uuid_bytes,
                message_type,
                ct.len() as u32,
                nonce,
                fencing_epoch,
                &response_correlation_id_bytes(msg),
                json_u64(msg, "session_seq").unwrap_or_default(),
                self.session
                    .lock()
                    .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
                    .conn_id()
                    .unwrap_or_default(),
            );

            self.session
                .lock()
                .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
                .decrypt_push(nonce, &aad, &ct)
                .map_err(|e| GodarkError::Encryption(format!("Failed to decrypt ack: {e}")))?
        };

        let ack_result = proto_bridge::parse_node_response(&plaintext)?;
        match ack_result {
            proto_bridge::NodeResponseKind::Ack {
                order_id,
                success,
                sequence,
                error_code,
                reject_text,
                ..
            } => {
                if !success {
                    return Err(crate::order_error_code::make_order_error_from_code(
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

    /// Decrypts an `encrypted_push` command response and returns the plaintext
    /// `NodeResponse` bytes. Used by the mass-quote / batch ack pipelines.
    fn decrypt_command_plaintext(
        &self,
        msg: &Value,
        default_message_type: &str,
    ) -> Result<Vec<u8>, GodarkError> {
        let msg_type = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if msg_type == "error" {
            let message = msg
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(GodarkError::Order {
                message: message.to_string(),
                error_code: None,
            });
        }
        if msg_type != "encrypted_push" {
            return Err(GodarkError::Order {
                message: format!("Unexpected response type: {msg_type}"),
                error_code: None,
            });
        }
        if let Some(err) = msg.get("_decrypt_error").and_then(|v| v.as_str()) {
            return Err(GodarkError::Encryption(err.to_string()));
        }
        if let Some(b64) = msg.get("_decrypted_plaintext").and_then(|v| v.as_str()) {
            return BASE64
                .decode(b64)
                .map_err(|e| GodarkError::Encryption(format!("cached ack plaintext: {e}")));
        }

        let ct_b64 = msg
            .get("encrypted_body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ct = BASE64
            .decode(ct_b64)
            .map_err(|e| GodarkError::Encryption(format!("base64 decode: {e}")))?;
        let nonce = msg.get("nonce").and_then(|v| v.as_u64()).unwrap_or(0);
        let user_uuid_bytes = self.current_user_uuid_bytes();
        let message_type = msg
            .get("message_type")
            .and_then(|v| v.as_str())
            .unwrap_or(default_message_type);
        let fencing_epoch = msg
            .get("fencing_epoch")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let aad = proto_bridge::build_response_header_aad(
            &user_uuid_bytes,
            message_type,
            ct.len() as u32,
            nonce,
            fencing_epoch,
            &response_correlation_id_bytes(msg),
            json_u64(msg, "session_seq").unwrap_or_default(),
            self.session
                .lock()
                .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
                .conn_id()
                .unwrap_or_default(),
        );

        self.session
            .lock()
            .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
            .decrypt_push(nonce, &aad, &ct)
            .map_err(|e| GodarkError::Encryption(format!("Failed to decrypt ack: {e}")))
    }

    fn parse_mass_quote_response(
        &self,
        msg: &Value,
    ) -> Result<crate::types::MassQuoteAck, GodarkError> {
        let plaintext = self.decrypt_command_plaintext(msg, "mass_quote_ack")?;
        proto_bridge::parse_mass_quote_ack(&plaintext)
    }

    fn parse_batch_cancel_response(
        &self,
        msg: &Value,
    ) -> Result<crate::types::BatchCancelAck, GodarkError> {
        let plaintext = self.decrypt_command_plaintext(msg, "batch_cancel_ack")?;
        proto_bridge::parse_batch_cancel_ack(&plaintext)
    }

    fn parse_batch_modify_response(
        &self,
        msg: &Value,
    ) -> Result<crate::types::BatchModifyAck, GodarkError> {
        let plaintext = self.decrypt_command_plaintext(msg, "batch_modify_ack")?;
        match proto_bridge::parse_batch_modify_ack(&plaintext) {
            Ok(ack) => Ok(ack),
            Err(first) => {
                if let Ok(proto_bridge::NodeResponseKind::Ack {
                    success: false,
                    error_code,
                    reject_text,
                    ..
                }) = proto_bridge::parse_node_response(&plaintext)
                {
                    return Err(crate::order_error_code::make_order_error_from_code(
                        error_code,
                        reject_text.as_deref(),
                    ));
                }
                Err(first)
            }
        }
    }

    fn register_place_outcome_waiter(
        &self,
    ) -> Result<(u64, oneshot::Receiver<Result<OrderUpdate, GodarkError>>), GodarkError> {
        let (sender, receiver) = oneshot::channel();
        let mut state = self
            .place_outcomes
            .lock()
            .map_err(|_| GodarkError::Session("Place outcome mutex poisoned".into()))?;
        state.next_token += 1;
        let token = state.next_token;
        state.waiters.push(PlaceOutcomeWaiter {
            token,
            order_id: None,
            sender: Some(sender),
        });
        Ok((token, receiver))
    }

    fn cancel_place_outcome_waiter(&self, token: u64) {
        if let Ok(mut state) = self.place_outcomes.lock() {
            state.waiters.retain(|waiter| waiter.token != token);
        }
    }

    async fn await_place_outcome(
        &self,
        order_id: &str,
        token: u64,
        receiver: oneshot::Receiver<Result<OrderUpdate, GodarkError>>,
    ) -> Result<OrderUpdate, GodarkError> {
        {
            let mut state = self
                .place_outcomes
                .lock()
                .map_err(|_| GodarkError::Session("Place outcome mutex poisoned".into()))?;
            if let Some(update) = state
                .recent
                .iter()
                .find(|update| update.order_id == order_id)
                .cloned()
            {
                state.waiters.retain(|waiter| waiter.token != token);
                return Ok(update);
            }
            if let Some(waiter) = state
                .waiters
                .iter_mut()
                .find(|waiter| waiter.token == token)
            {
                waiter.order_id = Some(order_id.to_string());
            }
        }
        match tokio::time::timeout(self.config.place_order_terminal_timeout, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(GodarkError::Connection(
                "place_order outcome waiter was closed".into(),
            )),
            Err(_) => {
                self.cancel_place_outcome_waiter(token);
                Err(GodarkError::Timeout(format!(
                    "place_order timed out waiting for book confirmation after {:?}",
                    self.config.place_order_terminal_timeout
                )))
            }
        }
    }

    // ------------------------------------------------------------------
    // Internals: event loop
    // ------------------------------------------------------------------

    fn start_event_loop(&mut self, mut rx: mpsc::Receiver<TransportEvent>) {
        let config = self.config.clone();
        let transport = Arc::clone(&self.transport);
        let order_tx = self.order_tx.clone();
        let positions_snapshot_tx = self.positions_snapshot_tx.clone();
        let system_health_tx = self.system_health_tx.clone();
        let balance_tx = self.balance_tx.clone();
        let funding_rate_tx = self.funding_rate_tx.clone();
        let account_margin_tx = self.account_margin_tx.clone();
        let error_tx = self.error_tx.clone();
        let session = Arc::clone(&self.session);
        let user_uuid = Arc::clone(&self.user_uuid);
        let connected = Arc::clone(&self.connected);
        let desired_channels = Arc::clone(&self.desired_channels);
        let reconnect_attempts = Arc::clone(&self.reconnect_attempts);
        let intentional_close = Arc::clone(&self.intentional_close);
        let reconnect_tx = self.reconnect_tx.clone();
        let place_outcomes = Arc::clone(&self.place_outcomes);
        let encrypted_ack_waiters = Arc::clone(&self.encrypted_ack_waiters);

        self.event_handle = Some(tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    TransportEvent::OrderUpdate(val) => {
                        if let Some(update) = parse_cleartext_order_update(&val) {
                            observe_place_order_update(&place_outcomes, &update);
                            let _ = order_tx.send(update).await;
                        }
                    }
                    TransportEvent::EncryptedPush(val) => {
                        match decrypt_push_plaintext(&session, &user_uuid, &val) {
                            Ok(plaintext) => {
                                let message_type = val
                                    .get("message_type")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if message_type.ends_with("ack") {
                                    let mut enriched = val.clone();
                                    if let Some(obj) = enriched.as_object_mut() {
                                        obj.insert(
                                            "_decrypted_plaintext".into(),
                                            Value::String(BASE64.encode(&plaintext)),
                                        );
                                    }
                                    let wire_corr =
                                        json_u128(&val, "correlation_id").filter(|c| *c != 0);
                                    let plaintext_corr =
                                        match proto_bridge::parse_node_response(&plaintext) {
                                            Ok(proto_bridge::NodeResponseKind::Ack {
                                                correlation_id: raw,
                                                ..
                                            }) => {
                                                if raw.len() == 16 {
                                                    let mut arr = [0u8; 16];
                                                    arr.copy_from_slice(&raw);
                                                    // Proto body stores correlation as LE u128.
                                                    Some(u128::from_le_bytes(arr))
                                                } else if raw.len() == 8 {
                                                    let mut arr = [0u8; 8];
                                                    arr.copy_from_slice(&raw);
                                                    Some(u128::from(u64::from_le_bytes(arr)))
                                                } else {
                                                    None
                                                }
                                            }
                                            _ => None,
                                        }
                                        .filter(|c| *c != 0);
                                    if let Ok(mut waiters) = encrypted_ack_waiters.lock() {
                                        for key in [wire_corr, plaintext_corr].into_iter().flatten()
                                        {
                                            if let Some(tx) = waiters.remove(&key) {
                                                let _ = tx.send(enriched.clone());
                                                break;
                                            }
                                        }
                                        // No single-waiter fallback: with concurrent in-flight
                                        // commands, an unmatched/orphan ack must not steal a
                                        // different caller's waiter.
                                    }
                                } else if let Ok(push) =
                                    decode_decrypted_push(message_type, &plaintext)
                                {
                                    match push {
                                        DecodedPush::Order(update) => {
                                            observe_place_order_update(&place_outcomes, &update);
                                            let _ = order_tx.send(update).await;
                                        }
                                        DecodedPush::PositionsSnapshot(snap) => {
                                            let _ = positions_snapshot_tx.send(snap).await;
                                        }
                                        DecodedPush::SystemHealth(health) => {
                                            let _ = system_health_tx.send(health).await;
                                        }
                                        DecodedPush::Balance(b) => {
                                            let _ = balance_tx.send(b).await;
                                        }
                                        DecodedPush::FundingRate(f) => {
                                            let _ = funding_rate_tx.send(f).await;
                                        }
                                        DecodedPush::AccountMargin(a) => {
                                            let _ = account_margin_tx.send(a).await;
                                        }
                                        DecodedPush::BalanceAndPosition { balance, positions } => {
                                            if let Some(b) = balance {
                                                let _ = balance_tx.send(b).await;
                                            }
                                            if let Some(snap) = positions {
                                                let _ = positions_snapshot_tx.send(snap).await;
                                            }
                                        }
                                        DecodedPush::Ignored => {}
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Encrypted push error: {e}");
                                // Unblock a waiting command so callers fail fast instead of
                                // hanging for the full command timeout after a decrypt failure.
                                if val
                                    .get("message_type")
                                    .and_then(|v| v.as_str())
                                    .is_some_and(|mt| mt.ends_with("ack"))
                                {
                                    let mut err_msg = val.clone();
                                    if let Some(obj) = err_msg.as_object_mut() {
                                        obj.insert(
                                            "_decrypt_error".into(),
                                            Value::String(e.to_string()),
                                        );
                                    }
                                    if let Ok(mut waiters) = encrypted_ack_waiters.lock() {
                                        if let Some(corr) =
                                            json_u128(&val, "correlation_id").filter(|c| *c != 0)
                                        {
                                            if let Some(tx) = waiters.remove(&corr) {
                                                let _ = tx.send(err_msg);
                                            }
                                        }
                                        // No single-waiter fallback (see success-path comment).
                                    }
                                }
                                let _ = error_tx.try_send(e);
                            }
                        }
                    }
                    TransportEvent::RekeyRequired(payload) => {
                        tracing::debug!(?payload, "rekey required");
                        // Inflight encrypted commands cannot complete across rekey.
                        fail_encrypted_ack_waiters(
                            &encrypted_ack_waiters,
                            "session rekey in progress",
                        );
                        let current_uuid = user_uuid.lock().ok().and_then(|guard| *guard);
                        if let Some(uid) = current_uuid {
                            if let Err(err) = {
                                let transport = transport.lock().await;
                                setup_hpke_session_with_transport(
                                    &uid,
                                    session.lock().ok().and_then(|s| s.conn_id()).unwrap_or(0),
                                    &config,
                                    &transport,
                                    &session,
                                )
                                .await
                            } {
                                tracing::warn!("Rust client rekey failed: {err:?}");
                                let _ = error_tx
                                    .try_send(GodarkError::Session(format!("Rekey failed: {err}")));
                                if let Ok(mut guard) = session.lock() {
                                    guard.reset();
                                }
                            }
                        }
                    }
                    TransportEvent::Disconnected => {
                        connected.store(false, Ordering::SeqCst);
                        if let Ok(mut guard) = session.lock() {
                            guard.reset();
                        }
                        fail_place_outcome_waiters(
                            &place_outcomes,
                            "connection lost while waiting for order confirmation",
                        );
                        fail_encrypted_ack_waiters(
                            &encrypted_ack_waiters,
                            "connection lost while waiting for encrypted command ack",
                        );
                        let _ = reconnect_tx.send(ReconnectEvent::Disconnected).await;
                        if let Some(next_rx) = reconnect_transport(
                            &config,
                            &transport,
                            &session,
                            &user_uuid,
                            &desired_channels,
                            &connected,
                            &reconnect_attempts,
                            &intentional_close,
                            &reconnect_tx,
                        )
                        .await
                        {
                            rx = next_rx;
                            continue;
                        }
                        break;
                    }
                    TransportEvent::AuthResult(payload) => {
                        tracing::debug!(?payload, "ignoring late auth_result");
                    }
                    TransportEvent::HpkeSetupReply {
                        conn_id,
                        established,
                    } => {
                        tracing::debug!(conn_id, established, "hpke setup reply already applied");
                    }
                }
            }
        }));
    }

    // ------------------------------------------------------------------
    // Helpers
    // ------------------------------------------------------------------

    fn ensure_ready(&self) -> Result<(), GodarkError> {
        if !self.connected.load(Ordering::SeqCst) {
            return Err(GodarkError::Connection("Not connected".into()));
        }
        if self
            .user_uuid
            .lock()
            .ok()
            .and_then(|guard| *guard)
            .is_none()
        {
            return Err(GodarkError::Connection("Not authenticated".into()));
        }
        let session = self
            .session
            .lock()
            .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?;
        if !session.is_established() {
            return Err(GodarkError::Session("HPKE session not established".into()));
        }
        Ok(())
    }

    fn current_user_uuid(&self) -> Result<Uuid, GodarkError> {
        self.user_uuid
            .lock()
            .map_err(|_| GodarkError::Connection("User id mutex poisoned".into()))?
            .ok_or_else(|| GodarkError::Connection("Not authenticated".into()))
    }

    fn current_user_uuid_bytes(&self) -> Vec<u8> {
        self.user_uuid
            .lock()
            .ok()
            .and_then(|g| *g)
            .map(|u| u.as_bytes().to_vec())
            .unwrap_or_default()
    }

    fn resolve_symbol(&self, symbol: &str) -> Result<u64, GodarkError> {
        self.config.symbol_map.get(symbol).copied().ok_or_else(|| {
            GodarkError::Config(format!(
                "Unknown symbol '{symbol}'. Known: {:?}",
                self.config.symbol_map.keys().collect::<Vec<_>>()
            ))
        })
    }
}

fn timestamp_ns() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64
}

async fn establish_transport_connection(
    config: &GodarkConfig,
    transport: &Arc<AsyncMutex<EdgeTransport>>,
    session: &Arc<Mutex<CryptoSession>>,
    user_uuid_slot: &Arc<Mutex<Option<Uuid>>>,
) -> Result<mpsc::Receiver<TransportEvent>, GodarkError> {
    let mut transport = transport.lock().await;
    transport.connect().await?;

    let auth_result = transport.authenticate(&config.auth_token).await?;
    if auth_result.get("success").and_then(|v| v.as_bool()) != Some(true) {
        transport.disconnect().await;
        let err = auth_result
            .get("error")
            .and_then(|v| v.as_str())
            .unwrap_or("authentication failed");
        return Err(GodarkError::Authentication(err.to_string()));
    }

    let uid = parse_user_uuid_from_auth(&auth_result).or_else(|_| {
        config.user_uuid.ok_or_else(|| {
            GodarkError::Authentication(
                "auth response has no user_uuid and none configured \
                     (set GODARK_USER_UUID or pass .user_uuid() to the builder)"
                    .into(),
            )
        })
    })?;

    {
        let mut guard = user_uuid_slot
            .lock()
            .map_err(|_| GodarkError::Connection("User id mutex poisoned".into()))?;
        *guard = Some(uid);
    }

    if let Err(err) = setup_hpke_session_with_transport(
        &uid,
        parse_conn_id_from_auth(&auth_result)?,
        config,
        &transport,
        session,
    )
    .await
    {
        transport.disconnect().await;
        if let Ok(mut guard) = user_uuid_slot.lock() {
            *guard = None;
        }
        return Err(err);
    }

    transport
        .take_event_receiver()
        .ok_or_else(|| GodarkError::Session("No event receiver after connect".into()))
}

async fn setup_hpke_session_with_transport(
    user_uuid: &Uuid,
    conn_id: u64,
    config: &GodarkConfig,
    transport: &EdgeTransport,
    session: &Arc<Mutex<CryptoSession>>,
) -> Result<(), GodarkError> {
    let pin_hex = config.hpke_static_public_key_hex.as_deref().ok_or_else(|| {
        GodarkError::Config(
            "HPKE static public key unset; pass .hpke_static_public_key_hex() or set GDX_HPKE_STATIC_PUBLIC_KEY".into(),
        )
    })?;
    let remote_static = parse_pinned_static_public_key(pin_hex)?;
    let encapped = {
        let mut sess = session
            .lock()
            .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?;
        sess.setup(&remote_static, *user_uuid, conn_id)?
    };
    let frame = wire::encode_hpke_setup(user_uuid.as_bytes(), conn_id, &encapped);
    let reply = match tokio::time::timeout(HPKE_SETUP_TIMEOUT, transport.send_hpke_setup(frame)).await {
        Ok(Ok(reply)) => reply,
        Ok(Err(err)) => {
            if let Ok(mut sess) = session.lock() {
                sess.abort_setup();
            }
            return Err(err);
        }
        Err(_) => {
            if let Ok(mut sess) = session.lock() {
                sess.abort_setup();
            }
            return Err(GodarkError::Session("HPKE setup timed out".into()));
        }
    };
    let reply_conn_id = reply
        .get("conn_id")
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
        .unwrap_or(0);
    if reply_conn_id != conn_id {
        if let Ok(mut sess) = session.lock() {
            sess.abort_setup();
        }
        return Err(GodarkError::Session(format!(
            "HPKE setup conn_id mismatch: expected {conn_id}, got {reply_conn_id}"
        )));
    }
    if reply.get("established").and_then(|v| v.as_bool()) != Some(true) {
        if let Ok(mut sess) = session.lock() {
            sess.abort_setup();
        }
        return Err(GodarkError::Session("HPKE setup not established".into()));
    }
    {
        let mut sess = session
            .lock()
            .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?;
        sess.establish()?;
    }
    tracing::info!("HPKE session established (conn_id={conn_id})");
    Ok(())
}

fn parse_conn_id_from_auth(msg: &Value) -> Result<u64, GodarkError> {
    msg.get("conn_id")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .filter(|id| *id != 0)
        .ok_or_else(|| GodarkError::Session("login response missing conn_id".into()))
}

async fn resubscribe_desired_channels(
    transport: &Arc<AsyncMutex<EdgeTransport>>,
    desired_channels: &Arc<Mutex<HashSet<String>>>,
) -> Result<(), GodarkError> {
    let channels = desired_channels
        .lock()
        .map_err(|_| GodarkError::Connection("Desired channel mutex poisoned".into()))?
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    if channels.is_empty() {
        return Ok(());
    }

    transport
        .lock()
        .await
        .send_subscribe(&channels, "subscribe")
        .await
}

fn reconnect_backoff_delay(attempt: u32) -> Duration {
    let secs = (1u64 << attempt.min(4)).min(MAX_BACKOFF.as_secs());
    Duration::from_secs(secs.max(1))
}

#[allow(clippy::too_many_arguments)]
async fn reconnect_transport(
    config: &GodarkConfig,
    transport: &Arc<AsyncMutex<EdgeTransport>>,
    session: &Arc<Mutex<CryptoSession>>,
    user_uuid_slot: &Arc<Mutex<Option<Uuid>>>,
    desired_channels: &Arc<Mutex<HashSet<String>>>,
    connected: &Arc<AtomicBool>,
    reconnect_attempts: &Arc<AtomicU32>,
    intentional_close: &Arc<AtomicBool>,
    reconnect_tx: &mpsc::Sender<ReconnectEvent>,
) -> Option<mpsc::Receiver<TransportEvent>> {
    if !config.auto_reconnect || intentional_close.load(Ordering::SeqCst) {
        return None;
    }

    loop {
        if intentional_close.load(Ordering::SeqCst) {
            return None;
        }

        let prev = reconnect_attempts.fetch_add(1, Ordering::SeqCst);
        let delay = reconnect_backoff_delay(prev);
        let _ = reconnect_tx
            .send(ReconnectEvent::Attempting {
                attempt: prev.saturating_add(1),
                delay,
            })
            .await;
        tracing::warn!("Rust client disconnected; reconnecting in {:?}", delay);
        tokio::time::sleep(delay).await;

        if intentional_close.load(Ordering::SeqCst) {
            return None;
        }

        {
            let mut guard = transport.lock().await;
            *guard =
                EdgeTransport::new(&config::ws_url(&config.base_url), config.transport.clone());
        }

        match establish_transport_connection(config, transport, session, user_uuid_slot).await {
            Ok(rx) => {
                if let Err(err) = resubscribe_desired_channels(transport, desired_channels).await {
                    connected.store(false, Ordering::SeqCst);
                    let _ = reconnect_tx
                        .send(ReconnectEvent::Failed {
                            error: err.to_string(),
                        })
                        .await;
                    tracing::warn!("Rust client resubscribe failed after reconnect: {err:?}");
                    continue;
                }
                reconnect_attempts.store(0, Ordering::SeqCst);
                connected.store(true, Ordering::SeqCst);
                let _ = reconnect_tx.send(ReconnectEvent::Reconnected).await;
                tracing::info!("Rust client reconnected");
                return Some(rx);
            }
            Err(err) => {
                connected.store(false, Ordering::SeqCst);
                let _ = reconnect_tx
                    .send(ReconnectEvent::Failed {
                        error: err.to_string(),
                    })
                    .await;
                tracing::warn!("Rust client reconnect attempt failed: {err:?}");
            }
        }
    }
}

enum DecodedPush {
    Order(OrderUpdate),
    PositionsSnapshot(PositionsSnapshot),
    SystemHealth(SystemHealthUpdate),
    Balance(BalanceUpdate),
    FundingRate(FundingRateUpdate),
    AccountMargin(AccountMarginUpdate),
    BalanceAndPosition {
        balance: Option<BalanceUpdate>,
        positions: Option<PositionsSnapshot>,
    },
    Ignored,
}

fn decrypt_push_plaintext(
    session: &Arc<Mutex<CryptoSession>>,
    user_uuid_slot: &Arc<Mutex<Option<Uuid>>>,
    msg: &Value,
) -> Result<Vec<u8>, GodarkError> {
    let ct_b64 = msg
        .get("encrypted_body")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GodarkError::Encryption("missing encrypted_body".into()))?;
    let ct = BASE64
        .decode(ct_b64)
        .map_err(|e| GodarkError::Encryption(format!("base64 decode: {e}")))?;
    let nonce =
        json_u64(msg, "nonce").ok_or_else(|| GodarkError::Encryption("missing nonce".into()))?;
    let user_uuid_bytes = user_uuid_slot
        .lock()
        .ok()
        .and_then(|g| *g)
        .map(|u| u.as_bytes().to_vec())
        .unwrap_or_default();
    let message_type = msg
        .get("message_type")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GodarkError::Encryption("missing message_type".into()))?;
    let fencing_epoch = json_u64(msg, "fencing_epoch").unwrap_or_default();
    let conn_id = json_u64(msg, "conn_id").unwrap_or_else(|| {
        session
            .lock()
            .ok()
            .and_then(|s| s.conn_id())
            .unwrap_or_default()
    });

    let corr_bytes = response_correlation_id_bytes(msg);
    let session_seq = json_u64(msg, "session_seq").unwrap_or_default();
    let aad = proto_bridge::build_response_header_aad(
        &user_uuid_bytes,
        message_type,
        ct.len() as u32,
        nonce,
        fencing_epoch,
        &corr_bytes,
        session_seq,
        conn_id,
    );

    session
        .lock()
        .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
        .decrypt_push(nonce, &aad, &ct)
        .map_err(|e| GodarkError::Encryption(format!("Failed to decrypt push: {e}")))
}

fn decode_decrypted_push(message_type: &str, plaintext: &[u8]) -> Result<DecodedPush, GodarkError> {
    if message_type.ends_with("ack") {
        return Ok(DecodedPush::Ignored);
    }

    match proto_bridge::parse_sequencer_to_edge_message(plaintext)? {
        EdgeMessage::OrderUpdate(update) => Ok(DecodedPush::Order(update)),
        EdgeMessage::PositionsSnapshot(snap) => Ok(DecodedPush::PositionsSnapshot(snap)),
        EdgeMessage::SystemHealth(h) => Ok(DecodedPush::SystemHealth(h)),
        EdgeMessage::BalanceUpdate(b) => Ok(DecodedPush::Balance(b)),
        EdgeMessage::FundingRateUpdate(f) => Ok(DecodedPush::FundingRate(f)),
        EdgeMessage::AccountMarginUpdate(a) => Ok(DecodedPush::AccountMargin(a)),
        EdgeMessage::BalanceAndPosition { balance, positions } => {
            Ok(DecodedPush::BalanceAndPosition { balance, positions })
        }
        EdgeMessage::Unknown => Ok(DecodedPush::Ignored),
    }
}

fn is_terminal_place_update(update: &OrderUpdate) -> bool {
    matches!(
        update.update_type,
        OrderUpdateType::Open
            | OrderUpdateType::Rejected
            | OrderUpdateType::Filled
            | OrderUpdateType::PartiallyFilled
            | OrderUpdateType::Cancelled
    ) || matches!(
        update.status,
        OrderStatus::Rejected | OrderStatus::Filled | OrderStatus::Cancelled
    )
}

fn fail_place_outcome_waiters(state: &Arc<Mutex<PlaceOutcomeState>>, message: &str) {
    let Ok(mut state) = state.lock() else {
        return;
    };
    state.recent.clear();
    let waiters = std::mem::take(&mut state.waiters);
    for waiter in waiters {
        if let Some(sender) = waiter.sender {
            let _ = sender.send(Err(GodarkError::Connection(message.to_string())));
        }
    }
}

/// Drop all pending encrypted-command oneshots. Closing the sender makes the
/// awaiting `rx` complete with `RecvError`, which `send_encrypted_command`
/// maps to a connection error — fail-fast instead of waiting out the timeout.
fn fail_encrypted_ack_waiters(
    waiters: &Arc<Mutex<HashMap<u128, oneshot::Sender<Value>>>>,
    _message: &str,
) {
    let Ok(mut waiters) = waiters.lock() else {
        return;
    };
    waiters.clear();
}

fn observe_place_order_update(state: &Arc<Mutex<PlaceOutcomeState>>, update: &OrderUpdate) {
    if !is_terminal_place_update(update) {
        return;
    }
    let Ok(mut state) = state.lock() else {
        return;
    };
    state.recent.push_back(update.clone());
    if state.recent.len() > 64 {
        state.recent.pop_front();
    }
    if let Some(index) = state
        .waiters
        .iter()
        .position(|waiter| waiter.order_id.as_deref() == Some(update.order_id.as_str()))
    {
        if let Some(sender) = state.waiters.remove(index).sender {
            let _ = sender.send(Ok(update.clone()));
        }
    }
}

fn parse_cleartext_order_update(msg: &Value) -> Option<OrderUpdate> {
    Some(OrderUpdate {
        order_id: json_string(msg, "order_id", ""),
        user_uuid: json_uuid(msg),
        symbol_id: json_u64(msg, "symbol_id").unwrap_or_default(),
        side: parse_side(msg.get("side").and_then(Value::as_str).unwrap_or("BUY")),
        status: parse_order_status(
            msg.get("order_status")
                .or_else(|| msg.get("status"))
                .and_then(Value::as_str)
                .unwrap_or("NEW"),
        ),
        update_type: parse_order_update_type(
            msg.get("message_type")
                .and_then(Value::as_str)
                .unwrap_or("OPEN"),
        ),
        price: json_string(msg, "price", "0"),
        quantity: json_string(msg, "quantity", "0"),
        filled_qty: json_string(msg, "filled_qty", "0"),
        remaining_qty: json_string(msg, "remaining_qty", "0"),
        cum_fill: json_string(msg, "cum_fill", "0"),
        cancel_reason: parse_cancel_reason(msg.get("cancel_reason").and_then(Value::as_str)),
        reject_reason: msg
            .get("reject_reason")
            .or_else(|| msg.get("reject_reason_code"))
            .map(|value| {
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string())
            }),
        msg: msg
            .get("msg")
            .or_else(|| msg.get("reject_text"))
            .and_then(Value::as_str)
            .map(str::to_string),
        correlation_id: json_u128(msg, "correlation_id").unwrap_or_default(),
        timestamp: json_u64(msg, "timestamp").unwrap_or_default(),
    })
}

/// Parse UUID from auth_result JSON — tries `user_uuid` (string) first, falls back to `user_id`.
fn parse_user_uuid_from_auth(msg: &Value) -> Result<Uuid, GodarkError> {
    if let Some(s) = msg.get("user_uuid").and_then(|v| v.as_str()) {
        return Uuid::parse_str(s).map_err(|e| {
            GodarkError::Authentication(format!("invalid user_uuid in auth_result: {e}"))
        });
    }
    if let Some(s) = msg.get("user_id").and_then(|v| v.as_str()) {
        return Uuid::parse_str(s).map_err(|e| {
            GodarkError::Authentication(format!("invalid user_id UUID in auth_result: {e}"))
        });
    }
    Err(GodarkError::Authentication(
        "authentication succeeded but user_uuid missing".into(),
    ))
}

/// Parse a UUID from the `user_uuid` or `user_id` JSON field.
fn json_uuid(msg: &Value) -> Uuid {
    if let Some(s) = msg
        .get("user_uuid")
        .or_else(|| msg.get("user_id"))
        .and_then(|v| v.as_str())
    {
        if let Ok(u) = Uuid::parse_str(s) {
            return u;
        }
    }
    Uuid::nil()
}

fn json_string(msg: &Value, key: &str, default: &str) -> String {
    msg.get(key)
        .and_then(|v| {
            v.as_str()
                .map(str::to_string)
                .or_else(|| v.as_u64().map(|n| n.to_string()))
                .or_else(|| v.as_i64().map(|n| n.to_string()))
                .or_else(|| v.as_f64().map(|n| n.to_string()))
        })
        .unwrap_or_else(|| default.to_string())
}

fn json_u64(msg: &Value, key: &str) -> Option<u64> {
    msg.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().and_then(|n| u64::try_from(n).ok()))
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    })
}

fn json_u128(msg: &Value, key: &str) -> Option<u128> {
    msg.get(key).and_then(|v| {
        v.as_u64().map(u128::from).or_else(|| {
            v.as_str().and_then(|s| {
                let s = s.trim();
                s.parse::<u128>().ok().or_else(|| {
                    let hex = s.strip_prefix("0x").unwrap_or(s);
                    u128::from_str_radix(hex, 16).ok()
                })
            })
        })
    })
}

fn response_correlation_id_bytes(msg: &Value) -> Vec<u8> {
    json_u128(msg, "correlation_id")
        .filter(|value| *value != 0)
        .map(|value| value.to_be_bytes().to_vec())
        .unwrap_or_default()
}

fn parse_side(raw: &str) -> Side {
    match raw {
        "SELL" => Side::Sell,
        _ => Side::Buy,
    }
}

fn parse_order_status(raw: &str) -> OrderStatus {
    match raw {
        "PARTIALLY_FILLED" => OrderStatus::PartiallyFilled,
        "FILLED" => OrderStatus::Filled,
        "CANCELLED" => OrderStatus::Cancelled,
        "REJECTED" => OrderStatus::Rejected,
        _ => OrderStatus::New,
    }
}

fn parse_order_update_type(raw: &str) -> OrderUpdateType {
    match raw {
        "FILLED" => OrderUpdateType::Filled,
        "PARTIALLY_FILLED" => OrderUpdateType::PartiallyFilled,
        "CANCELLED" => OrderUpdateType::Cancelled,
        "REJECTED" => OrderUpdateType::Rejected,
        "MODIFIED" => OrderUpdateType::Modified,
        "CANCEL_REJECTED" => OrderUpdateType::CancelRejected,
        "MODIFY_REJECTED" => OrderUpdateType::ModifyRejected,
        _ => OrderUpdateType::Open,
    }
}

fn parse_cancel_reason(raw: Option<&str>) -> Option<CancelReason> {
    match raw {
        Some("USER_REQUESTED") => Some(CancelReason::UserRequested),
        Some("IOC_REMAINDER") => Some(CancelReason::IocRemainder),
        Some("FOK_NOT_FILLED") => Some(CancelReason::FokNotFilled),
        Some("EXPIRED") => Some(CancelReason::Expired),
        Some("SYSTEM") => Some(CancelReason::System),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::json;

    use super::*;
    use crate::enums::{OrderType, Side, TimeInForce};
    use crate::types::{Confirmation, ReconnectEvent};

    fn test_config() -> GodarkConfig {
        GodarkClient::builder().api_key("test").build().unwrap()
    }

    #[test]
    fn test_builder_creates_client() {
        let config = GodarkClient::builder().api_key("test").build().unwrap();
        let client = GodarkClient::new(config);
        assert!(!client.is_connected());
    }

    #[test]
    fn test_new_client_not_connected() {
        let client = GodarkClient::new(test_config());
        assert!(!client.is_connected());
        assert!(client.user_uuid().is_none());
    }

    #[tokio::test]
    async fn test_place_outcome_waiter_handles_push_before_ack() {
        let client = GodarkClient::new(test_config());
        let (token, receiver) = client.register_place_outcome_waiter().unwrap();
        let update = OrderUpdate {
            order_id: "42".into(),
            user_uuid: Uuid::nil(),
            symbol_id: 1,
            side: Side::Buy,
            status: OrderStatus::New,
            update_type: OrderUpdateType::Open,
            price: "1".into(),
            quantity: "1".into(),
            filled_qty: "0".into(),
            remaining_qty: "1".into(),
            cum_fill: "0".into(),
            cancel_reason: None,
            reject_reason: None,
            msg: None,
            correlation_id: 0,
            timestamp: 0,
        };
        observe_place_order_update(&client.place_outcomes, &update);
        let got = client
            .await_place_outcome("42", token, receiver)
            .await
            .unwrap();
        assert_eq!(got, update);
    }

    #[tokio::test]
    async fn test_place_outcome_waiter_disconnect_clears_cache() {
        let client = GodarkClient::new(test_config());
        let (token, receiver) = client.register_place_outcome_waiter().unwrap();
        {
            let mut state = client.place_outcomes.lock().unwrap();
            state
                .waiters
                .iter_mut()
                .find(|w| w.token == token)
                .unwrap()
                .order_id = Some("99".into());
            state.recent.push_back(OrderUpdate {
                order_id: "99".into(),
                user_uuid: Uuid::nil(),
                symbol_id: 1,
                side: Side::Buy,
                status: OrderStatus::New,
                update_type: OrderUpdateType::Open,
                price: "1".into(),
                quantity: "1".into(),
                filled_qty: "0".into(),
                remaining_qty: "1".into(),
                cum_fill: "0".into(),
                cancel_reason: None,
                reject_reason: None,
                msg: None,
                correlation_id: 0,
                timestamp: 0,
            });
        }
        fail_place_outcome_waiters(&client.place_outcomes, "disconnected while waiting");
        let err = receiver.await.unwrap().unwrap_err();
        assert!(matches!(err, GodarkError::Connection(ref m) if m.contains("disconnected")));
        let state = client.place_outcomes.lock().unwrap();
        assert!(state.waiters.is_empty());
        assert!(state.recent.is_empty());
    }

    #[tokio::test]
    async fn test_fail_encrypted_ack_waiters_cancels_pending() {
        let client = GodarkClient::new(test_config());
        let (tx, rx) = oneshot::channel();
        {
            let mut waiters = client.encrypted_ack_waiters.lock().unwrap();
            waiters.insert(0xabcdu128, tx);
        }
        fail_encrypted_ack_waiters(
            &client.encrypted_ack_waiters,
            "disconnected while waiting for encrypted command ack",
        );
        assert!(
            client.encrypted_ack_waiters.lock().unwrap().is_empty(),
            "map must be cleared"
        );
        assert!(rx.await.is_err(), "oneshot must be cancelled by clear");
    }

    #[test]
    fn test_confirmation_default_is_book() {
        assert_eq!(Confirmation::default(), Confirmation::Book);
    }

    #[test]
    fn test_is_terminal_place_update_covers_fill_states() {
        let base = OrderUpdate {
            order_id: "1".into(),
            user_uuid: Uuid::nil(),
            symbol_id: 1,
            side: Side::Buy,
            status: OrderStatus::New,
            update_type: OrderUpdateType::Modified,
            price: "1".into(),
            quantity: "1".into(),
            filled_qty: "0".into(),
            remaining_qty: "1".into(),
            cum_fill: "0".into(),
            cancel_reason: None,
            reject_reason: None,
            msg: None,
            correlation_id: 0,
            timestamp: 0,
        };
        assert!(!is_terminal_place_update(&base));
        for update_type in [
            OrderUpdateType::Open,
            OrderUpdateType::Rejected,
            OrderUpdateType::Filled,
            OrderUpdateType::PartiallyFilled,
            OrderUpdateType::Cancelled,
        ] {
            let mut u = base.clone();
            u.update_type = update_type;
            assert!(is_terminal_place_update(&u));
        }
    }

    #[test]
    fn test_place_order_terminal_timeout_rejects_zero() {
        let err = GodarkClient::builder()
            .api_key("k")
            .place_order_terminal_timeout(Duration::ZERO)
            .build()
            .unwrap_err();
        assert!(matches!(err, GodarkError::Config(ref m) if m.contains("greater than zero")));
    }

    #[test]
    fn test_parse_cleartext_order_update_reject_text() {
        let msg = json!({
            "order_id": "7",
            "user_uuid": "00000000-0000-0000-0000-000000000001",
            "symbol_id": 1,
            "side": "BUY",
            "status": "REJECTED",
            "update_type": "REJECTED",
            "price": "1",
            "quantity": "1",
            "filled_qty": "0",
            "remaining_qty": "1",
            "cum_fill": "0",
            "reject_reason": 2007,
            "reject_text": "far from mark",
            "correlation_id": 0,
            "timestamp": 1
        });
        let update = parse_cleartext_order_update(&msg).expect("parse");
        assert_eq!(update.reject_reason.as_deref(), Some("2007"));
        assert_eq!(update.msg.as_deref(), Some("far from mark"));
    }

    #[tokio::test]
    async fn test_ensure_ready_not_connected() {
        let client = GodarkClient::new(test_config());
        let err = client
            .place_order(
                "BTC-USDC-PERP",
                Side::Buy,
                OrderType::Limit,
                1.0,
                Some(100.0),
                TimeInForce::Gtc,
                false,
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, GodarkError::Connection(ref s) if s == "Not connected"));
    }

    #[test]
    fn test_resolve_symbol_known() {
        let client = GodarkClient::new(test_config());
        assert_eq!(client.resolve_symbol("BTC-USDC-PERP").unwrap(), 1);
    }

    #[test]
    fn test_resolve_symbol_unknown() {
        let client = GodarkClient::new(test_config());
        let err = client.resolve_symbol("UNKNOWN").unwrap_err();
        assert!(matches!(err, GodarkError::Config(_)));
    }

    #[test]
    fn test_take_error_receiver() {
        let mut client = GodarkClient::new(test_config());
        assert!(client.take_error_receiver().is_some());
        assert!(client.take_error_receiver().is_none());
    }

    #[test]
    fn test_take_order_receiver() {
        let mut client = GodarkClient::new(test_config());
        assert!(client.take_order_receiver().is_some());
        assert!(client.take_order_receiver().is_none());
    }

    #[test]
    fn test_take_positions_snapshot_receiver() {
        let mut client = GodarkClient::new(test_config());
        assert!(client.take_positions_snapshot_receiver().is_some());
        assert!(client.take_positions_snapshot_receiver().is_none());
    }

    #[tokio::test]
    async fn test_place_order_when_disconnected() {
        let client = GodarkClient::new(test_config());
        let err = client
            .place_order(
                "BTC-USDC-PERP",
                Side::Buy,
                OrderType::Market,
                0.1,
                None,
                TimeInForce::Ioc,
                false,
                None,
                None,
            )
            .await
            .unwrap_err();
        match err {
            GodarkError::Connection(msg) => assert_eq!(msg, "Not connected"),
            other => panic!("expected Connection error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_cancel_order_when_disconnected() {
        let client = GodarkClient::new(test_config());
        let err = client
            .cancel_order("12345", "BTC-USDC-PERP")
            .await
            .unwrap_err();
        assert!(matches!(err, GodarkError::Connection(_)));
    }

    #[tokio::test]
    async fn test_modify_order_when_disconnected() {
        let client = GodarkClient::new(test_config());
        let err = client
            .modify_order("12345", "BTC-USDC-PERP", Some(100.0), None)
            .await
            .unwrap_err();
        assert!(matches!(err, GodarkError::Connection(_)));
    }

    #[tokio::test]
    async fn test_start_event_loop_routes_order_updates() {
        let mut client = GodarkClient::new(test_config());
        let mut order_rx = client.take_order_receiver().expect("order receiver");
        let (event_tx, event_rx) = mpsc::channel(8);

        client.start_event_loop(event_rx);
        event_tx
            .send(TransportEvent::OrderUpdate(json!({
                "type": "order_update",
                "order_id": "42",
                "user_uuid": "00000000-0000-0000-0000-000000000007",
                "symbol_id": 1,
                "side": "SELL",
                "order_status": "PARTIALLY_FILLED",
                "message_type": "PARTIALLY_FILLED",
                "price": "101.5",
                "quantity": "2",
                "filled_qty": "1",
                "remaining_qty": "1",
                "cum_fill": "1",
                "cancel_reason": "USER_REQUESTED",
                "correlation_id": "99",
                "timestamp": 123
            })))
            .await
            .expect("send event");

        let update = tokio::time::timeout(Duration::from_millis(100), order_rx.recv())
            .await
            .expect("receive timeout")
            .expect("order update");

        assert_eq!(update.order_id, "42");
        assert_eq!(
            update.user_uuid,
            Uuid::parse_str("00000000-0000-0000-0000-000000000007").unwrap()
        );
        assert_eq!(update.symbol_id, 1);
        assert_eq!(update.side, Side::Sell);
        assert_eq!(update.status, OrderStatus::PartiallyFilled);
        assert_eq!(update.update_type, OrderUpdateType::PartiallyFilled);
        assert_eq!(update.cancel_reason, Some(CancelReason::UserRequested));
        assert_eq!(update.correlation_id, 99);
        assert_eq!(update.timestamp, 123);
    }

    #[tokio::test]
    async fn test_start_event_loop_marks_client_disconnected() {
        let mut client = GodarkClient::new(test_config());
        client.connected.store(true, Ordering::SeqCst);
        let (event_tx, event_rx) = mpsc::channel(8);

        client.start_event_loop(event_rx);
        event_tx
            .send(TransportEvent::Disconnected)
            .await
            .expect("send event");

        tokio::time::sleep(Duration::from_millis(25)).await;
        assert!(!client.is_connected());
    }

    #[tokio::test]
    async fn test_reconnect_hook_emits_disconnected_when_auto_reconnect_off() {
        let config = GodarkClient::builder()
            .api_key("test")
            .auto_reconnect(false)
            .build()
            .unwrap();
        let mut client = GodarkClient::new(config);
        let mut reconnect_rx = client
            .take_reconnect_receiver()
            .expect("reconnect receiver");
        let (event_tx, event_rx) = mpsc::channel(8);
        client.connected.store(true, Ordering::SeqCst);
        client.start_event_loop(event_rx);
        event_tx
            .send(TransportEvent::Disconnected)
            .await
            .expect("send event");

        let ev = tokio::time::timeout(Duration::from_millis(200), reconnect_rx.recv())
            .await
            .expect("timeout")
            .expect("event");
        assert_eq!(ev, ReconnectEvent::Disconnected);
    }
}
