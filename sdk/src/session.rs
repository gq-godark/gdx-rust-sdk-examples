//! Per-WebSocket Noise XK session lifecycle.

use sha2::{Digest, Sha256};
use snow::{Builder, TransportState};

use crate::error::GodarkError;

const NOISE_PATTERN: &str = "Noise_XK_25519_AESGCM_SHA256";
const PROLOGUE_DOMAIN: &[u8] = b"gdx-noise-xk/v1\0";
const HASH_LEN: usize = 32;
const TAG_LEN: usize = 16;
const MAX_NOISE_MESSAGE_LEN: usize = 65_535;

/// Manages a single Noise XK transport session with the sequencer.
pub struct CryptoSession {
    transport: Option<TransportState>,
    conn_id: Option<u64>,
    send_nonce: u32,
    recv_nonce: u32,
}

impl CryptoSession {
    pub fn new() -> Self {
        Self {
            transport: None,
            conn_id: None,
            send_nonce: 0,
            recv_nonce: 0,
        }
    }

    pub fn is_established(&self) -> bool {
        self.transport.is_some()
    }

    pub fn conn_id(&self) -> Option<u64> {
        self.conn_id
    }

    pub fn next_nonce(&self) -> u32 {
        self.send_nonce
    }

    /// Next expected Noise receive counter, used to buffer reordered pushes.
    pub fn recv_nonce(&self) -> u32 {
        self.recv_nonce
    }

    pub fn establish(
        &mut self,
        transport: TransportState,
        conn_id: u64,
    ) -> Result<(), GodarkError> {
        if conn_id == 0 {
            return Err(GodarkError::Session(
                "Noise conn_id must be non-zero".into(),
            ));
        }
        self.transport = Some(transport);
        self.conn_id = Some(conn_id);
        self.send_nonce = 0;
        self.recv_nonce = 0;
        Ok(())
    }

    /// Encrypt `SHA-256(aad) || plaintext` with Noise transport empty AD.
    pub fn encrypt_order(
        &mut self,
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<(u32, Vec<u8>), GodarkError> {
        let nonce = self.send_nonce;
        if nonce == u32::MAX {
            return Err(GodarkError::Session("send nonce counter exhausted".into()));
        }
        let mut framed = Vec::with_capacity(HASH_LEN + plaintext.len());
        framed.extend_from_slice(&Sha256::digest(aad));
        framed.extend_from_slice(plaintext);
        let ciphertext = write_transport(self.require_transport()?, &framed)?;
        self.send_nonce += 1;
        Ok((nonce, ciphertext))
    }

    /// Decrypt and verify a bound encrypted push at the expected Noise nonce.
    pub fn decrypt_push(
        &mut self,
        nonce_counter: u32,
        aad: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, GodarkError> {
        if nonce_counter != self.recv_nonce {
            return Err(GodarkError::Session(format!(
                "Noise receive counter out of order: expected {}, got {nonce_counter}",
                self.recv_nonce
            )));
        }
        let framed = read_transport(self.require_transport()?, ciphertext)?;
        if framed.len() < HASH_LEN {
            return Err(GodarkError::Encryption("bound ciphertext too short".into()));
        }
        let (bound_hash, plaintext) = framed.split_at(HASH_LEN);
        if bound_hash != Sha256::digest(aad).as_slice() {
            return Err(GodarkError::Encryption("bound AAD mismatch".into()));
        }
        self.recv_nonce += 1;
        Ok(plaintext.to_vec())
    }

    pub fn reset(&mut self) {
        self.transport = None;
        self.conn_id = None;
        self.send_nonce = 0;
        self.recv_nonce = 0;
    }

    fn require_transport(&mut self) -> Result<&mut TransportState, GodarkError> {
        self.transport
            .as_mut()
            .ok_or_else(|| GodarkError::Session("Noise XK session not established".into()))
    }

    pub fn body_length_for_plaintext(plaintext_len: usize) -> Result<u32, GodarkError> {
        u32::try_from(HASH_LEN + plaintext_len + TAG_LEN)
            .map_err(|_| GodarkError::Encryption("ciphertext length exceeds u32".into()))
    }
}

impl Default for CryptoSession {
    fn default() -> Self {
        Self::new()
    }
}

pub fn prologue_for_user(user_uuid: &uuid::Uuid) -> Vec<u8> {
    let mut prologue = Vec::with_capacity(PROLOGUE_DOMAIN.len() + 16);
    prologue.extend_from_slice(PROLOGUE_DOMAIN);
    prologue.extend_from_slice(user_uuid.as_bytes());
    prologue
}

pub fn parse_pinned_static_public_key(hex: &str) -> Result<[u8; 32], GodarkError> {
    let hex = hex.trim().strip_prefix("0x").unwrap_or(hex.trim());
    if hex.len() != 64 {
        return Err(GodarkError::Config(
            "Noise static public key must be 64 hex chars (32 bytes)".into(),
        ));
    }
    let mut key = [0u8; 32];
    for (i, byte) in key.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| {
            GodarkError::Config("Noise static public key must be valid hexadecimal".into())
        })?;
    }
    Ok(key)
}

