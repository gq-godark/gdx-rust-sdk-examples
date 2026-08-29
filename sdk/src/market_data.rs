// Market data WebSocket client — mirrors Java/Python MarketDataClient

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Mutex};
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};
use uuid::Uuid;

use crate::config::{self, TransportConfig};
use crate::error::GodarkError;
use crate::types::ReconnectEvent;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(30);
/// Maximum delay between reconnect attempts (matches trading client backoff cap).
pub(crate) const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(15);

const ORDERBOOK_DOCS_WIRE_MSG: &str =
    "L2 orderbook is not available on /ws/v1; set GODARK_MARKET_DATA_WS_URL to a direct L2 \
     stream URL, use subscribe_public_channel for public edge feeds, or \
     GODARK_MARKET_DATA_USE_GOMARKET=1 for local /ws/gomarket";

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Active I/O tasks for one WebSocket session; stopped when recv completes or on [`MarketDataClient::disconnect`].
struct ConnectionHandles {
    recv: JoinHandle<()>,
    write: JoinHandle<()>,
    heartbeat: JoinHandle<()>,
}

/// Shared state for reconnect and subscribe while disconnected.
struct MarketDataInner {
    url: String,
    docs_wire: bool,
    transport: TransportConfig,
    event_tx: mpsc::Sender<(String, Value)>,
    reconnect_tx: mpsc::Sender<ReconnectEvent>,
    write_tx: Arc<Mutex<Option<mpsc::Sender<Message>>>>,
    /// `(channel, symbol)` for symbol streams; `("__public__", channel)` for public edge feeds.
    desired_subs: Arc<Mutex<HashSet<(String, String)>>>,
    connected: Arc<AtomicBool>,
    auto_reconnect: Arc<AtomicBool>,
    intentional_close: Arc<AtomicBool>,
    reconnect_attempts: Arc<AtomicU32>,
    handles: Arc<Mutex<Option<ConnectionHandles>>>,
}

pub struct MarketDataClient {
    inner: Arc<MarketDataInner>,
    event_rx: Option<mpsc::Receiver<(String, Value)>>,
    reconnect_rx: Option<mpsc::Receiver<ReconnectEvent>>,
    supervisor: Option<JoinHandle<()>>,
}

fn reconnect_backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64 << attempt.min(4);
    Duration::from_secs(secs)
        .min(MAX_RECONNECT_DELAY)
        .max(Duration::from_secs(1))
}

/// Map a market-data JSON message to the `(channel, symbol)` key string used for events.
///
/// Data events use `type` of `orderbook` or `trade` (singular); subscriptions use `trades`.
/// Snapshot types map to `volume:`, `open_interest:`, `funding_rate:`.
pub fn subscription_callback_key(val: &Value) -> Option<String> {
    let typ = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
    match typ {
        "status" | "subscribed" | "unsubscribed" | "pong" | "error" => return None,
        _ => {}
    }
    let symbol = val.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
    match typ {
        "orderbook" => Some(format!("orderbook:{symbol}")),
        "trade" => Some(format!("trades:{symbol}")),
        "volume_snapshot" => Some("volume:".to_string()),
        "open_interest_snapshot" => Some("open_interest:".to_string()),
        "funding_rate_snapshot" => Some("funding_rate:".to_string()),
        _ => {
            if let Some(ch) = val.get("channel").and_then(|v| v.as_str()) {
                if !ch.is_empty() {
                    return Some(format!("{ch}:{symbol}"));
                }
            }
            None
        }
    }
}

/// Legacy key helper with a non-`None` fallback for arbitrary frames.
/// Retained for tests; the recv path now filters on [`subscription_callback_key`].
#[cfg(test)]
pub(crate) fn gomarket_event_key(val: &Value) -> String {
    subscription_callback_key(val).unwrap_or_else(|| {
        let typ = val.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let symbol = val.get("symbol").and_then(|v| v.as_str()).unwrap_or("");
        format!("{typ}:{symbol}")
    })
}

fn subscribe_payload(docs_wire: bool, channel: &str, symbol: &str) -> Value {
    if docs_wire {
        serde_json::json!({
            "id": Uuid::new_v4().to_string(),
            "op": "subscribe",
            "args": [{"channel": channel, "symbol": symbol}],
        })
    } else {
        serde_json::json!({
            "action": "subscribe",
            "channel": channel,
            "symbol": symbol,
        })
    }
}

