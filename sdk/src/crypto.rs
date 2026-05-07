// X25519 ECDH + AES-256-GCM — mirrors Python SDK _crypto.py

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::error::GodarkError;

const HKDF_INFO: &[u8] = b"gdx-e2e-session-key-v1";

/// Generate an ephemeral X25519 keypair.
/// Returns (private_key, 32-byte public key).
pub fn generate_ephemeral_keypair() -> (StaticSecret, [u8; 32]) {
    let mut rng = rand::rng();
    let mut key_bytes = [0u8; 32];
    rand::Fill::fill(&mut key_bytes, &mut rng);
    let secret = StaticSecret::from(key_bytes);
    let public = PublicKey::from(&secret);
    (secret, public.to_bytes())
}

/// Derive a 32-byte AES session key from X25519 ECDH + HKDF-SHA256.
///
/// HKDF salt = min(local_pub, remote_pub) || max(local_pub, remote_pub)
/// (byte-lexicographic, matching Rust gdx-crypto and Python SDK).
pub fn derive_session_key(
    private_key: &StaticSecret,
    local_public: &[u8; 32],
    remote_public: &[u8; 32],
) -> Result<Zeroizing<[u8; 32]>, GodarkError> {
    let remote_pk = PublicKey::from(*remote_public);
    let shared_secret = private_key.diffie_hellman(&remote_pk);

    if shared_secret.as_bytes() == &[0u8; 32] {
        return Err(GodarkError::Encryption(
            "Weak public key: ECDH shared secret is all zeros".into(),
        ));
    }

    let salt = if local_public <= remote_public {
        [local_public.as_slice(), remote_public.as_slice()].concat()
    } else {
        [remote_public.as_slice(), local_public.as_slice()].concat()
    };

    let hk = Hkdf::<Sha256>::new(Some(&salt), shared_secret.as_bytes());
    let mut okm = Zeroizing::new([0u8; 32]);
    hk.expand(HKDF_INFO, okm.as_mut())
        .map_err(|e| GodarkError::Encryption(format!("HKDF expand failed: {e}")))?;

    Ok(okm)
}