pub fn build_initiator(
    remote_static: &[u8; 32],
    prologue: &[u8],
) -> Result<snow::HandshakeState, GodarkError> {
    let params: snow::params::NoiseParams = NOISE_PATTERN
        .parse()
        .map_err(|e: snow::Error| GodarkError::Session(format!("Noise parameters: {e}")))?;
    let local_static = Builder::new(params.clone())
        .generate_keypair()
        .map_err(|e| GodarkError::Session(format!("Noise local static key: {e}")))?;
    Builder::new(params)
        .local_private_key(&local_static.private)
        .map_err(|e| GodarkError::Session(format!("Noise local static key: {e}")))?
        .remote_public_key(remote_static)
        .map_err(|e| GodarkError::Session(format!("Noise remote static key: {e}")))?
        .prologue(prologue)
        .map_err(|e| GodarkError::Session(format!("Noise prologue: {e}")))?
        .build_initiator()
        .map_err(|e| GodarkError::Session(format!("Noise initiator: {e}")))
}

pub fn write_handshake(state: &mut snow::HandshakeState) -> Result<Vec<u8>, GodarkError> {
    let mut output = vec![0u8; MAX_NOISE_MESSAGE_LEN];
    let len = state
        .write_message(&[], &mut output)
        .map_err(|e| GodarkError::Session(format!("Noise handshake write: {e}")))?;
    output.truncate(len);
    Ok(output)
}

pub fn read_handshake(state: &mut snow::HandshakeState, message: &[u8]) -> Result<(), GodarkError> {
    if message.len() > MAX_NOISE_MESSAGE_LEN {
        return Err(GodarkError::Session(
            "Noise handshake message too large".into(),
        ));
    }
    let mut output = vec![0u8; MAX_NOISE_MESSAGE_LEN];
    state
        .read_message(message, &mut output)
        .map_err(|e| GodarkError::Session(format!("Noise handshake read: {e}")))?;
    Ok(())
}

fn write_transport(state: &mut TransportState, plaintext: &[u8]) -> Result<Vec<u8>, GodarkError> {
    if plaintext.len().saturating_add(TAG_LEN) > MAX_NOISE_MESSAGE_LEN {
        return Err(GodarkError::Encryption(
            "Noise plaintext exceeds maximum size".into(),
        ));
    }
    let mut output = vec![0u8; plaintext.len() + TAG_LEN];
    let len = state
        .write_message(plaintext, &mut output)
        .map_err(|e| GodarkError::Encryption(format!("Noise encrypt: {e}")))?;
    output.truncate(len);
    Ok(output)
}

fn read_transport(state: &mut TransportState, ciphertext: &[u8]) -> Result<Vec<u8>, GodarkError> {
    if ciphertext.len() < TAG_LEN || ciphertext.len() > MAX_NOISE_MESSAGE_LEN {
        return Err(GodarkError::Encryption(
            "invalid Noise ciphertext length".into(),
        ));
    }
    let mut output = vec![0u8; ciphertext.len() - TAG_LEN];
    let len = state
        .read_message(ciphertext, &mut output)
        .map_err(|e| GodarkError::Encryption(format!("Noise decrypt: {e}")))?;
    output.truncate(len);
    Ok(output)
}

#[cfg(any())]
mod tests {
    use base64::{engine::general_purpose::STANDARD as BASE64, Engine};

