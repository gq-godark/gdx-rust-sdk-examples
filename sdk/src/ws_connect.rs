//! Shared WebSocket dial helper for trading transport and market data.

use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::HeaderName;
use tokio_tungstenite::tungstenite::http::Request;
use tokio_tungstenite::tungstenite::Error as WsError;
use tokio_tungstenite::{connect_async_tls_with_config, connect_async_with_config, Connector};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use crate::config::TransportConfig;
use crate::error::GodarkError;

type WsStream = WebSocketStream<MaybeTlsStream<TcpStream>>;

fn build_request(url: &str, cfg: &TransportConfig) -> Result<Request<()>, GodarkError> {
    let mut request = url
        .into_client_request()
        .map_err(|e| GodarkError::Connection(format!("WebSocket request: {e}")))?;

    for (k, v) in &cfg.extra_headers {
        let name = HeaderName::from_bytes(k.as_bytes())
            .map_err(|e| GodarkError::Config(format!("Invalid header name {k:?}: {e}")))?;
        let value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(v)
            .map_err(|e| GodarkError::Config(format!("Invalid header value for {k}: {e}")))?;
        request.headers_mut().insert(name, value);
    }
    Ok(request)
}

/// Establish a client WebSocket using [`TransportConfig`] (timeouts, headers, TLS verify override).
pub async fn connect_websocket(url: &str, cfg: &TransportConfig) -> Result<WsStream, GodarkError> {
    let connect_inner = async {
        if cfg.tls_skip_verify {
            let request = build_request(url, cfg)?;
            let tls = native_tls::TlsConnector::builder()
                .danger_accept_invalid_certs(true)
                .build()
                .map_err(|e| GodarkError::Connection(format!("TLS: {e}")))?;
            connect_async_tls_with_config(request, None, false, Some(Connector::NativeTls(tls)))
                .await
        } else {
            let request = build_request(url, cfg)?;
            connect_async_with_config(request, None, false).await
        }
        .map(|(ws, _)| ws)
        .map_err(ws_err_to_godark)
    };

    tokio::time::timeout(cfg.connect_timeout, connect_inner)
        .await
        .map_err(|_| {
            GodarkError::Timeout(format!(
                "WebSocket connect timed out after {:?}",
                cfg.connect_timeout
            ))
        })?
}

fn ws_err_to_godark(e: WsError) -> GodarkError {
    GodarkError::Connection(format!("WebSocket connect: {e}"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::time::Duration;

    use super::*;

    #[test]
    fn test_transport_config_defaults() {
        let d = TransportConfig::default();
        assert!(!d.tls_skip_verify);
        assert!(d.extra_headers.is_empty());
        assert_eq!(d.connect_timeout, Duration::from_secs(30));
        assert_eq!(d.command_timeout, Duration::from_secs(30));
        assert_eq!(d.stale_timeout, Duration::from_secs(60));
        assert_eq!(d.heartbeat_interval, Duration::from_secs(30));
    }

    #[test]
    fn test_transport_config_custom_timeouts() {
        let cfg = TransportConfig {
            connect_timeout: Duration::from_secs(5),
            command_timeout: Duration::from_secs(7),
            stale_timeout: Duration::from_secs(10),
            heartbeat_interval: Duration::from_secs(2),
            ..TransportConfig::default()
        };
        assert_eq!(cfg.connect_timeout, Duration::from_secs(5));
        assert_eq!(cfg.command_timeout, Duration::from_secs(7));
    }

    #[test]
    fn test_transport_config_extra_headers() {
        let mut h = HashMap::new();
        h.insert("X-Test".to_string(), "1".to_string());
        let cfg = TransportConfig {
            extra_headers: h,
            ..TransportConfig::default()
        };
        assert_eq!(cfg.extra_headers.get("X-Test"), Some(&"1".to_string()));
    }

    #[test]
    fn test_transport_config_tls_skip_verify_flag() {
        let cfg = TransportConfig {
            tls_skip_verify: true,
            ..TransportConfig::default()
        };
        assert!(cfg.tls_skip_verify);
    }
}
