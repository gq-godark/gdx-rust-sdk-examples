// ECDH session lifecycle — mirrors Python SDK _session.py

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use x25519_dalek::StaticSecret;
use zeroize::Zeroizing;

use crate::crypto::{self, NonceTracker};
use crate::error::GodarkError;

/// Manages a single ECDH session with the sequencer.
pub struct CryptoSession {
    private_key: Option<StaticSecret>,
    local_public: Option<[u8; 32]>,
    session_key: Option<Zeroizing<[u8; 32]>>,
    session_id: Option<u64>,
    nonce: NonceTracker,
    established: bool,
}

impl CryptoSession {
    pub fn new() -> Self {
        Self {
            private_key: None,
            local_public: None,
            session_key: None,
            session_id: None,
            nonce: NonceTracker::new(),
            established: false,
        }
    }

    pub fn is_established(&self) -> bool {
        self.established
    }

    pub fn session_id(&self) -> Option<u64> {
        self.session_id
    }

    pub fn next_nonce(&self) -> u32 {
        self.nonce.peek_next()
    }

    /// Generate ephemeral X25519 keypair. Returns base64-encoded public key.
    pub fn generate_keypair(&mut self) -> String {
        let (secret, public_bytes) = crypto::generate_ephemeral_keypair();
        self.private_key = Some(secret);
        self.local_public = Some(public_bytes);
        self.established = false;
        self.session_key = None;
        self.session_id = None;
        self.nonce.reset();
        BASE64.encode(public_bytes)
    }

    /// Complete ECDH handshake: derive session key from sequencer's public key.
    pub fn establish(
        &mut self,
        sequencer_pubkey_b64: &str,
        session_id: u64,
    ) -> Result<(), GodarkError> {
        let private_key = self
            .private_key
            .take()
            .ok_or_else(|| GodarkError::Session("Must call generate_keypair() first".into()))?;

        let local_public = self
            .local_public
            .ok_or_else(|| GodarkError::Session("Must call generate_keypair() first".into()))?;

        let remote_bytes = BASE64
            .decode(sequencer_pubkey_b64)
            .map_err(|e| GodarkError::Session(format!("Invalid base64 public key: {e}")))?;

        if remote_bytes.len() != 32 {
            return Err(GodarkError::Session(format!(
                "Sequencer public key must be 32 bytes, got {}",
                remote_bytes.len()
            )));
        }

        let mut remote_pub = [0u8; 32];
        remote_pub.copy_from_slice(&remote_bytes);

        let key = crypto::derive_session_key(&private_key, &local_public, &remote_pub)?;
        self.session_key = Some(key);
        self.session_id = Some(session_id);
        self.nonce.reset();
        self.established = true;
        Ok(())
    }

    /// Encrypt an order payload. Returns (nonce_counter, ciphertext).
    pub fn encrypt_order(
        &mut self,
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<(u32, Vec<u8>), GodarkError> {
        if !self.established {
            return Err(GodarkError::Session("Session not established".into()));
        }
        let key = self
            .session_key
            .as_ref()
            .ok_or_else(|| GodarkError::Session("No session key".into()))?;
        let session_id = self.session_id.unwrap();
        let nonce_counter = self.nonce.advance()?;
        let ct = crypto::encrypt(key, nonce_counter, session_id, aad, plaintext)?;
        Ok((nonce_counter, ct))
    }

    /// Decrypt an encrypted_push from the sequencer.
    pub fn decrypt_push(
        &mut self,
        nonce_counter: u32,
        aad: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, GodarkError> {
        if !self.established {
            return Err(GodarkError::Session("Session not established".into()));
        }
        let key = self
            .session_key
            .as_ref()
            .ok_or_else(|| GodarkError::Session("No session key".into()))?;
        let session_id = self.session_id.unwrap();
        let pt = crypto::decrypt(key, nonce_counter, session_id, aad, ciphertext)?;
        self.nonce.commit_recv(nonce_counter);
        Ok(pt)
    }

    /// Reset session state (for reconnect or rekey).
    pub fn reset(&mut self) {
        self.private_key = None;
        self.local_public = None;
        self.session_key = None;
        self.session_id = None;
        self.nonce.reset();
        self.established = false;
    }
}

impl Default for CryptoSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
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
