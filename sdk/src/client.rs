// GodarkClient — main entry point, mirrors Python SDK client.py

use std::collections::{BTreeMap, HashSet, VecDeque};
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
use crate::enums::{
    CancelReason, OrderStatus, OrderType, OrderUpdateType, PositionUpdateType, Side, TimeInForce,
};
use crate::error::GodarkError;
use crate::proto_bridge::{self, EdgeMessage};
use crate::session::{
    build_initiator, parse_pinned_static_public_key, prologue_for_user, read_handshake,
    write_handshake, CryptoSession,
};
use crate::transport::{EdgeTransport, TransportEvent};
use crate::types::{
    BalanceUpdate, Confirmation, FundingRateUpdate, MarginAlert, OrderAck, OrderUpdate,
    PositionUpdate, PositionsSnapshot, ReconnectEvent, SettlementUpdate, SystemHealthUpdate,
};

const NOISE_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const _MAX_BACKOFF: Duration = Duration::from_secs(15);
const MAX_ENCRYPTED_PUSH_BUFFER: usize = 64;

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
    #[allow(dead_code)]
    order_tx: mpsc::Sender<OrderUpdate>,
    order_rx: Option<mpsc::Receiver<OrderUpdate>>,
    #[allow(dead_code)]
    position_tx: mpsc::Sender<PositionUpdate>,
    position_rx: Option<mpsc::Receiver<PositionUpdate>>,
    #[allow(dead_code)]
    positions_snapshot_tx: mpsc::Sender<PositionsSnapshot>,
    positions_snapshot_rx: Option<mpsc::Receiver<PositionsSnapshot>>,
    #[allow(dead_code)]
    system_health_tx: mpsc::Sender<SystemHealthUpdate>,
    system_health_rx: Option<mpsc::Receiver<SystemHealthUpdate>>,
    #[allow(dead_code)]
    balance_tx: mpsc::Sender<BalanceUpdate>,
    balance_rx: Option<mpsc::Receiver<BalanceUpdate>>,
    #[allow(dead_code)]
    margin_alert_tx: mpsc::Sender<MarginAlert>,
    margin_alert_rx: Option<mpsc::Receiver<MarginAlert>>,
    #[allow(dead_code)]
    funding_rate_tx: mpsc::Sender<FundingRateUpdate>,
    funding_rate_rx: Option<mpsc::Receiver<FundingRateUpdate>>,
    #[allow(dead_code)]
    settlement_tx: mpsc::Sender<SettlementUpdate>,
    settlement_rx: Option<mpsc::Receiver<SettlementUpdate>>,
    #[allow(dead_code)]
    error_tx: mpsc::Sender<GodarkError>,
    error_rx: Option<mpsc::Receiver<GodarkError>>,
    event_handle: Option<JoinHandle<()>>,
    reconnect_attempts: Arc<AtomicU32>,
    intentional_close: Arc<AtomicBool>,
    reconnect_tx: mpsc::Sender<ReconnectEvent>,
    reconnect_rx: Option<mpsc::Receiver<ReconnectEvent>>,
    place_outcomes: Arc<Mutex<PlaceOutcomeState>>,
    encrypted_push_buffer: Arc<Mutex<BTreeMap<u32, Value>>>,
}

impl GodarkClient {
    pub fn builder() -> GodarkConfigBuilder {
        GodarkConfigBuilder::new()
    }

