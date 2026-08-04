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
                        .unwrap_or(Value::Bool(false))
                })
            } else {
                serde_json::json!({
                    "type": "auth_result",
                    "success": false,
                    "error": "invalid auth response"
                })
            }
        }
        "noise.handshake" | "noise_handshake" => {
            if code != 0 {
                serde_json::json!({
                    "type": "error",
                    "message": msg_str.unwrap_or("noise handshake failed")
                })
            } else if let Some(d) = data.and_then(|v| v.as_object()) {
                serde_json::json!({
                    "type": "noise_handshake_reply",
                    "conn_id": d.get("conn_id"),
                    "message": d.get("message").cloned().unwrap_or(Value::String(String::new())),
                    "established": d.get("established").cloned().unwrap_or(Value::Bool(false))
                })
            } else {
                serde_json::json!({ "type": "error", "message": "invalid noise handshake response" })
            }
        }
        "session.setup" | "session_setup" => {
            if code != 0 {
                serde_json::json!({
                    "type": "error",
                    "message": msg_str.unwrap_or("session setup failed")
                })
            } else if let Some(d) = data.and_then(|v| v.as_object()) {
                let seq_pk = d
                    .get("sequencer_ecdh_pubkey")
                    .and_then(|v| v.as_str())
                    .or_else(|| d.get("server_ecdh_pubkey").and_then(|v| v.as_str()))
                    .unwrap_or("");
                serde_json::json!({
                    "type": "session_established",
                    "sequencer_ecdh_pubkey": seq_pk,
                    "session_id": d.get("session_id")
                })
            } else {
                serde_json::json!({ "type": "error", "message": "invalid session response" })
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
        | "order.batch_cancel" | "order.batch_modify" | "order.spline_place"
        | "order.spline_anchor_update" => {
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
                        "session_seq": d.get("session_seq").cloned().unwrap_or(Value::Null)
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
                        "encrypted_body": d.get("ciphertext").or_else(|| d.get("encrypted_body")),
                        "nonce": d.get("nonce").cloned().unwrap_or(Value::from(0u64)),
                        "fencing_epoch": d.get("fencing_epoch").cloned().unwrap_or(Value::from(0u64)),
                        "correlation_id": d.get("correlation_id").cloned().unwrap_or(Value::Null),
                        "session_seq": d.get("session_seq").cloned().unwrap_or(Value::Null)
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
    SessionEstablished(Value),
    RekeyRequired(Value),
    OrderUpdate(Value),
    PositionUpdate(Value),
    EncryptedPush(Value),
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

    pub fn use_docs_wire(&self) -> bool {
        self.transport.use_docs_wire
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

        let last_inbound = Arc::new(Mutex::new(Some(Instant::now())));
        let write_handle = tokio::spawn(Self::write_loop(ws_write, write_rx));
        let recv_last = Arc::clone(&last_inbound);
        let recv_pending_sub = Arc::clone(&self.pending_sub);
        let recv_handle = tokio::spawn(Self::recv_loop(
            ws_read,
            event_tx.clone(),
            cmd_rx,
            session_rx,
            recv_pending_sub,
            recv_last,
        ));

        let hb_write_tx = write_tx.clone();
        let hb_event_tx = event_tx;
        let heartbeat_interval = self.transport.heartbeat_interval;
        let stale_timeout = self.transport.stale_timeout;
        let use_docs = self.transport.use_docs_wire;
        let heartbeat_handle = tokio::spawn(Self::heartbeat_loop(
            hb_write_tx,
            hb_event_tx,
            last_inbound,
            heartbeat_interval,
            stale_timeout,
            use_docs,
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
        if self.transport.use_docs_wire {
            payload["id"] = serde_json::json!(Uuid::new_v4().to_string());
        }

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
        let payload = if self.transport.use_docs_wire {
            serde_json::json!({
                "id": Uuid::new_v4().to_string(),
                "op": "login",
                "args": { "token": token }
            })
        } else {
            serde_json::json!({
                "type": "auth",
                "data": { "token": token }
            })
        };
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

    pub async fn send_session_setup(&self, payload: &Value) -> Result<Value, GodarkError> {
        let (tx, rx) = oneshot::channel();
        let session = PendingSession { tx };
        self.session_tx
            .as_ref()
            .ok_or_else(|| GodarkError::Connection("Not connected".into()))?
            .send(session)
            .await
            .map_err(|_| GodarkError::Connection("Session channel closed".into()))?;
        self.send_json(payload).await?;

        let cmd_to = self.transport.command_timeout;
        tokio::time::timeout(cmd_to, rx)
            .await
            .map_err(|_| GodarkError::Timeout("Session setup timed out".into()))?
            .map_err(|_| GodarkError::Connection("Session setup cancelled".into()))
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
        last_inbound: Arc<Mutex<Option<Instant>>>,
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
                    let Message::Text(ref text) = msg else { continue };
                    let text = text.as_ref();
                    let Ok(val) = serde_json::from_str::<Value>(text) else { continue };

                    if let Ok(mut g) = last_inbound.lock() {
                        *g = Some(Instant::now());
                    }

                    let val = normalize_inbound_value(&val);
                    Self::dispatch(
                        &val,
                        &event_tx,
                        &mut pending_cmd,
                        &mut pending_session,
                        &pending_sub,
                    ).await;
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
        pending_session: &mut Option<PendingSession>,
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
            "session_established" => {
                if let Some(session) = pending_session.take() {
                    let _ = session.tx.send(val.clone());
                } else {
                    let _ = event_tx
                        .send(TransportEvent::SessionEstablished(val.clone()))
                        .await;
                }
            }
            "noise_handshake_reply" => {
                if let Some(cmd) = pending_cmd.take() {
                    let _ = cmd.tx.send(val.clone());
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
            "position_update" => {
                let _ = event_tx
                    .send(TransportEvent::PositionUpdate(val.clone()))
                    .await;
            }
            "encrypted_push" => {
                let sub_type = val
                    .get("message_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if matches!(
                    sub_type,
                    "ack" | "mass_quote_ack" | "batch_cancel_ack" | "batch_modify_ack"
                        | "spline_order_ack"
                ) {
                    if let Some(cmd) = pending_cmd.take() {
                        let _ = cmd.tx.send(val.clone());
                    } else {
                        let _ = event_tx
                            .send(TransportEvent::EncryptedPush(val.clone()))
                            .await;
                    }
                } else {
                    let _ = event_tx
                        .send(TransportEvent::EncryptedPush(val.clone()))
                        .await;
                }
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
        last_inbound: Arc<Mutex<Option<Instant>>>,
        heartbeat_interval: Duration,
        stale_timeout: Duration,
        use_docs_wire: bool,
    ) {
        loop {
            tokio::time::sleep(heartbeat_interval).await;
            let stale = {
                let guard = last_inbound.lock().ok();
                guard
                    .and_then(|g| *g)
                    .map(|t| t.elapsed() > stale_timeout)
                    .unwrap_or(true)
            };
            if stale {
                tracing::warn!("Stale connection (no inbound within {stale_timeout:?}), closing");
                let _ = event_tx.send(TransportEvent::Disconnected).await;
                break;
            }
            let ping = if use_docs_wire {
                serde_json::json!({
                    "id": Uuid::new_v4().to_string(),
                    "op": "ping",
                    "args": serde_json::json!({})
                })
            } else {
                serde_json::json!({"type": "ping"})
            };
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
    fn test_docs_encrypted_spline_preserves_response_aad_fields() {
        let normalized = normalize_inbound_value(&json!({
            "id": "request-2",
            "op": "order.spline_place",
            "code": 0,
            "data": {
                "message_type": "spline_order_ack",
                "ciphertext": "AAAA",
                "nonce": 7,
                "fencing_epoch": 3,
                "correlation_id": "1339673755198158349044581307228491536",
                "session_seq": 42
            }
        }));
        assert_eq!(normalized["type"], "encrypted_push");
        assert_eq!(normalized["message_type"], "spline_order_ack");
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
    async fn test_dispatch_session_established_sends_event() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val =
            json!({"type":"session_established","sequencer_ecdh_pubkey":"abc","session_id":1});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        match event_rx.recv().await {
            Some(TransportEvent::SessionEstablished(v)) => assert_eq!(v, val),
            other => panic!("expected SessionEstablished, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_dispatch_session_established_resolves_pending_session() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let (session_tx, session_rx) = tokio::sync::oneshot::channel();
        let mut pending_session = Some(PendingSession { tx: session_tx });
        let pending_sub = make_sub_slot(None);

        let val =
            json!({"type":"session_established","sequencer_ecdh_pubkey":"abc","session_id":1});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        assert!(event_rx.try_recv().is_err());
        let received = session_rx
            .await
            .expect("session_established should resolve pending session");
        assert_eq!(received, val);
        assert!(pending_session.is_none());
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
    async fn test_dispatch_position_update_sends_event() {
        let (event_tx, mut event_rx) = tokio::sync::mpsc::channel(16);
        let mut pending_cmd = None;
        let mut pending_session = None;
        let pending_sub = make_sub_slot(None);

        let val = json!({"type":"position_update","symbol":"BTC","size":"1"});
        EdgeTransport::dispatch(
            &val,
            &event_tx,
            &mut pending_cmd,
            &mut pending_session,
            &pending_sub,
        )
        .await;

        match event_rx.recv().await {
            Some(TransportEvent::PositionUpdate(v)) => assert_eq!(v, val),
            other => panic!("expected PositionUpdate, got {other:?}"),
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

        assert!(
            event_rx.try_recv().is_err(),
            "ack push must not go to event channel"
        );
        let received = cmd_rx
            .await
            .expect("encrypted_push ack should resolve command");
        assert_eq!(received, val);
        assert!(pending_cmd.is_none());
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
}