/// Build 96-bit GCM nonce: session_id (64-bit BE) || counter (32-bit BE).
pub fn build_gcm_nonce(session_id: u64, counter: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(&session_id.to_be_bytes());
    nonce[8..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

/// Encrypt with AES-256-GCM. Returns ciphertext + 16-byte auth tag.
pub fn encrypt(
    key: &[u8; 32],
    counter: u32,
    session_id: u64,
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, GodarkError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| GodarkError::Encryption(format!("AES key init: {e}")))?;
    let nonce_bytes = build_gcm_nonce(session_id, counter);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let payload = aes_gcm::aead::Payload {
        msg: plaintext,
        aad,
    };
    cipher
        .encrypt(nonce, payload)
        .map_err(|e| GodarkError::Encryption(format!("AES-GCM encrypt: {e}")))
}

/// Decrypt AES-256-GCM ciphertext (includes auth tag).
pub fn decrypt(
    key: &[u8; 32],
    counter: u32,
    session_id: u64,
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, GodarkError> {
    let cipher = Aes256Gcm::new_from_slice(key)
        .map_err(|e| GodarkError::Encryption(format!("AES key init: {e}")))?;
    let nonce_bytes = build_gcm_nonce(session_id, counter);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let payload = aes_gcm::aead::Payload {
        msg: ciphertext,
        aad,
    };
    cipher
        .decrypt(nonce, payload)
        .map_err(|e| GodarkError::Encryption(format!("AES-GCM decrypt: {e}")))
}

/// Monotonic send nonce counter + receive replay detection.
#[derive(Debug)]
pub struct NonceTracker {
    send_counter: u32,
    last_recv: Option<u32>,
}

impl NonceTracker {
    pub fn new() -> Self {
        Self {
            send_counter: 0,
            last_recv: None,
        }
    }

    pub fn peek_next(&self) -> u32 {
        self.send_counter
    }

    pub fn advance(&mut self) -> Result<u32, GodarkError> {
        let n = self.send_counter;
        if n == u32::MAX {
            return Err(GodarkError::Encryption(
                "Send nonce counter exceeded u32 max".into(),
            ));
        }
        self.send_counter = n + 1;
        Ok(n)
    }

    pub fn commit_recv(&mut self, received: u32) {
        self.last_recv = Some(received);
    }

    pub fn reset(&mut self) {
        self.send_counter = 0;
        self.last_recv = None;
    }
}

impl Default for NonceTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
impl NonceTracker {
    fn test_send_counter(&self) -> u32 {
        self.send_counter
    }

    fn test_last_recv(&self) -> Option<u32> {
        self.last_recv
    }

    fn with_send_counter(counter: u32) -> Self {
        Self {
            send_counter: counter,
            last_recv: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keypair_produces_32_byte_pubkey() {
        let (_sk, pk) = generate_ephemeral_keypair();
        assert_eq!(pk.len(), 32);
    }

    #[test]
    fn test_keypair_different_each_time() {
        let (_sk1, pk1) = generate_ephemeral_keypair();
        let (_sk2, pk2) = generate_ephemeral_keypair();
        assert_ne!(pk1, pk2);
    }

    #[test]
    fn test_derive_session_key_deterministic() {
        let (sk_a, pk_a) = generate_ephemeral_keypair();
        let (_sk_b, pk_b) = generate_ephemeral_keypair();

        let k1 = derive_session_key(&sk_a, &pk_a, &pk_b).unwrap();
        let k2 = derive_session_key(&sk_a, &pk_a, &pk_b).unwrap();
        assert_eq!(k1.as_ref(), k2.as_ref());
    }

    #[test]
    fn test_derive_session_key_salt_ordering() {
        let (sk_a, pk_a) = generate_ephemeral_keypair();
        let (sk_b, pk_b) = generate_ephemeral_keypair();

        let from_alice = derive_session_key(&sk_a, &pk_a, &pk_b).unwrap();
        let from_bob = derive_session_key(&sk_b, &pk_b, &pk_a).unwrap();
        assert_eq!(from_alice.as_ref(), from_bob.as_ref());
    }

    #[test]
    fn test_gcm_nonce_layout() {
        let n = build_gcm_nonce(0x0102_0304_0506_0708, 0x0A0B_0C0D);
        assert_eq!(
            n,
            [0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x0A, 0x0B, 0x0C, 0x0D]
        );
    }

    #[test]
    fn test_gcm_nonce_zero() {
        let n = build_gcm_nonce(0, 0);
        assert_eq!(n, [0u8; 12]);
    }

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let key = [7u8; 32];
        let session_id = 99u64;
        let counter = 0u32;
        let aad = b"meta";
        let plaintext = b"hello world";

        let ct = encrypt(&key, counter, session_id, aad, plaintext).unwrap();
        let out = decrypt(&key, counter, session_id, aad, &ct).unwrap();
        assert_eq!(out, plaintext);
    }

    #[test]
    fn test_encrypt_decrypt_with_aad() {
        let key = [3u8; 32];
        let session_id = 1u64;
        let counter = 5u32;
        let aad_ok = b"expected-aad";
        let aad_bad = b"wrong-aad___";
        let plaintext = b"payload";

        let ct = encrypt(&key, counter, session_id, aad_ok, plaintext).unwrap();
        let ok = decrypt(&key, counter, session_id, aad_ok, &ct);
        assert!(ok.is_ok());
        assert_eq!(ok.unwrap(), plaintext);

        let bad = decrypt(&key, counter, session_id, aad_bad, &ct);
        assert!(bad.is_err());
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let key1 = [1u8; 32];
        let key2 = [2u8; 32];
        let session_id = 42u64;
        let counter = 0u32;
        let aad = b"";
        let plaintext = b"secret";

        let ct = encrypt(&key1, counter, session_id, aad, plaintext).unwrap();
        let err = decrypt(&key2, counter, session_id, aad, &ct);
        assert!(err.is_err());
    }

    #[test]
    fn test_decrypt_wrong_aad_fails() {
        let key = [9u8; 32];
        let session_id = 7u64;
        let counter = 2u32;
        let aad1 = b"aad-one";
        let aad2 = b"aad-two";
        let plaintext = b"data";

        let ct = encrypt(&key, counter, session_id, aad1, plaintext).unwrap();
        assert!(decrypt(&key, counter, session_id, aad2, &ct).is_err());
    }

    #[test]
    fn test_nonce_tracker_monotonic() {
        let mut t = NonceTracker::new();
        assert_eq!(t.peek_next(), 0);
        assert_eq!(t.advance().unwrap(), 0);
        assert_eq!(t.peek_next(), 1);
        assert_eq!(t.advance().unwrap(), 1);
        assert_eq!(t.peek_next(), 2);
        assert_eq!(t.advance().unwrap(), 2);
        assert_eq!(t.peek_next(), 3);
        assert_eq!(t.advance().unwrap(), 3);
    }

    #[test]
    fn test_nonce_tracker_reset() {
        let mut t = NonceTracker::new();
        t.advance().unwrap();
        t.advance().unwrap();
        t.advance().unwrap();
        assert_eq!(t.test_send_counter(), 3);
        t.reset();
        assert_eq!(t.peek_next(), 0);
        assert_eq!(t.test_send_counter(), 0);
    }

    #[test]
    fn test_nonce_tracker_overflow() {
        let mut t = NonceTracker::with_send_counter(u32::MAX);
        let err = t.advance();
        assert!(err.is_err());
        match err.unwrap_err() {
            GodarkError::Encryption(msg) => {
                assert!(msg.contains("max") || msg.contains("u32"));
            }
            other => panic!("expected Encryption error, got {other:?}"),
        }
    }

    #[test]
    fn test_nonce_tracker_commit_recv() {
        let mut t = NonceTracker::new();
        assert_eq!(t.test_last_recv(), None);
        t.commit_recv(12345);
        assert_eq!(t.test_last_recv(), Some(12345));
    }
}
