// WebSocket transport for gdx-edge — mirrors Python SDK _transport.py

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use crate::config::TransportConfig;
use crate::error::GodarkError;
use crate::heartbeat::HeartbeatTracker;
use crate::wire::{self, DecodedBinary};

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn is_docs_reply(val: &Value) -> bool {
    if val.get("type").is_some() {
        return false;
    }
    val.get("op").and_then(|v| v.as_str()).is_some()
        && val.get("code").and_then(|v| v.as_i64()).is_some()
}

/// Map gdx-edge `{id, op, code, data?, message?}` replies to legacy `type` / `event` JSON.
fn normalize_inbound_value(val: &Value) -> Value {
    if !is_docs_reply(val) {
        return val.clone();
    }
    let code = val.get("code").and_then(|v| v.as_i64()).unwrap_or(0);
    let op = val.get("op").and_then(|v| v.as_str()).unwrap_or("");
    let data = val.get("data");
    let msg_str = val.get("message").and_then(|v| v.as_str());

    match op {
        "pong" if code == 0 => serde_json::json!({ "type": "pong" }),
        "login" => {
            if code != 0 {
                serde_json::json!({
                    "type": "auth_result",
                    "success": false,
                    "error": msg_str.unwrap_or("authentication failed")
                })
            } else if let Some(d) = data.and_then(|v| v.as_object()) {
                serde_json::json!({
                    "type": "auth_result",
                    "success": true,
                    "user_uuid": d.get("user_uuid"),
                    "account_id": d.get("account_id"),
                    "session_id": d.get("session_id"),
                    "token_expires_at": d.get("token_expires_at"),
                    "cancel_on_disconnect": d
                        .get("cancel_on_disconnect")
                        .cloned()
                        .unwrap_or(Value::Bool(false)),
                    "conn_id": d.get("conn_id").cloned().unwrap_or(Value::Null),
                })
            } else {
                serde_json::json!({
                    "type": "auth_result",
                    "success": false,
                    "error": "invalid auth response"
                })
            }
        }
        "subscribe" | "unsubscribe" => {
            if code != 0 {
                let ch = data
                    .and_then(|v| v.as_object())
                    .and_then(|d| d.get("channel"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                serde_json::json!({
                    "event": "error",
                    "message": msg_str.unwrap_or("channel error"),
                    "channel": ch
                })
            } else if let Some(d) = data.and_then(|v| v.as_object()) {
                if d.contains_key("channel") {
                    serde_json::json!({ "event": op, "channel": d.get("channel") })
                } else {
                    serde_json::json!({ "event": op })
                }
            } else {
                serde_json::json!({ "event": op })
            }
        }
        "logout" => {
            if code != 0 {
                serde_json::json!({ "type": "error", "message": msg_str.unwrap_or("logout failed") })
            } else {
                serde_json::json!({ "type": "ack", "success": true })
            }
        }
        "order.place" | "order.cancel" | "order.modify" | "order.mass_quote"
        | "order.batch_cancel" | "order.batch_modify" => {
            if code != 0 {
                serde_json::json!({ "type": "error", "message": msg_str.unwrap_or("order error") })
            } else if let Some(d) = data.and_then(|v| v.as_object()) {
                if d.get("message_type").is_some()
                    && (d.contains_key("ciphertext") || d.contains_key("encrypted_body"))
                {
                    serde_json::json!({
                        "type": "encrypted_push",
                        "message_type": d.get("message_type"),
                        "encrypted_body": d
                            .get("ciphertext")
                            .or_else(|| d.get("encrypted_body"))
                            .cloned()
                            .unwrap_or(Value::Null),
                        "nonce": d.get("nonce").cloned().unwrap_or(Value::from(0u64)),
                        "fencing_epoch": d.get("fencing_epoch").cloned().unwrap_or(Value::from(0u64)),
                        "correlation_id": d.get("correlation_id").cloned().unwrap_or(Value::Null),
                        "session_seq": d.get("session_seq").cloned().unwrap_or(Value::Null),
                        "conn_id": d.get("conn_id").cloned().unwrap_or(Value::Null),
                    })
                } else {
                    serde_json::json!({
                        "type": "ack",
                        "success": d.get("success").cloned().unwrap_or(Value::Bool(true)),
                        "order_id": d.get("order_id"),
                        "sequence": d.get("sequence"),
                        "error": d.get("error"),
                        "error_code": d.get("error_code")
                    })
                }
            } else {
                serde_json::json!({ "type": "error", "message": "invalid order response" })
            }
        }
        _ => {
            if let Some(d) = data.and_then(|v| v.as_object()) {
                if d.get("event").and_then(|v| v.as_str()) == Some("rekey_required") {
                    return serde_json::json!({
                        "type": "rekey_required",
                        "session_id": d.get("session_id")
                    });
                }
                if d.get("message_type").is_some()
                    && (d.contains_key("ciphertext") || d.contains_key("encrypted_body"))
                {
                    return serde_json::json!({
                        "type": "encrypted_push",
                        "message_type": d.get("message_type"),
                        "encrypted_body": d
                            .get("ciphertext")
                            .or_else(|| d.get("encrypted_body"))
                            .cloned()
                            .unwrap_or(Value::Null),
                        "nonce": d.get("nonce").cloned().unwrap_or(Value::from(0u64)),
                        "fencing_epoch": d.get("fencing_epoch").cloned().unwrap_or(Value::from(0u64)),
                        "correlation_id": d.get("correlation_id").cloned().unwrap_or(Value::Null),
                        "session_seq": d.get("session_seq").cloned().unwrap_or(Value::Null),
                        "conn_id": d.get("conn_id").cloned().unwrap_or(Value::Null),
                    });
                }
            }
            val.clone()
        }
    }
}

/// Internal message types sent from the recv loop to the owner.
#[derive(Debug)]
pub enum TransportEvent {
    AuthResult(Value),
    RekeyRequired(Value),
    OrderUpdate(Value),
    EncryptedPush(Value),
    PublicMessage(Value),
    HpkeSetupReply {
        conn_id: u64,
        established: bool,
    },
    /// Stale heartbeat detected; reason is surfaced on the client error channel before close.
    StaleDisconnect {
        reason: String,
    },
    Disconnected,
}

/// Pending command awaiting a response.
struct PendingCommand {
    tx: oneshot::Sender<Value>,
}

/// Pending session-setup awaiting a session_established response.
struct PendingSession {
    tx: oneshot::Sender<Value>,
}

/// Pending subscription awaiting N channel acks.
struct PendingSubscription {
    remaining: usize,
    op: String,
    tx: oneshot::Sender<Result<(), GodarkError>>,
}

pub struct EdgeTransport {
    url: String,
    transport: TransportConfig,
    write_tx: Option<mpsc::Sender<Message>>,
    event_rx: Option<mpsc::Receiver<TransportEvent>>,
    cmd_tx: Option<mpsc::Sender<PendingCommand>>,
    session_tx: Option<mpsc::Sender<PendingSession>>,
    // Subscription slot. Owned by the caller (send_subscribe), read by
    // recv_loop's dispatcher. Using a shared slot instead of an mpsc
    // channel guarantees the entry is registered BEFORE any wire send,
    // which is required: a fast `event="subscribe"` ack from the edge
    // can otherwise race ahead of the registration and be silently dropped.
    pending_sub: Arc<Mutex<Option<PendingSubscription>>>,
    recv_handle: Option<JoinHandle<()>>,
    heartbeat_handle: Option<JoinHandle<()>>,
    write_handle: Option<JoinHandle<()>>,
    connected: bool,
}

impl EdgeTransport {
    pub fn new(url: &str, transport: TransportConfig) -> Self {
        Self {
            url: url.to_string(),
            transport,
            write_tx: None,
            event_rx: None,
            cmd_tx: None,
            session_tx: None,
            pending_sub: Arc::new(Mutex::new(None)),
            recv_handle: None,
            heartbeat_handle: None,
            write_handle: None,
            connected: false,
        }
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<TransportEvent>> {
        self.event_rx.take()
    }

    pub async fn connect(&mut self) -> Result<(), GodarkError> {
        let ws_stream = crate::ws_connect::connect_websocket(&self.url, &self.transport).await?;

        let (ws_write, ws_read) = ws_stream.split();

        let (write_tx, write_rx) = mpsc::channel::<Message>(64);
        let (event_tx, event_rx) = mpsc::channel::<TransportEvent>(256);
        let (cmd_tx, cmd_rx) = mpsc::channel::<PendingCommand>(8);
        let (session_tx, session_rx) = mpsc::channel::<PendingSession>(4);

        let heartbeat_tracker = Arc::new(Mutex::new(HeartbeatTracker::new(Instant::now())));
        let write_handle = tokio::spawn(Self::write_loop(ws_write, write_rx));
        let recv_tracker = Arc::clone(&heartbeat_tracker);
        let recv_pending_sub = Arc::clone(&self.pending_sub);
        let recv_handle = tokio::spawn(Self::recv_loop(
            ws_read,
            event_tx.clone(),
            cmd_rx,
            session_rx,
            recv_pending_sub,
            recv_tracker,
        ));

        let hb_write_tx = write_tx.clone();
        let hb_event_tx = event_tx;
        let heartbeat_interval = self.transport.heartbeat_interval;
        let stale_timeout = self.transport.stale_timeout;
        let missed_heartbeat_limit = self.transport.missed_heartbeat_limit;
        let heartbeat_handle = tokio::spawn(Self::heartbeat_loop(
            hb_write_tx,
            hb_event_tx,
            heartbeat_tracker,
            heartbeat_interval,
            stale_timeout,
            missed_heartbeat_limit,
        ));

        self.write_tx = Some(write_tx);
        self.event_rx = Some(event_rx);
        self.cmd_tx = Some(cmd_tx);
        self.session_tx = Some(session_tx);
        self.recv_handle = Some(recv_handle);
        self.heartbeat_handle = Some(heartbeat_handle);
        self.write_handle = Some(write_handle);
        self.connected = true;

        tracing::info!("Connected to {}", self.url);
        Ok(())
    }

    pub async fn disconnect(&mut self) {
        self.connected = false;
        if let Some(h) = self.heartbeat_handle.take() {
            h.abort();
        }
        if let Some(h) = self.recv_handle.take() {
            h.abort();
        }
        if let Some(h) = self.write_handle.take() {
            h.abort();
        }
        self.write_tx = None;
        self.event_rx = None;
        self.cmd_tx = None;
        self.session_tx = None;
        // Drop any in-flight pending subscription so its awaiter wakes
        // with an error instead of hanging until the timeout.
        if let Ok(mut slot) = self.pending_sub.lock() {
            if let Some(sub) = slot.take() {
                let _ = sub
                    .tx
                    .send(Err(GodarkError::Connection("disconnected".into())));
            }
        }
        tracing::info!("Disconnected");
    }

    pub async fn send_json(&self, obj: &Value) -> Result<(), GodarkError> {
        let tx = self
            .write_tx
            .as_ref()
            .ok_or_else(|| GodarkError::Connection("Not connected".into()))?;
        let text = serde_json::to_string(obj)
            .map_err(|e| GodarkError::Connection(format!("JSON serialize: {e}")))?;
        tx.send(Message::Text(text.into()))
            .await
            .map_err(|_| GodarkError::Connection("Write channel closed".into()))
    }

    pub async fn send_binary(&self, bytes: Vec<u8>) -> Result<(), GodarkError> {
        let tx = self
            .write_tx
            .as_ref()
            .ok_or_else(|| GodarkError::Connection("Not connected".into()))?;
        tx.send(Message::Binary(bytes.into()))
            .await
            .map_err(|_| GodarkError::Connection("Write channel closed".into()))
    }

    pub async fn send_command(&self, payload: &Value) -> Result<Value, GodarkError> {
        let (tx, rx) = oneshot::channel();
        let cmd = PendingCommand { tx };
        self.cmd_tx
            .as_ref()
            .ok_or_else(|| GodarkError::Connection("Not connected".into()))?
            .send(cmd)
            .await
            .map_err(|_| GodarkError::Connection("Command channel closed".into()))?;
        self.send_json(payload).await?;

        let cmd_to = self.transport.command_timeout;
        tokio::time::timeout(cmd_to, rx)
            .await
            .map_err(|_| GodarkError::Timeout(format!("Command timed out after {cmd_to:?}")))?
            .map_err(|_| GodarkError::Connection("Command cancelled".into()))
    }

    pub async fn send_subscribe(&self, channels: &[String], op: &str) -> Result<(), GodarkError> {
        let args: Vec<Value> = channels
            .iter()
            .map(|c| serde_json::json!({ "channel": c }))
            .collect();
        let mut payload = serde_json::json!({ "op": op, "args": args });
        payload["id"] = serde_json::json!(Uuid::new_v4().to_string());

        // Synchronously register the PendingSubscription BEFORE sending the
        // wire frame. The recv loop reads from this same shared slot when
        // processing the edge's `event="subscribe"` ack. If we sent first
        // and registered after, a fast ack (as gdx-core PR #203 enabled)
        // would race ahead of the registration and be silently dropped,
        // causing this call to time out forever.
        let (tx, rx) = oneshot::channel();
        if !self.connected {
            return Err(GodarkError::Connection("Not connected".into()));
        }
        {
            let mut slot = self
                .pending_sub
                .lock()
                .map_err(|_| GodarkError::Connection("pending_sub poisoned".into()))?;
            if slot.is_some() {
                return Err(GodarkError::Connection(
                    "subscription already in flight".into(),
                ));
            }
            *slot = Some(PendingSubscription {
                remaining: channels.len(),
                op: op.to_string(),
                tx,
            });
        }

        if let Err(e) = self.send_json(&payload).await {
            // Wire send failed — clear the slot so the next subscribe
            // can register fresh.
            if let Ok(mut slot) = self.pending_sub.lock() {
                let _ = slot.take();
            }
            return Err(e);
        }

        let cmd_to = self.transport.command_timeout;
        match tokio::time::timeout(cmd_to, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                // oneshot Sender dropped (recv_loop exited).
                Err(GodarkError::Connection("Sub cancelled".into()))
            }
            Err(_) => {
                // Time-out: clean up the slot so a retry can register.
                if let Ok(mut slot) = self.pending_sub.lock() {
                    let _ = slot.take();
                }
                Err(GodarkError::Timeout(format!("{op} timed out")))
            }
        }
    }

    pub async fn authenticate(&self, token: &str) -> Result<Value, GodarkError> {
        let payload = serde_json::json!({
            "id": Uuid::new_v4().to_string(),
            "op": "login",
            "args": { "token": token }
        });
        let (tx, rx) = oneshot::channel();
        let cmd = PendingCommand { tx };
        self.cmd_tx
            .as_ref()
            .ok_or_else(|| GodarkError::Connection("Not connected".into()))?
            .send(cmd)
            .await
            .map_err(|_| GodarkError::Connection("Auth channel closed".into()))?;
        self.send_json(&payload).await?;

        let cmd_to = self.transport.command_timeout;
        tokio::time::timeout(cmd_to, rx)
            .await
            .map_err(|_| GodarkError::Timeout("Auth timed out".into()))?
            .map_err(|_| GodarkError::Connection("Auth cancelled".into()))
    }

    pub async fn send_hpke_setup(&self, frame: Vec<u8>) -> Result<Value, GodarkError> {
        let (tx, rx) = oneshot::channel();
        let session = PendingSession { tx };
        self.session_tx
            .as_ref()
            .ok_or_else(|| GodarkError::Connection("Not connected".into()))?
            .send(session)
            .await
            .map_err(|_| GodarkError::Connection("HPKE setup channel closed".into()))?;
        self.send_binary(frame).await?;

        let cmd_to = self.transport.command_timeout;
        tokio::time::timeout(cmd_to, rx)
            .await
            .map_err(|_| GodarkError::Timeout("HPKE setup timed out".into()))?
            .map_err(|_| GodarkError::Connection("HPKE setup cancelled".into()))
    }

    // --- Background tasks ---

    async fn write_loop(
        mut ws_write: futures_util::stream::SplitSink<WsStream, Message>,
        mut rx: mpsc::Receiver<Message>,
    ) {
        while let Some(msg) = rx.recv().await {
            if ws_write.send(msg).await.is_err() {
                break;
            }
        }
    }

    async fn recv_loop(
        mut ws_read: futures_util::stream::SplitStream<WsStream>,
        event_tx: mpsc::Sender<TransportEvent>,
        mut cmd_rx: mpsc::Receiver<PendingCommand>,
        mut session_rx: mpsc::Receiver<PendingSession>,
        pending_sub: Arc<Mutex<Option<PendingSubscription>>>,
        heartbeat_tracker: Arc<Mutex<HeartbeatTracker>>,
    ) {
        let mut pending_cmd: Option<PendingCommand> = None;
        let mut pending_session: Option<PendingSession> = None;

        loop {
            tokio::select! {
                biased;
                Some(cmd) = cmd_rx.recv() => {
                    pending_cmd = Some(cmd);
                }
                Some(session) = session_rx.recv() => {
                    pending_session = Some(session);
                }
                maybe_msg = ws_read.next() => {
                    let Some(result) = maybe_msg else { break };
                    let msg = match result {
                        Ok(m) => m,
                        Err(_) => break,
                    };
                    if let Ok(mut tracker) = heartbeat_tracker.lock() {
                        tracker.record_inbound(Instant::now());
                    }
                    match msg {
                        Message::Binary(bytes) => {
                            match wire::decode_binary_frame(&bytes) {
                                Ok(DecodedBinary::HpkeSetupReply { conn_id, established }) => {
                                    let val = serde_json::json!({
                                        "type": "hpke_setup_reply",
                                        "conn_id": conn_id,
                                        "established": established,
                                    });
                                    if let Some(session) = pending_session.take() {
                                        let _ = session.tx.send(val);
                                    } else {
                                        let _ = event_tx
                                            .send(TransportEvent::HpkeSetupReply { conn_id, established })
                                            .await;
                                    }
                                }
                                Ok(DecodedBinary::EncryptedPush(push)) => {
                                    if let Some(val) = wire::encrypted_push_to_json(&push) {
                                        let _ = event_tx
                                            .send(TransportEvent::EncryptedPush(val))
                                            .await;
                                    }
                                }
                                Ok(DecodedBinary::Ignored)
                                | Ok(DecodedBinary::EncryptedOrder(_))
                                | Ok(DecodedBinary::HpkeSetup(_)) => {}
                                Err(e) => {
                                    tracing::warn!("binary frame decode failed: {e}");
                                }
                            }
                        }
                        Message::Text(text) => {
                            let text_str: &str = text.as_ref();
                            let Ok(val) = serde_json::from_str::<Value>(text_str) else { continue };
                            let val = normalize_inbound_value(&val);
                            Self::dispatch(
                                &val,
                                &event_tx,
                                &mut pending_cmd,
                                &mut pending_session,
                                &pending_sub,
                            ).await;
                        }
                        _ => {}
                    }
                }
            }
        }

        if let Some(cmd) = pending_cmd.take() {
            let _ = cmd
                .tx
                .send(serde_json::json!({"type": "error", "message": "disconnected"}));
        }
        if let Some(session) = pending_session.take() {
            let _ = session
                .tx
                .send(serde_json::json!({"type": "error", "message": "disconnected"}));
        }
        if let Ok(mut slot) = pending_sub.lock() {
            if let Some(sub) = slot.take() {
                let _ = sub
                    .tx
                    .send(Err(GodarkError::Connection("disconnected".into())));
            }
        }
        let _ = event_tx.send(TransportEvent::Disconnected).await;
    }

    async fn dispatch(
        val: &Value,
        event_tx: &mpsc::Sender<TransportEvent>,
        pending_cmd: &mut Option<PendingCommand>,
        _pending_session: &mut Option<PendingSession>,
        pending_sub: &Arc<Mutex<Option<PendingSubscription>>>,
    ) {
        let msg_type = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let event = val.get("event").and_then(|v| v.as_str()).unwrap_or("");
        match msg_type {
            "pong" => {}
            "auth_result" => {
                if let Some(cmd) = pending_cmd.take() {
                    let _ = cmd.tx.send(val.clone());
                } else {
                    let _ = event_tx.send(TransportEvent::AuthResult(val.clone())).await;
                }
            }
            "rekey_required" => {
                let _ = event_tx
                    .send(TransportEvent::RekeyRequired(val.clone()))
                    .await;
            }
            "order_update" => {
                let _ = event_tx
                    .send(TransportEvent::OrderUpdate(val.clone()))
                    .await;
            }
            // Edge auto-fetches open orders on `orders` subscribe and pushes a
            // cleartext snapshot. Fan rows out as order_update-shaped events so
            // callers (and clear helpers) can cancel resting inventory.
            "open_orders_snapshot" => {
                if let Some(rows) = val.get("rows").and_then(|r| r.as_array()) {
                    for row in rows {
                        let mut update = row.clone();
                        if let Some(obj) = update.as_object_mut() {
                            obj.entry("type".to_string())
                                .or_insert_with(|| Value::String("order_update".into()));
                            obj.entry("message_type".to_string())
                                .or_insert_with(|| Value::String("OPEN".into()));
                        }
                        let _ = event_tx.send(TransportEvent::OrderUpdate(update)).await;
                    }
                }
            }
            "encrypted_push" => {
                // Always decrypt on the client event loop in WebSocket arrival
                // order (web parity). Command waiters are correlation-keyed and
                // resolved after decrypt — do not divert acks around that path.
                let _ = event_tx
                    .send(TransportEvent::EncryptedPush(val.clone()))
                    .await;
            }
            "funding_rate_snapshot" | "volume_snapshot" | "open_interest_snapshot" => {
                let _ = event_tx
                    .send(TransportEvent::PublicMessage(val.clone()))
                    .await;
            }
            "ack" | "error" => {
                if let Some(cmd) = pending_cmd.take() {
                    let _ = cmd.tx.send(val.clone());
                }
            }
            _ => {}
        }

        if event == "subscribe" || event == "unsubscribe" {
            if let Ok(mut slot) = pending_sub.lock() {
                let mut take_now = false;
                if let Some(sub) = slot.as_mut() {
                    if event == sub.op {
                        sub.remaining = sub.remaining.saturating_sub(1);
                        if sub.remaining == 0 {
                            take_now = true;
                        }
                    }
                }
                if take_now {
                    if let Some(sub) = slot.take() {
                        let _ = sub.tx.send(Ok(()));
                    }
                }
            }
        } else if event == "error" {
            if let Ok(mut slot) = pending_sub.lock() {
                if let Some(sub) = slot.take() {
                    let msg = val
                        .get("message")
                        .and_then(|v| v.as_str())
                        .unwrap_or("channel error");
                    let _ = sub.tx.send(Err(GodarkError::Connection(msg.to_string())));
                }
            }
        }
    }

    async fn heartbeat_loop(
        write_tx: mpsc::Sender<Message>,
        event_tx: mpsc::Sender<TransportEvent>,
        heartbeat_tracker: Arc<Mutex<HeartbeatTracker>>,
        heartbeat_interval: Duration,
        stale_timeout: Duration,
        missed_heartbeat_limit: u32,
    ) {
        loop {
            tokio::time::sleep(heartbeat_interval).await;
            let stale_reason = heartbeat_tracker.lock().ok().and_then(|mut tracker| {
                tracker
                    .on_tick(Instant::now(), stale_timeout, missed_heartbeat_limit)
                    .err()
            });
            if let Some(reason) = stale_reason {
                tracing::warn!("{reason}");
                let _ = event_tx
                    .send(TransportEvent::StaleDisconnect {
                        reason: reason.clone(),
                    })
                    .await;
                let _ = event_tx.send(TransportEvent::Disconnected).await;
                break;
            }
            let ping = serde_json::json!({
                "id": Uuid::new_v4().to_string(),
                "op": "ping",
                "args": serde_json::json!({})
            });
            let text = serde_json::to_string(&ping).unwrap();
            if write_tx.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use serde_json::json;

    use super::{
        normalize_inbound_value, EdgeTransport, PendingCommand, PendingSession,
        PendingSubscription, TransportEvent,
    };

    // Helper: wraps the pending-sub slot in an Arc<Mutex> the way the
    // production code does, so each test can keep its `Option`-style
    // ergonomics (and original assertions) without growing more lines.
    fn make_sub_slot(sub: Option<PendingSubscription>) -> Arc<Mutex<Option<PendingSubscription>>> {
        Arc::new(Mutex::new(sub))
    }

    #[test]
    fn test_docs_encrypted_batch_preserves_response_aad_fields() {
        let normalized = normalize_inbound_value(&json!({
            "id": "request-1",
            "op": "order.mass_quote",
            "code": 0,
            "data": {
                "message_type": "mass_quote_ack",
                "ciphertext": "AAAA",
                "nonce": 7,
                "fencing_epoch": 3,
                "correlation_id": "1339673755198158349044581307228491536",
                "session_seq": 42
            }
        }));
        assert_eq!(normalized["type"], "encrypted_push");
        assert_eq!(
            normalized["correlation_id"],
            "1339673755198158349044581307228491536"
        );
        assert_eq!(normalized["session_seq"], 42);
    }

    #[test]
    fn test_docs_batch_error_is_normalized() {
        let normalized = normalize_inbound_value(&json!({
            "id": "request-1",
            "op": "order.mass_quote",
            "code": 503,
            "message": "system temporarily unavailable, retry",
            "data": {}
        }));
        assert_eq!(normalized["type"], "error");
        assert_eq!(
            normalized["message"],
            "system temporarily unavailable, retry"
        );
    }

    #[tokio::test]
    async fn test_dispatch_pong_no_effect() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let (cmd_tx, cmd_rx) = tokio::sync::oneshot::channel();
        let mut pending_cmd = Some(PendingCommand { tx: cmd_tx });
        let mut pending_session: Option<PendingSession> = None;
        let pending_sub = make_sub_slot(None);

        let val = json!({"type": "pong"});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        assert!(event_rx.try_recv().is_err());
        let timeout_result = tokio::time::timeout(Duration::from_millis(50), cmd_rx).await;
        assert!(
            timeout_result.is_err(),
            "pong must not resolve pending command"
        );
    }

    #[tokio::test]
    async fn test_dispatch_auth_result_resolves_command() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let (cmd_tx, cmd_rx) = tokio::sync::oneshot::channel();
        let mut pending_cmd = Some(PendingCommand { tx: cmd_tx });
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val = json!({"type":"auth_result","success":true,"user_id":42});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        assert!(event_rx.try_recv().is_err());
        let received = cmd_rx.await.expect("auth_result should resolve command");
        assert_eq!(received, val);
    }

    #[tokio::test]
    async fn test_dispatch_rekey_required_sends_event() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val = json!({"type":"rekey_required"});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        match event_rx.recv().await {
            Some(TransportEvent::RekeyRequired(v)) => assert_eq!(v, val),
            other => panic!("expected RekeyRequired, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dispatch_order_update_sends_event() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val = json!({"type":"order_update","order_id":"ord-1","status":"open"});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        match event_rx.recv().await {
            Some(TransportEvent::OrderUpdate(v)) => assert_eq!(v, val),
            other => panic!("expected OrderUpdate, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dispatch_encrypted_push_sends_event() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val = json!({"type":"encrypted_push","ciphertext":"deadbeef"});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        match event_rx.recv().await {
            Some(TransportEvent::EncryptedPush(v)) => assert_eq!(v, val),
            other => panic!("expected EncryptedPush, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dispatch_ack_resolves_command() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let (cmd_tx, cmd_rx) = tokio::sync::oneshot::channel();
        let mut pending_cmd = Some(PendingCommand { tx: cmd_tx });
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val = json!({"type":"ack","success":true,"order_id":123});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        assert!(event_rx.try_recv().is_err());
        let received = cmd_rx.await.expect("ack should resolve command");
        assert_eq!(received, val);
    }

    #[tokio::test]
    async fn test_dispatch_error_resolves_command() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let (cmd_tx, cmd_rx) = tokio::sync::oneshot::channel();
        let mut pending_cmd = Some(PendingCommand { tx: cmd_tx });
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val = json!({"type":"error","message":"bad"});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        assert!(event_rx.try_recv().is_err());
        let received = cmd_rx.await.expect("error type should resolve command");
        assert_eq!(received, val);
    }

    #[tokio::test]
    async fn test_dispatch_subscribe_ack_counting() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let (sub_tx, mut sub_rx) = tokio::sync::oneshot::channel();
        let pending_sub = make_sub_slot(Some(PendingSubscription {
            remaining: 2,
            op: "subscribe".to_string(),
            tx: sub_tx,
        }));

        let msg1 = json!({"event":"subscribe"});
        EdgeTransport::dispatch(
            &msg1,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;
        assert!(sub_rx.try_recv().is_err());
        assert!(pending_sub.lock().unwrap().is_some());

        let msg2 = json!({"event":"subscribe"});
        EdgeTransport::dispatch(
            &msg2,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        assert!(event_rx.try_recv().is_err());
        let _ = sub_rx
            .await
            .expect("subscription completes after second ack");
        assert!(pending_sub.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn test_dispatch_event_error_rejects_sub() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let (sub_tx, sub_rx) = tokio::sync::oneshot::channel();
        let pending_sub = make_sub_slot(Some(PendingSubscription {
            remaining: 1,
            op: "subscribe".to_string(),
            tx: sub_tx,
        }));

        let val = json!({"event":"error","message":"bad channel"});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        assert!(event_rx.try_recv().is_err());
        let err = sub_rx
            .await
            .expect("sub channel should resolve")
            .unwrap_err();
        match err {
            crate::error::GodarkError::Connection(msg) => assert_eq!(msg, "bad channel"),
            e => panic!("expected Connection error, got {e:?}"),
        }
        assert!(pending_sub.lock().unwrap().is_none());
    }

    #[tokio::test]
    async fn test_dispatch_encrypted_push_ack_resolves_command() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let (cmd_tx, cmd_rx) = tokio::sync::oneshot::channel();
        let mut pending_cmd = Some(PendingCommand { tx: cmd_tx });
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val =
            json!({"type":"encrypted_push","message_type":"ack","encrypted_body":"abc","nonce":1});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        // Encrypted acks go to the event loop for ordered HPKE decrypt; the
        // transport pending_cmd slot is intentionally left for cleartext paths.
        match event_rx.recv().await {
            Some(TransportEvent::EncryptedPush(msg)) => assert_eq!(msg, val),
            other => panic!("expected EncryptedPush, got {other:?}"),
        }
        assert!(
            pending_cmd.is_some(),
            "encrypted ack must not take pending_cmd"
        );
        drop(cmd_rx);
    }

    #[tokio::test]
    async fn test_dispatch_encrypted_push_non_ack_sends_event() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val =
            json!({"type":"encrypted_push","message_type":"order_update","encrypted_body":"xyz"});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        match event_rx.recv().await {
            Some(TransportEvent::EncryptedPush(v)) => assert_eq!(v, val),
            other => panic!("expected EncryptedPush event, got {other:?}"),
        }
    }

    // Regression test for the gdx-rust-sdk subscribe race fixed alongside
    // gdx-core PR #203. We simulate the timing the original bug would have
    // hit: the sub slot is populated *before* the ack arrives, and the ack
    // must resolve the registration. (The pre-fix code wrote the slot via
    // an mpsc channel that recv_loop polled in a `select!` arm with no
    // ordering guarantee, so a fast inline ack could race ahead and be
    // dropped.) With the new design, the slot is populated synchronously
    // by send_subscribe before any wire send, so this test must always
    // pass.
    #[tokio::test]
    async fn test_dispatch_subscribe_ack_after_registration() {
        let (event_tx, _event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let (sub_tx, sub_rx) = tokio::sync::oneshot::channel();
        let pending_sub = make_sub_slot(Some(PendingSubscription {
            remaining: 1,
            op: "subscribe".to_string(),
            tx: sub_tx,
        }));

        let ack = json!({"event":"subscribe"});
        EdgeTransport::dispatch(
            &ack,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        let result = tokio::time::timeout(Duration::from_millis(200), sub_rx)
            .await
            .expect("ack must resolve subscriber within 200 ms")
            .expect("oneshot must not be dropped");
        assert!(result.is_ok());
        assert!(pending_sub.lock().unwrap().is_none());
    }

    #[test]
    fn test_heartbeat_tracker_single_miss_keeps_connection() {
        use std::time::Instant;

        use crate::heartbeat::HeartbeatTracker;

        let t0 = Instant::now();
        let mut tracker = HeartbeatTracker::new(t0);
        tracker.on_tick(t0, Duration::from_millis(120), 2).unwrap();
        let t1 = t0 + Duration::from_millis(30);
        assert!(tracker.on_tick(t1, Duration::from_millis(120), 2).is_ok());
    }

    #[test]
    fn test_heartbeat_tracker_second_miss_is_stale() {
        use std::time::Instant;

        use crate::heartbeat::HeartbeatTracker;

        let t0 = Instant::now();
        let mut tracker = HeartbeatTracker::new(t0);
        tracker.on_tick(t0, Duration::from_millis(120), 2).unwrap();
        let t1 = t0 + Duration::from_millis(30);
        tracker.on_tick(t1, Duration::from_millis(120), 2).unwrap();
        let t2 = t0 + Duration::from_millis(60);
        let err = tracker
            .on_tick(t2, Duration::from_millis(120), 2)
            .unwrap_err();
        assert!(err.contains("missed 2 heartbeat responses"));
    }

    #[tokio::test]
    async fn test_stale_disconnect_event_ordering() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(4);
        let reason = "stale heartbeat: missed 2 heartbeat responses (limit 2)".to_string();
        event_tx
            .send(TransportEvent::StaleDisconnect {
                reason: reason.clone(),
            })
            .await
            .unwrap();
        event_tx.send(TransportEvent::Disconnected).await.unwrap();

        match event_rx.recv().await {
            Some(TransportEvent::StaleDisconnect { reason: r }) => assert_eq!(r, reason),
            other => panic!("expected StaleDisconnect first, got {other:?}"),
        }
        match event_rx.recv().await {
            Some(TransportEvent::Disconnected) => {}
            other => panic!("expected Disconnected second, got {other:?}"),
        }
    }
}