    pub fn new(config: GodarkConfig) -> Self {
        let ws_url = config::ws_url(&config.base_url);
        let (order_tx, order_rx) = mpsc::channel(256);
        let (position_tx, position_rx) = mpsc::channel(256);
        let (positions_snapshot_tx, positions_snapshot_rx) = mpsc::channel(64);
        let (system_health_tx, system_health_rx) = mpsc::channel(64);
        let (balance_tx, balance_rx) = mpsc::channel(64);
        let (margin_alert_tx, margin_alert_rx) = mpsc::channel(64);
        let (funding_rate_tx, funding_rate_rx) = mpsc::channel(64);
        let (settlement_tx, settlement_rx) = mpsc::channel(64);
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
            position_tx,
            position_rx: Some(position_rx),
            positions_snapshot_tx,
            positions_snapshot_rx: Some(positions_snapshot_rx),
            system_health_tx,
            system_health_rx: Some(system_health_rx),
            balance_tx,
            balance_rx: Some(balance_rx),
            margin_alert_tx,
            margin_alert_rx: Some(margin_alert_rx),
            funding_rate_tx,
            funding_rate_rx: Some(funding_rate_rx),
            settlement_tx,
            settlement_rx: Some(settlement_rx),
            error_tx,
            error_rx: Some(error_rx),
            event_handle: None,
            reconnect_attempts: Arc::new(AtomicU32::new(0)),
            intentional_close: Arc::new(AtomicBool::new(false)),
            reconnect_tx,
            reconnect_rx: Some(reconnect_rx),
            place_outcomes: Arc::new(Mutex::new(PlaceOutcomeState::default())),
            encrypted_push_buffer: Arc::new(Mutex::new(BTreeMap::new())),
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

    pub fn take_position_receiver(&mut self) -> Option<mpsc::Receiver<PositionUpdate>> {
        self.position_rx.take()
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

    /// Receive shielded balance updates for the authenticated user.
    pub fn take_balance_receiver(&mut self) -> Option<mpsc::Receiver<BalanceUpdate>> {
        self.balance_rx.take()
    }

    /// Receive margin tier transitions / recoveries.
    pub fn take_margin_alert_receiver(&mut self) -> Option<mpsc::Receiver<MarginAlert>> {
        self.margin_alert_rx.take()
    }

    /// Receive per-symbol funding rate ticks.
    pub fn take_funding_rate_receiver(&mut self) -> Option<mpsc::Receiver<FundingRateUpdate>> {
        self.funding_rate_rx.take()
    }

    /// Receive settlement batch lifecycle updates.
    pub fn take_settlement_receiver(&mut self) -> Option<mpsc::Receiver<SettlementUpdate>> {
        self.settlement_rx.take()
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
        if let Some(h) = self.event_handle.take() {
            h.abort();
        }
        self.transport.lock().await.disconnect().await;
        if let Ok(mut session) = self.session.lock() {
            session.reset();
        }
        if let Ok(mut buffer) = self.encrypted_push_buffer.lock() {
            buffer.clear();
        }
        if let Ok(mut guard) = self.user_uuid.lock() {
            *guard = None;
        }
    }

    pub async fn logout(&mut self) -> Result<(), GodarkError> {
        self.intentional_close.store(true, Ordering::SeqCst);
        let result = async {
            if self.connected.load(Ordering::SeqCst) && self.config.transport.use_docs_wire {
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
        &mut self,
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
        &mut self,
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
        &mut self,
        symbol: &str,
        legs: &[crate::types::MassQuoteLegInput],
        leverage: u32,
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
            leverage,
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
        &mut self,
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
        &mut self,
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
    // Internals: ECDH session
    // ------------------------------------------------------------------

    // ------------------------------------------------------------------
    // Internals: encrypted order pipeline
    // ------------------------------------------------------------------

    async fn send_encrypted_order(
        &mut self,
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
        &mut self,
        request_type: &str,
        symbol_id: u64,
        plaintext: &[u8],
        correlation_id: &[u8],
    ) -> Result<Value, GodarkError> {
        let body_length = CryptoSession::body_length_for_plaintext(plaintext.len())?;
        let uuid = self.current_user_uuid()?;
        let (actual_nonce, ciphertext) = {
            let mut session = self
                .session
                .lock()
                .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?;
            let nonce_counter = session.next_nonce();
            let conn_id = session
                .conn_id()
                .ok_or_else(|| GodarkError::Session("Noise XK session not established".into()))?;

            let aad = proto_bridge::build_order_header_aad(
                uuid.as_bytes(),
                symbol_id,
                request_type,
                nonce_counter as u64,
                body_length,
                correlation_id,
                conn_id,
            );

            let (actual_nonce, ciphertext) = session
                .encrypt_order(&aad, plaintext)
                .map_err(|e| GodarkError::Encryption(format!("Failed to encrypt order: {e}")))?;
            (actual_nonce, ciphertext)
        };

        let body_b64 = BASE64.encode(&ciphertext);
        let corr_hex = if correlation_id.len() == 16 {
            let arr: [u8; 16] = correlation_id.try_into().unwrap();
            let v = u128::from_be_bytes(arr);
            if v == 0 {
                None
            } else {
                Some(format!("{v:032x}"))
            }
        } else {
            None
        };
        let mut header_json = serde_json::json!({
            "symbol_id": symbol_id,
            "request_type": request_type,
            "nonce": actual_nonce,
            "body_length": body_length,
        });
        if let Some(cid) = corr_hex {
            header_json["correlation_id"] = serde_json::Value::String(cid);
        }
        let payload = if self.config.transport.use_docs_wire {
            let wire_op = match request_type {
                "place" => "order.place",
                "cancel" => "order.cancel",
                "modify" => "order.modify",
                "mass_quote" => "order.mass_quote",
                "batch_cancel" => "order.batch_cancel",
                "batch_modify" => "order.batch_modify",
                other => {
                    return Err(GodarkError::Config(format!(
                        "invalid encrypted request_type: {other}"
                    )));
                }
            };
            serde_json::json!({
                "id": Uuid::new_v4().to_string(),
                "op": wire_op,
                "args": {
                    "header": header_json,
                    "ciphertext": body_b64,
                }
            })
        } else {
            serde_json::json!({
                "type": "encrypted_order",
                "data": {
                    "header": header_json,
                    "encrypted_body": body_b64,
                }
            })
        };

        let response = self.transport.lock().await.send_command(&payload).await?;
        if let Some(nonce) = json_u64(&response, "nonce") {
            self.wait_for_prior_noise_messages(nonce as u32).await?;
        }
        Ok(response)
    }

    /// A command ack can arrive after an encrypted subscription push. Wait for
    /// the event task to consume preceding Noise transport messages first.
    async fn wait_for_prior_noise_messages(&self, nonce: u32) -> Result<(), GodarkError> {
        let deadline = tokio::time::Instant::now() + self.config.transport.command_timeout;
        loop {
            let expected = self
                .session
                .lock()
                .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
                .recv_nonce();
            if expected >= nonce {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(GodarkError::Timeout(format!(
                    "waiting for encrypted push nonce {expected} before command ack nonce {nonce}"
                )));
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    fn parse_order_response(&mut self, msg: &Value) -> Result<OrderAck, GodarkError> {
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

    fn decrypt_ack_push(&mut self, msg: &Value) -> Result<OrderAck, GodarkError> {
        let ct_b64 = msg
            .get("encrypted_body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ct = BASE64
            .decode(ct_b64)
            .map_err(|e| GodarkError::Encryption(format!("base64 decode: {e}")))?;
        let nonce = msg.get("nonce").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
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
            nonce as u64,
            fencing_epoch,
            &response_correlation_id_bytes(msg),
            json_u64(msg, "session_seq").unwrap_or_default(),
            self.session
                .lock()
                .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
                .conn_id()
                .unwrap_or_default(),
        );

        let plaintext = self
            .session
            .lock()
            .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
            .decrypt_push(nonce, &aad, &ct)
            .map_err(|e| GodarkError::Encryption(format!("Failed to decrypt ack: {e}")))?;

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
        &mut self,
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

        let ct_b64 = msg
            .get("encrypted_body")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let ct = BASE64
            .decode(ct_b64)
            .map_err(|e| GodarkError::Encryption(format!("base64 decode: {e}")))?;
        let nonce = msg.get("nonce").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
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
            nonce as u64,
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
        &mut self,
        msg: &Value,
    ) -> Result<crate::types::MassQuoteAck, GodarkError> {
        let plaintext = self.decrypt_command_plaintext(msg, "mass_quote_ack")?;
        proto_bridge::parse_mass_quote_ack(&plaintext)
    }

    fn parse_batch_cancel_response(
        &mut self,
        msg: &Value,
    ) -> Result<crate::types::BatchCancelAck, GodarkError> {
        let plaintext = self.decrypt_command_plaintext(msg, "batch_cancel_ack")?;
        proto_bridge::parse_batch_cancel_ack(&plaintext)
    }

    fn parse_batch_modify_response(
        &mut self,
        msg: &Value,
    ) -> Result<crate::types::BatchModifyAck, GodarkError> {
        let plaintext = self.decrypt_command_plaintext(msg, "batch_modify_ack")?;
        proto_bridge::parse_batch_modify_ack(&plaintext)
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
        let position_tx = self.position_tx.clone();
        let positions_snapshot_tx = self.positions_snapshot_tx.clone();
        let system_health_tx = self.system_health_tx.clone();
        let balance_tx = self.balance_tx.clone();
        let margin_alert_tx = self.margin_alert_tx.clone();
        let funding_rate_tx = self.funding_rate_tx.clone();
        let settlement_tx = self.settlement_tx.clone();
        let error_tx = self.error_tx.clone();
        let session = Arc::clone(&self.session);
        let user_uuid = Arc::clone(&self.user_uuid);
        let connected = Arc::clone(&self.connected);
        let desired_channels = Arc::clone(&self.desired_channels);
        let reconnect_attempts = Arc::clone(&self.reconnect_attempts);
        let intentional_close = Arc::clone(&self.intentional_close);
        let reconnect_tx = self.reconnect_tx.clone();
        let place_outcomes = Arc::clone(&self.place_outcomes);
        let encrypted_push_buffer = Arc::clone(&self.encrypted_push_buffer);

        self.event_handle = Some(tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                match event {
                    TransportEvent::OrderUpdate(val) => {
                        if let Some(update) = parse_cleartext_order_update(&val) {
                            observe_place_order_update(&place_outcomes, &update);
                            let _ = order_tx.send(update).await;
                        }
                    }
                    TransportEvent::PositionUpdate(val) => {
                        if let Some(update) = parse_cleartext_position_update(&val) {
                            let _ = position_tx.send(update).await;
                        }
                    }
                    TransportEvent::EncryptedPush(val) => {
                        match parse_buffered_encrypted_push(
                            &session,
                            &user_uuid,
                            &encrypted_push_buffer,
                            &val,
                        ) {
                            Ok(pushes) => {
                                for push in pushes {
                                    match push {
                                        DecodedPush::Order(update) => {
                                            observe_place_order_update(&place_outcomes, &update);
                                            let _ = order_tx.send(update).await;
                                        }
                                        DecodedPush::Position(update) => {
                                            let _ = position_tx.send(update).await;
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
                                        DecodedPush::MarginAlert(a) => {
                                            let _ = margin_alert_tx.send(a).await;
                                        }
                                        DecodedPush::FundingRate(f) => {
                                            let _ = funding_rate_tx.send(f).await;
                                        }
                                        DecodedPush::Settlement(s) => {
                                            let _ = settlement_tx.send(s).await;
                                        }
                                        // Future-proof: a sequencer push variant we don't
                                        // recognize is silently dropped — never an error.
                                        DecodedPush::Ignored => {}
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::warn!("Encrypted push error: {e}");
                                let _ = error_tx.try_send(e);
                            }
                        }
                    }
                    TransportEvent::RekeyRequired(_) => {
                        let current_uuid = user_uuid.lock().ok().and_then(|guard| *guard);
                        if let Some(uid) = current_uuid {
                            if let Ok(mut buffer) = encrypted_push_buffer.lock() {
                                buffer.clear();
                            }
                            if let Err(err) = {
                                let transport = transport.lock().await;
                                setup_noise_session_with_transport(
                                    &uid, &config, &transport, &session,
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
                        if let Ok(mut buffer) = encrypted_push_buffer.lock() {
                            buffer.clear();
                        }
                        fail_place_outcome_waiters(
                            &place_outcomes,
                            "connection lost while waiting for order confirmation",
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
                    TransportEvent::AuthResult(_) | TransportEvent::SessionEstablished(_) => {}
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
            return Err(GodarkError::Session(
                "Noise XK session not established".into(),
            ));
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

    if let Err(err) = setup_noise_session_with_transport(&uid, config, &transport, session).await {
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

async fn setup_noise_session_with_transport(
    user_uuid: &Uuid,
    config: &GodarkConfig,
    transport: &EdgeTransport,
    session: &Arc<Mutex<CryptoSession>>,
) -> Result<(), GodarkError> {
    let pin_hex = config.noise_static_public_key_hex.as_deref().ok_or_else(|| {
        GodarkError::Config(
            "Noise static public key unset; pass .noise_static_public_key_hex() or set GDX_NOISE_STATIC_PUBLIC_KEY".into(),
        )
    })?;
    let remote_static = parse_pinned_static_public_key(pin_hex)?;
    let mut initiator = build_initiator(&remote_static, &prologue_for_user(user_uuid))?;

    let send_handshake = |message: Vec<u8>| async move {
        let message = BASE64.encode(message);
        let payload = if transport.use_docs_wire() {
            serde_json::json!({
                "id": Uuid::new_v4().to_string(),
                "op": "noise.handshake",
                "args": { "message": message },
            })
        } else {
            serde_json::json!({
                "type": "noise_handshake",
                "data": { "message": message },
            })
        };
        tokio::time::timeout(NOISE_HANDSHAKE_TIMEOUT, transport.send_command(&payload))
            .await
            .map_err(|_| GodarkError::Session("Noise handshake timed out".into()))?
    };

    let reply1 = send_handshake(write_handshake(&mut initiator)?).await?;
    ensure_noise_reply(&reply1, false)?;
    let conn_id = noise_reply_conn_id(&reply1)?;
    let msg2 = decode_noise_reply_message(&reply1)?;
    read_handshake(&mut initiator, &msg2)?;

    let reply2 = send_handshake(write_handshake(&mut initiator)?).await?;
    ensure_noise_reply(&reply2, true)?;
    if noise_reply_conn_id(&reply2)? != conn_id {
        return Err(GodarkError::Session(
            "noise_handshake_reply conn_id changed during handshake".into(),
        ));
    }
    let transport_state = initiator
        .into_transport_mode()
        .map_err(|e| GodarkError::Session(format!("Noise transport: {e}")))?;
    session
        .lock()
        .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
        .establish(transport_state, conn_id)?;
    tracing::info!("Noise XK session established (conn_id={conn_id})");
    Ok(())
}

fn noise_reply_conn_id(reply: &Value) -> Result<u64, GodarkError> {
    reply
        .get("conn_id")
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        })
        .filter(|id| *id != 0)
        .ok_or_else(|| GodarkError::Session("noise_handshake_reply missing valid conn_id".into()))
}

fn decode_noise_reply_message(reply: &Value) -> Result<Vec<u8>, GodarkError> {
    let message = reply
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GodarkError::Session("noise_handshake_reply missing message".into()))?;
    BASE64
        .decode(message)
        .map_err(|e| GodarkError::Session(format!("invalid Noise handshake message: {e}")))
}

fn ensure_noise_reply(reply: &Value, expected_established: bool) -> Result<(), GodarkError> {
    if reply.get("type").and_then(|v| v.as_str()) == Some("error") {
        let message = reply
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("noise handshake failed");
        return Err(GodarkError::Session(message.into()));
    }
    if reply.get("type").and_then(|v| v.as_str()) != Some("noise_handshake_reply") {
        return Err(GodarkError::Session(
            "invalid Noise handshake response".into(),
        ));
    }
    if reply.get("established").and_then(|v| v.as_bool()) != Some(expected_established) {
        return Err(GodarkError::Session(
            if expected_established {
                "noise_handshake_reply expected established after message 3"
            } else {
                "noise_handshake_reply unexpectedly established after message 1"
            }
            .into(),
        ));
    }
    Ok(())
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
    let secs = (1u64 << attempt.min(4)).min(_MAX_BACKOFF.as_secs());
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
    Position(PositionUpdate),
    PositionsSnapshot(PositionsSnapshot),
    SystemHealth(SystemHealthUpdate),
    Balance(BalanceUpdate),
    MarginAlert(MarginAlert),
    FundingRate(FundingRateUpdate),
    Settlement(SettlementUpdate),
    /// Recognized-but-unhandled push (e.g. a future variant we don't decode);
    /// silently dropped instead of being flagged as an error.
    Ignored,
}

/// Preserve Noise receive-counter order when pushes arrive out of order.
///
/// Noise transport nonces are implicit, unlike the cleartext `nonce` field in
/// the edge envelope. A future nonce therefore cannot be decrypted yet: retain
/// it until all preceding pushes have been authenticated and processed.
fn parse_buffered_encrypted_push(
    session: &Arc<Mutex<CryptoSession>>,
    user_uuid_slot: &Arc<Mutex<Option<Uuid>>>,
    buffer: &Arc<Mutex<BTreeMap<u32, Value>>>,
    msg: &Value,
) -> Result<Vec<DecodedPush>, GodarkError> {
    let nonce = json_u64(msg, "nonce")
        .ok_or_else(|| GodarkError::Encryption("missing nonce".into()))? as u32;
    let expected = session
        .lock()
        .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
        .recv_nonce();
    if nonce < expected {
        return Err(GodarkError::Encryption(format!(
            "stale encrypted push nonce {nonce}; expected {expected}"
        )));
    }
    if nonce > expected {
        let mut buffered = buffer
            .lock()
            .map_err(|_| GodarkError::Session("Encrypted push buffer mutex poisoned".into()))?;
        if buffered.len() >= MAX_ENCRYPTED_PUSH_BUFFER && !buffered.contains_key(&nonce) {
            return Err(GodarkError::Encryption(format!(
                "encrypted push buffer limit ({MAX_ENCRYPTED_PUSH_BUFFER}) reached"
            )));
        }
        buffered.insert(nonce, msg.clone());
        return Ok(Vec::new());
    }

    let mut decoded = vec![parse_encrypted_push(session, user_uuid_slot, msg)?];
    loop {
        let next_nonce = session
            .lock()
            .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
            .recv_nonce();
        let next = buffer
            .lock()
            .map_err(|_| GodarkError::Session("Encrypted push buffer mutex poisoned".into()))?
            .remove(&next_nonce);
        let Some(next) = next else {
            break;
        };
        decoded.push(parse_encrypted_push(session, user_uuid_slot, &next)?);
    }
    Ok(decoded)
}

fn parse_encrypted_push(
    session: &Arc<Mutex<CryptoSession>>,
    user_uuid_slot: &Arc<Mutex<Option<Uuid>>>,
    msg: &Value,
) -> Result<DecodedPush, GodarkError> {
    let ct_b64 = msg
        .get("encrypted_body")
        .and_then(|v| v.as_str())
        .ok_or_else(|| GodarkError::Encryption("missing encrypted_body".into()))?;
    let ct = BASE64
        .decode(ct_b64)
        .map_err(|e| GodarkError::Encryption(format!("base64 decode: {e}")))?;
    let nonce = json_u64(msg, "nonce")
        .ok_or_else(|| GodarkError::Encryption("missing nonce".into()))? as u32;
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

    let aad = proto_bridge::build_response_header_aad(
        &user_uuid_bytes,
        message_type,
        ct.len() as u32,
        nonce as u64,
        fencing_epoch,
        &response_correlation_id_bytes(msg),
        json_u64(msg, "session_seq").unwrap_or_default(),
        session
            .lock()
            .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
            .conn_id()
            .unwrap_or_default(),
    );

    let plaintext = session
        .lock()
        .map_err(|_| GodarkError::Session("Session mutex poisoned".into()))?
        .decrypt_push(nonce, &aad, &ct)
        .map_err(|e| GodarkError::Encryption(format!("Failed to decrypt push: {e}")))?;

    // A late command ack may be routed through the event stream after its
    // caller has already received the matching response. It still consumes a
    // Noise transport nonce, so authenticate it before dropping it.
    if message_type.ends_with("ack") {
        return Ok(DecodedPush::Ignored);
    }

    match proto_bridge::parse_sequencer_to_edge_message(&plaintext)? {
        EdgeMessage::OrderUpdate(update) => Ok(DecodedPush::Order(update)),
        EdgeMessage::PositionUpdate(update) => Ok(DecodedPush::Position(update)),
        EdgeMessage::PositionsSnapshot(snap) => Ok(DecodedPush::PositionsSnapshot(snap)),
        EdgeMessage::SystemHealth(h) => Ok(DecodedPush::SystemHealth(h)),
        EdgeMessage::BalanceUpdate(b) => Ok(DecodedPush::Balance(b)),
        EdgeMessage::MarginAlert(a) => Ok(DecodedPush::MarginAlert(a)),
        EdgeMessage::FundingRateUpdate(f) => Ok(DecodedPush::FundingRate(f)),
        EdgeMessage::SettlementUpdate(s) => Ok(DecodedPush::Settlement(s)),
        // Parsed but not yet routed to a consumer channel.
        EdgeMessage::AccountMarginUpdate(_) => Ok(DecodedPush::Ignored),
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

fn parse_cleartext_position_update(msg: &Value) -> Option<PositionUpdate> {
    Some(PositionUpdate {
        user_uuid: json_uuid(msg),
        symbol_id: json_u64(msg, "symbol_id").unwrap_or_default(),
        side: parse_side(msg.get("side").and_then(Value::as_str).unwrap_or("BUY")),
        update_type: parse_position_update_type(
            msg.get("update_type")
                .and_then(Value::as_str)
                .unwrap_or("SNAPSHOT"),
        ),
        size: json_string(msg, "size", "0"),
        entry_price: json_string(msg, "entry_price", "0"),
        previous_size: json_string(msg, "previous_size", "0"),
        fill_price: json_string(msg, "fill_price", "0"),
        fill_qty: json_string(msg, "fill_qty", "0"),
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

/// Extract user UUID bytes from a push JSON message for AAD construction.
#[allow(dead_code)]
fn parse_user_uuid_bytes(msg: &Value) -> Vec<u8> {
    if let Some(s) = msg
        .get("user_uuid")
        .or_else(|| msg.get("user_id"))
        .and_then(|v| v.as_str())
    {
        if let Ok(u) = Uuid::parse_str(s) {
            return u.as_bytes().to_vec();
        }
    }
    vec![0u8; 16]
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
        v.as_u64()
            .map(u128::from)
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
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

fn parse_position_update_type(raw: &str) -> PositionUpdateType {
    match raw {
        "OPEN" => PositionUpdateType::Open,
        "INCREASE" => PositionUpdateType::Increase,
        "DECREASE" => PositionUpdateType::Decrease,
        "CLOSE" => PositionUpdateType::Close,
        "FUNDING_APPLIED" => PositionUpdateType::FundingApplied,
        _ => PositionUpdateType::Snapshot,
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
        let mut client = GodarkClient::new(test_config());
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
    fn test_take_position_receiver() {
        let mut client = GodarkClient::new(test_config());
        assert!(client.take_position_receiver().is_some());
        assert!(client.take_position_receiver().is_none());
    }

    #[tokio::test]
    async fn test_place_order_when_disconnected() {
        let mut client = GodarkClient::new(test_config());
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
        let mut client = GodarkClient::new(test_config());
        let err = client
            .cancel_order("12345", "BTC-USDC-PERP")
            .await
            .unwrap_err();
        assert!(matches!(err, GodarkError::Connection(_)));
    }

    #[tokio::test]
    async fn test_modify_order_when_disconnected() {
        let mut client = GodarkClient::new(test_config());
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
    async fn test_start_event_loop_routes_position_updates() {
        let mut client = GodarkClient::new(test_config());
        let mut position_rx = client.take_position_receiver().expect("position receiver");
        let (event_tx, event_rx) = mpsc::channel(8);

        client.start_event_loop(event_rx);
        event_tx
            .send(TransportEvent::PositionUpdate(json!({
                "type": "position_update",
                "user_uuid": "00000000-0000-0000-0000-000000000007",
                "symbol_id": 1,
                "side": "BUY",
                "update_type": "INCREASE",
                "size": "2",
                "entry_price": "100",
                "previous_size": "1",
                "fill_price": "101",
                "fill_qty": "1",
                "correlation_id": "1234",
                "timestamp": 456
            })))
            .await
            .expect("send event");

        let update = tokio::time::timeout(Duration::from_millis(100), position_rx.recv())
            .await
            .expect("receive timeout")
            .expect("position update");

        assert_eq!(
            update.user_uuid,
            Uuid::parse_str("00000000-0000-0000-0000-000000000007").unwrap()
        );
        assert_eq!(update.symbol_id, 1);
        assert_eq!(update.side, Side::Buy);
        assert_eq!(update.update_type, PositionUpdateType::Increase);
        assert_eq!(update.correlation_id, 1234);
        assert_eq!(update.timestamp, 456);
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