fn unsubscribe_payload(docs_wire: bool, channel: &str, symbol: &str) -> Value {
    if docs_wire {
        let mut arg = serde_json::Map::new();
        arg.insert("channel".into(), Value::String(channel.to_string()));
        if !symbol.is_empty() {
            arg.insert("symbol".into(), Value::String(symbol.to_string()));
        }
        serde_json::json!({
            "id": Uuid::new_v4().to_string(),
            "op": "unsubscribe",
            "args": [Value::Object(arg)],
        })
    } else {
        serde_json::json!({
            "action": "unsubscribe",
            "channel": channel,
            "symbol": symbol,
        })
    }
}

fn public_subscribe_payload(channel: &str) -> Value {
    serde_json::json!({
        "id": Uuid::new_v4().to_string(),
        "op": "subscribe",
        "args": [{"channel": channel}],
    })
}

impl MarketDataClient {
    pub fn new(base_url: &str) -> Self {
        Self::with_transport(base_url, TransportConfig::default())
    }

    pub fn with_transport(base_url: &str, transport: TransportConfig) -> Self {
        let url = config::resolve_market_data_ws_url(base_url);
        let docs_wire = config::is_docs_wire_url(&url);
        let (event_tx, event_rx) = mpsc::channel(256);
        let (reconnect_tx, reconnect_rx) = mpsc::channel(256);
        Self {
            inner: Arc::new(MarketDataInner {
                url,
                docs_wire,
                transport,
                event_tx,
                reconnect_tx,
                write_tx: Arc::new(Mutex::new(None)),
                desired_subs: Arc::new(Mutex::new(HashSet::new())),
                connected: Arc::new(AtomicBool::new(false)),
                auto_reconnect: Arc::new(AtomicBool::new(true)),
                intentional_close: Arc::new(AtomicBool::new(false)),
                reconnect_attempts: Arc::new(AtomicU32::new(0)),
                handles: Arc::new(Mutex::new(None)),
            }),
            event_rx: Some(event_rx),
            reconnect_rx: Some(reconnect_rx),
            supervisor: None,
        }
    }

    /// WebSocket URL used for market data.
    pub fn endpoint_url(&self) -> &str {
        &self.inner.url
    }

    pub fn is_connected(&self) -> bool {
        self.inner.connected.load(Ordering::SeqCst)
    }

    /// When `true`, the client will reconnect after the socket drops (unless [`disconnect`](Self::disconnect) was called).
    pub fn auto_reconnect_enabled(&self) -> bool {
        self.inner.auto_reconnect.load(Ordering::SeqCst)
    }

    pub fn take_event_receiver(&mut self) -> Option<mpsc::Receiver<(String, Value)>> {
        self.event_rx.take()
    }

    pub fn take_reconnect_receiver(&mut self) -> Option<mpsc::Receiver<ReconnectEvent>> {
        self.reconnect_rx.take()
    }

    pub async fn connect(&mut self) -> Result<(), GodarkError> {
        self.inner.intentional_close.store(false, Ordering::SeqCst);
        self.inner.auto_reconnect.store(true, Ordering::SeqCst);
        if self.supervisor.is_some() {
            return Ok(());
        }

        let ws_stream =
            crate::ws_connect::connect_websocket(&self.inner.url, &self.inner.transport).await?;

        self.inner.reconnect_attempts.store(0, Ordering::SeqCst);
        Self::spawn_session(&self.inner, ws_stream).await?;

        let inner = Arc::clone(&self.inner);
        self.supervisor = Some(tokio::spawn(async move {
            MarketDataInner::supervisor_loop(inner).await;
        }));

        tracing::info!("Market data connected to {}", self.inner.url);
        Ok(())
    }

    async fn spawn_session(
        inner: &Arc<MarketDataInner>,
        ws_stream: WsStream,
    ) -> Result<(), GodarkError> {
        let (ws_write, ws_read) = ws_stream.split();
        let (write_tx, write_rx) = mpsc::channel::<Message>(64);
        {
            let mut slot = inner.write_tx.lock().await;
            *slot = Some(write_tx.clone());
        }

        let event_tx = inner.event_tx.clone();
        let write_handle = tokio::spawn(Self::write_loop(ws_write, write_rx));
        let recv_handle = tokio::spawn(Self::recv_loop(ws_read, event_tx));
        let hb_write_tx = write_tx.clone();
        let docs_wire = inner.docs_wire;
        let heartbeat_handle = tokio::spawn(Self::heartbeat_loop(hb_write_tx, docs_wire));

        inner.connected.store(true, Ordering::SeqCst);
        let handles = ConnectionHandles {
            recv: recv_handle,
            write: write_handle,
            heartbeat: heartbeat_handle,
        };
        let mut guard = inner.handles.lock().await;
        *guard = Some(handles);

        Self::replay_subscriptions(&write_tx, &inner.desired_subs, inner.docs_wire).await;
        Ok(())
    }