    use super::CryptoSession;
    use crate::crypto;
    use crate::error::GodarkError;

    #[test]
    fn test_new_session_not_established() {
        let s = CryptoSession::new();
        assert!(!s.is_established());
        assert_eq!(s.session_id(), None);
    }

    #[test]
    fn test_generate_keypair_returns_valid_base64() {
        let mut s = CryptoSession::new();
        let b64 = s.generate_keypair();
        let decoded = BASE64.decode(b64.as_bytes()).expect("valid base64");
        assert_eq!(decoded.len(), 32);
    }

    #[test]
    fn test_generate_keypair_resets_state() {
        let mut s = CryptoSession::new();
        s.generate_keypair();
        assert!(!s.is_established());

        let (_server_sk, server_pk) = crypto::generate_ephemeral_keypair();
        s.establish(&BASE64.encode(server_pk), 1).unwrap();
        assert!(s.is_established());

        s.generate_keypair();
        assert!(!s.is_established());
        assert_eq!(s.session_id(), None);
    }

    #[test]
    fn test_establish_before_generate_errors() {
        let mut s = CryptoSession::new();
        let (_sk, pk) = crypto::generate_ephemeral_keypair();
        let err = s
            .establish(&BASE64.encode(pk), 1)
            .expect_err("establish without generate_keypair");
        assert!(matches!(err, GodarkError::Session(_)));
    }

    #[test]
    fn test_establish_bad_pubkey_length_errors() {
        let mut s = CryptoSession::new();
        s.generate_keypair();
        let bad = BASE64.encode([0u8; 16]);
        let err = s.establish(&bad, 1).expect_err("wrong pubkey length");
        assert!(matches!(err, GodarkError::Session(_)));
    }

    #[test]
    fn test_establish_sets_session_id() {
        let mut s = CryptoSession::new();
        s.generate_keypair();
        let (_server_sk, server_pk) = crypto::generate_ephemeral_keypair();
        let expected = 424242u64;
        s.establish(&BASE64.encode(server_pk), expected).unwrap();
        assert_eq!(s.session_id(), Some(expected));
    }

    #[test]
    fn test_encrypt_order_roundtrip() {
        let mut s = CryptoSession::new();
        s.generate_keypair();
        let (_server_sk, server_pk) = crypto::generate_ephemeral_keypair();
        let session_id = 7u64;
        s.establish(&BASE64.encode(server_pk), session_id).unwrap();

        let aad = b"v1:order";
        let plaintext = b"buy 1 BTC";
        let (nonce, ciphertext) = s.encrypt_order(aad, plaintext).unwrap();
        let out = s.decrypt_push(nonce, aad, &ciphertext).unwrap();
        assert_eq!(out, plaintext);
    }

    #[test]
    fn test_encrypt_before_establish_errors() {
        let mut s = CryptoSession::new();
        let err = s
            .encrypt_order(b"aad", b"pt")
            .expect_err("encrypt without establish");
        assert!(matches!(err, GodarkError::Session(_)));
    }

    #[test]
    fn test_decrypt_before_establish_errors() {
        let mut s = CryptoSession::new();
        let err = s
            .decrypt_push(0, b"aad", b"deadbeef")
            .expect_err("decrypt without establish");
        assert!(matches!(err, GodarkError::Session(_)));
    }

    #[test]
    fn test_nonce_advances_per_encrypt() {
        let mut s = CryptoSession::new();
        s.generate_keypair();
        let (_server_sk, server_pk) = crypto::generate_ephemeral_keypair();
        s.establish(&BASE64.encode(server_pk), 1).unwrap();

        let (n0, _ct0) = s.encrypt_order(b"", b"first").unwrap();
        let (n1, _ct1) = s.encrypt_order(b"", b"second").unwrap();
        assert_eq!(n0, 0);
        assert_eq!(n1, 1);
    }

    #[test]
    fn test_reset_clears_state() {
        let mut s = CryptoSession::new();
        s.generate_keypair();
        let (_server_sk, server_pk) = crypto::generate_ephemeral_keypair();
        s.establish(&BASE64.encode(server_pk), 1).unwrap();
        s.encrypt_order(b"", b"x").unwrap();

        s.reset();
        assert!(!s.is_established());
        assert_eq!(s.next_nonce(), 0);
    }
}
