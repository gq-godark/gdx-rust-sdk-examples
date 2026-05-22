// GodarkError hierarchy — mirrors Python SDK errors.py

/// Unified error type for the GoDark SDK.
#[derive(Debug, thiserror::Error)]
pub enum GodarkError {
    #[error("authentication failed: {0}")]
    Authentication(String),

    #[error("ECDH session error: {0}")]
    Session(String),

    #[error("order rejected: {message}")]
    Order {
        message: String,
        error_code: Option<String>,
    },

    #[error("connection error: {0}")]
    Connection(String),

    #[error("encryption error: {0}")]
    Encryption(String),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("configuration error: {0}")]
    Config(String),

    #[error(transparent)]
    WebSocket(#[from] tokio_tungstenite::tungstenite::Error),

    #[error(transparent)]
    Proto(#[from] prost::DecodeError),
}

pub type Result<T> = std::result::Result<T, GodarkError>;

#[cfg(test)]
mod tests {
    use super::{GodarkError, Result};

    #[test]
    fn test_authentication_error_display() {
        let err = GodarkError::Authentication("bad token".into());
        let s = err.to_string();
        assert!(
            s.contains("authentication failed"),
            "unexpected display: {s}"
        );
    }

    #[test]
    fn test_session_error_display() {
        let err = GodarkError::Session("handshake failed".into());
        let s = err.to_string();
        assert!(s.contains("ECDH session error"), "unexpected display: {s}");
    }

    #[test]
    fn test_order_error_with_code() {
        let err = GodarkError::Order {
            message: "insufficient balance".into(),
            error_code: Some("E001".into()),
        };
        let s = err.to_string();
        assert!(s.contains("order rejected"), "unexpected display: {s}");
        assert!(
            s.contains("insufficient balance"),
            "unexpected display: {s}"
        );
    }

    #[test]
    fn test_order_error_without_code() {
        let err = GodarkError::Order {
            message: "no liquidity".into(),
            error_code: None,
        };
        let s = err.to_string();
        assert!(s.contains("order rejected"), "unexpected display: {s}");
        assert!(s.contains("no liquidity"), "unexpected display: {s}");
    }

    #[test]
    fn test_connection_error_display() {
        let err = GodarkError::Connection("reset by peer".into());
        let s = err.to_string();
        assert!(s.contains("connection error"), "unexpected display: {s}");
    }

    #[test]
    fn test_encryption_error_display() {
        let err = GodarkError::Encryption("bad tag".into());
        let s = err.to_string();
        assert!(s.contains("encryption error"), "unexpected display: {s}");
    }

    #[test]
    fn test_timeout_error_display() {
        let err = GodarkError::Timeout("deadline exceeded".into());
        let s = err.to_string();
        assert!(s.contains("timeout"), "unexpected display: {s}");
    }

    #[test]
    fn test_config_error_display() {
        let err = GodarkError::Config("missing API URL".into());
        let s = err.to_string();
        assert!(s.contains("configuration error"), "unexpected display: {s}");
    }

    #[test]
    fn test_from_prost_decode_error() {
        let decode_err = prost::DecodeError::new("truncated message");
        let err: GodarkError = decode_err.into();
        assert!(matches!(err, GodarkError::Proto(_)));
        let s = err.to_string();
        assert!(
            s.contains("failed to decode Protobuf message"),
            "unexpected display: {s}"
        );
    }

    #[test]
    fn test_result_type_alias() {
        let ok: Result<()> = Ok(());
        assert!(ok.is_ok());

        let err: Result<()> = Err(GodarkError::Config("test".into()));
        assert!(err.is_err());
        let e = match err {
            Err(e) => e,
            Ok(()) => panic!("expected Err"),
        };
        assert_eq!(
            e.to_string(),
            GodarkError::Config("test".into()).to_string()
        );
    }
}