    async fn replay_subscriptions(
        write_tx: &mpsc::Sender<Message>,
        desired_subs: &Arc<Mutex<HashSet<(String, String)>>>,
        docs_wire: bool,
    ) {
        let subs = desired_subs.lock().await.clone();
        for (channel, symbol) in subs {
            let payload = if channel == "__public__" {
                public_subscribe_payload(&symbol)
            } else {
                subscribe_payload(docs_wire, &channel, &symbol)
            };
            if let Ok(text) = serde_json::to_string(&payload) {
                let _ = write_tx.send(Message::Text(text.into())).await;
            }
        }
    }

    pub async fn disconnect(&mut self) {
        self.inner.intentional_close.store(true, Ordering::SeqCst);
        self.inner.auto_reconnect.store(false, Ordering::SeqCst);
        self.inner.connected.store(false, Ordering::SeqCst);
        if let Some(h) = self.supervisor.take() {
            h.abort();
        }
        {
            let mut slot = self.inner.write_tx.lock().await;
            *slot = None;
        }
        if let Some(h) = self.inner.handles.lock().await.take() {
            h.heartbeat.abort();
            h.recv.abort();
            h.write.abort();
        }
    }

    pub async fn subscribe_orderbook(&mut self, symbol: &str) -> Result<(), GodarkError> {
        if self.inner.docs_wire {
            return Err(GodarkError::InvalidInput(ORDERBOOK_DOCS_WIRE_MSG.into()));
        }
        self.subscribe_channel("orderbook", symbol).await
    }

    pub async fn subscribe_trades(&mut self, symbol: &str) -> Result<(), GodarkError> {
        self.subscribe_channel("trades", symbol).await
    }

    /// Public edge channel on `/ws/v1` (no auth): `volume`, `open_interest`, `funding_rate`.
    pub async fn subscribe_public_channel(&mut self, channel: &str) -> Result<(), GodarkError> {
        if channel.is_empty() {
            return Err(GodarkError::InvalidInput("channel is required".into()));
        }
        if !self.inner.docs_wire {
            return Err(GodarkError::InvalidInput(
                "subscribe_public_channel requires /ws/v1 edge URL".into(),
            ));
        }
        self.inner
            .desired_subs
            .lock()
            .await
            .insert(("__public__".to_string(), channel.to_string()));
        if self.inner.connected.load(Ordering::SeqCst) {
            let payload = public_subscribe_payload(channel);
            self.send_json(&payload).await?;
        }
        Ok(())
    }

    pub async fn unsubscribe(&mut self, channel: &str, symbol: &str) -> Result<(), GodarkError> {
        {
            let mut desired = self.inner.desired_subs.lock().await;
            desired.remove(&(channel.to_string(), symbol.to_string()));
            desired.remove(&("__public__".to_string(), channel.to_string()));
        }
        if self.inner.connected.load(Ordering::SeqCst) {
            let payload = unsubscribe_payload(self.inner.docs_wire, channel, symbol);
            self.send_json(&payload).await?;
        }
        Ok(())
    }

    async fn subscribe_channel(&mut self, channel: &str, symbol: &str) -> Result<(), GodarkError> {
        self.inner
            .desired_subs
            .lock()
            .await
            .insert((channel.to_string(), symbol.to_string()));
        if self.inner.connected.load(Ordering::SeqCst) {
            let payload = subscribe_payload(self.inner.docs_wire, channel, symbol);
            self.send_json(&payload).await?;
        }
        Ok(())
    }

    async fn send_json(&self, val: &Value) -> Result<(), GodarkError> {
        let tx = {
            let guard = self.inner.write_tx.lock().await;
            guard.as_ref().cloned()
        };
        let tx = tx.ok_or_else(|| GodarkError::Connection("Not connected".into()))?;
        let text = serde_json::to_string(val)
            .map_err(|e| GodarkError::Connection(format!("JSON: {e}")))?;
        tx.send(Message::Text(text.into()))
            .await
            .map_err(|_| GodarkError::Connection("Write channel closed".into()))
    }

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
        event_tx: mpsc::Sender<(String, Value)>,
    ) {
        while let Some(result) = ws_read.next().await {
            let msg = match result {
                Ok(m) => m,
                Err(_) => break,
            };
            let Message::Text(ref text) = msg else {
                continue;
            };
            let Ok(val) = serde_json::from_str::<Value>(text.as_ref()) else {
                continue;
            };
            // Only forward resolved data/snapshot frames (orderbook, trades,
            // volume/open_interest/funding_rate snapshots). Control frames
            // (status/subscribed/pong/error) are dropped so consumers receive
            // only actionable events — matches the JS/Python/Go/Java/C++ SDKs.
            let Some(key) = subscription_callback_key(&val) else {
                continue;
            };
            let _ = event_tx.send((key, val)).await;
        }
    }

    async fn heartbeat_loop(write_tx: mpsc::Sender<Message>, docs_wire: bool) {
        loop {
            tokio::time::sleep(HEARTBEAT_INTERVAL).await;
            let ping = if docs_wire {
                serde_json::json!({"id": Uuid::new_v4().to_string(), "op": "ping"})
            } else {
                serde_json::json!({"action": "ping"})
            };
            let text = serde_json::to_string(&ping).unwrap();
            if write_tx.send(Message::Text(text.into())).await.is_err() {
                break;
            }
        }
    }
}

impl MarketDataInner {
    async fn supervisor_loop(inner: Arc<MarketDataInner>) {
        loop {
            let handles = {
                let mut guard = inner.handles.lock().await;
                guard.take()
            };
            let Some(h) = handles else {
                break;
            };

            let recv_h = h.recv;
            let _ = recv_h.await;

            h.heartbeat.abort();
            h.write.abort();

            {
                let mut slot = inner.write_tx.lock().await;
                *slot = None;
            }
            inner.connected.store(false, Ordering::SeqCst);
            let _ = inner.reconnect_tx.send(ReconnectEvent::Disconnected).await;

            if !inner.auto_reconnect.load(Ordering::SeqCst)
                || inner.intentional_close.load(Ordering::SeqCst)
            {
                break;
            }

            // Reconnect until success or user stops / auto_reconnect off.
            loop {
                let prev = inner.reconnect_attempts.fetch_add(1, Ordering::SeqCst);
                let delay = reconnect_backoff_delay(prev);
                let _ = inner
                    .reconnect_tx
                    .send(ReconnectEvent::Attempting {
                        attempt: prev.saturating_add(1),
                        delay,
                    })
                    .await;
                tracing::warn!("Market data disconnected; reconnecting in {:?}", delay);
                tokio::time::sleep(delay).await;

                if inner.intentional_close.load(Ordering::SeqCst)
                    || !inner.auto_reconnect.load(Ordering::SeqCst)
                {
                    return;
                }

                let ws_result =
                    crate::ws_connect::connect_websocket(&inner.url, &inner.transport).await;
                let ws_stream = match ws_result {
                    Ok(w) => w,
                    Err(e) => {
                        let _ = inner
                            .reconnect_tx
                            .send(ReconnectEvent::Failed {
                                error: format!("Market data reconnect failed: {e}"),
                            })
                            .await;
                        tracing::warn!("Market data reconnect failed: {e}");
                        if !inner.auto_reconnect.load(Ordering::SeqCst)
                            || inner.intentional_close.load(Ordering::SeqCst)
                        {
                            return;
                        }
                        continue;
                    }
                };

                inner.reconnect_attempts.store(0, Ordering::SeqCst);
                match MarketDataClient::spawn_session(&inner, ws_stream).await {
                    Ok(()) => {
                        let _ = inner.reconnect_tx.send(ReconnectEvent::Reconnected).await;
                        tracing::info!("Market data reconnected to {}", inner.url);
                        break;
                    }
                    Err(err) => {
                        let _ = inner
                            .reconnect_tx
                            .send(ReconnectEvent::Failed {
                                error: err.to_string(),
                            })
                            .await;
                        if !inner.auto_reconnect.load(Ordering::SeqCst)
                            || inner.intentional_close.load(Ordering::SeqCst)
                        {
                            return;
                        }
                        continue;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        gomarket_event_key, reconnect_backoff_delay, subscription_callback_key, MarketDataClient,
        MAX_RECONNECT_DELAY,
    };

    /// Serialize tests that mutate process environment variables.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn clear_market_env() {
        for key in &[
            "GODARK_MARKET_DATA_WS_URL",
            "GDX_MARKET_DATA_WS_URL",
            "GODARK_MARKET_DATA_USE_GOMARKET",
            "GDX_MARKET_DATA_USE_GOMARKET",
        ] {
            std::env::remove_var(key);
        }
    }

    #[test]
    fn test_url_defaults_to_ws_v1() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_market_env();
        let client = MarketDataClient::new("wss://api.godark-dex.com");
        assert_eq!(client.endpoint_url(), "wss://api.godark-dex.com/ws/v1");
    }

    #[test]
    fn test_url_gomarket_flag() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_market_env();
        std::env::set_var("GODARK_MARKET_DATA_USE_GOMARKET", "1");
        let client = MarketDataClient::new("wss://api.godark-dex.com");
        assert_eq!(
            client.endpoint_url(),
            "wss://api.godark-dex.com/ws/gomarket"
        );
        let client = MarketDataClient::new("wss://api.godark-dex.com/ws");
        assert_eq!(
            client.endpoint_url(),
            "wss://api.godark-dex.com/ws/gomarket"
        );
        let client = MarketDataClient::new("wss://api.godark-dex.com/ws/v1");
        assert_eq!(
            client.endpoint_url(),
            "wss://api.godark-dex.com/ws/gomarket"
        );
        std::env::remove_var("GODARK_MARKET_DATA_USE_GOMARKET");
    }

    #[test]
    fn test_new_not_connected() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_market_env();
        let client = MarketDataClient::new("wss://api.godark-dex.com");
        assert!(!client.is_connected());
    }

    #[test]
    fn test_new_auto_reconnect_true() {
        let _guard = ENV_LOCK.lock().unwrap();
        clear_market_env();
        let client = MarketDataClient::new("wss://api.godark-dex.com");
        assert!(client.auto_reconnect_enabled());
    }

    #[test]
    fn test_reconnect_backoff_capped() {
        let d = reconnect_backoff_delay(100);
        assert!(d <= MAX_RECONNECT_DELAY);
        assert!(d >= std::time::Duration::from_secs(1));
    }

    #[test]
    fn test_reconnect_backoff_exponential() {
        assert_eq!(
            reconnect_backoff_delay(0),
            std::time::Duration::from_secs(1)
        );
        assert_eq!(
            reconnect_backoff_delay(1),
            std::time::Duration::from_secs(2)
        );
        assert_eq!(
            reconnect_backoff_delay(2),
            std::time::Duration::from_secs(4)
        );
    }

    #[test]
    fn test_gomarket_event_key_orderbook_and_trade() {
        assert_eq!(
            gomarket_event_key(&json!({"type":"orderbook","symbol":"BTC-USDC-PERP"})),
            "orderbook:BTC-USDC-PERP"
        );
        assert_eq!(
            gomarket_event_key(&json!({"type":"trade","symbol":"BTC-USDC-PERP"})),
            "trades:BTC-USDC-PERP"
        );
    }

    #[test]
    fn test_subscription_callback_key_snapshots() {
        assert_eq!(
            subscription_callback_key(&json!({"type":"volume_snapshot"})),
            Some("volume:".to_string())
        );
        assert_eq!(
            subscription_callback_key(&json!({"type":"open_interest_snapshot"})),
            Some("open_interest:".to_string())
        );
        assert_eq!(
            subscription_callback_key(&json!({"type":"funding_rate_snapshot"})),
            Some("funding_rate:".to_string())
        );
        assert_eq!(subscription_callback_key(&json!({"type":"pong"})), None);
    }

    #[test]
    fn test_unsubscribe_payload_docs_wire() {
        let payload = super::unsubscribe_payload(true, "volume", "");
        assert_eq!(payload["op"], "unsubscribe");
        assert_eq!(payload["args"][0]["channel"], "volume");
        assert!(payload.get("action").is_none());
        let gomarket = super::unsubscribe_payload(false, "trades", "BTC-USDC-PERP");
        assert_eq!(gomarket["action"], "unsubscribe");
        assert_eq!(gomarket["channel"], "trades");
    }
}
