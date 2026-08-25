//! Per-connection HPKE sealed session.

use std::collections::HashSet;

use uuid::Uuid;

use crate::error::GodarkError;
use crate::hpke::{self, SealedSession, TAG_LEN};

pub struct CryptoSession {
    sealed: Option<SealedSession>,
    pending_sealed: Option<SealedSession>,
    pending_conn_id: u64,
    send_counter: u64,
    conn_id: u64,
    seen_recv_nonces: HashSet<u64>,
}

impl CryptoSession {
    pub fn new() -> Self {
        Self {
            sealed: None,
            pending_sealed: None,
            pending_conn_id: 0,
            send_counter: 1,
            conn_id: 0,
            seen_recv_nonces: HashSet::new(),
        }
    }

    pub fn is_established(&self) -> bool {
        self.sealed.is_some()
    }

    pub fn conn_id(&self) -> Option<u64> {
        (self.conn_id != 0).then_some(self.conn_id)
    }

    pub fn next_nonce(&self) -> u64 {
        self.send_counter
    }

    pub fn body_length_for_plaintext(plaintext_len: usize) -> Result<u32, GodarkError> {
        u32::try_from(plaintext_len + TAG_LEN)
            .map_err(|_| GodarkError::Encryption("encrypted body too large".into()))
    }

    /// HPKE Base setup against the pinned sequencer public key.
    ///
    /// Returns the encapped key for ``hpke_setup``; the session is not
    /// established until [`Self::establish`] confirms the peer reply.
    pub fn setup(
        &mut self,
        recipient_public: &[u8; 32],
        user_uuid: Uuid,
        conn_id: u64,
    ) -> Result<Vec<u8>, GodarkError> {
        if conn_id == 0 {
            return Err(GodarkError::Session("HPKE conn_id must be non-zero".into()));
        }
        let info = hpke::info_for_conn(user_uuid, conn_id);
        let (encapped, sealed) = hpke::setup_session(recipient_public, &info)?;
        self.pending_sealed = Some(sealed);
        self.pending_conn_id = conn_id;
        Ok(encapped)
    }

    /// Commit a pending HPKE setup after the sequencer confirms.
    pub fn establish(&mut self) -> Result<(), GodarkError> {
        let sealed = self
            .pending_sealed
            .take()
            .ok_or_else(|| GodarkError::Session("HPKE setup not pending peer confirmation".into()))?;
        self.sealed = Some(sealed);
        self.conn_id = self.pending_conn_id;
        self.pending_conn_id = 0;
        self.send_counter = 1;
        self.seen_recv_nonces.clear();
        Ok(())
    }

    /// Discard a pending HPKE setup (timeout, rejection, or mismatch).
    pub fn abort_setup(&mut self) {
        self.pending_sealed = None;
        self.pending_conn_id = 0;
    }

    /// One-shot REST HPKE (conn_id is 0 on the order header).
    pub fn setup_rest(
        recipient_public: &[u8; 32],
        user_uuid: Uuid,
        request_id: u64,
    ) -> Result<(Vec<u8>, SealedSession), GodarkError> {
        let info = hpke::info_for_rest_request(user_uuid, request_id);
        hpke::setup_session(recipient_public, &info)
    }

    pub fn encrypt_order(
        &mut self,
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<(u64, Vec<u8>), GodarkError> {
        let sealed = self
            .sealed
            .as_ref()
            .ok_or_else(|| GodarkError::Session("HPKE session not established".into()))?;
        let nonce = self.send_counter;
        if nonce == u64::MAX {
            return Err(GodarkError::Encryption("send nonce overflow".into()));
        }
        let ct = sealed.seal_c2s(&hpke::nonce_from_u64(nonce), aad, plaintext)?;
        self.send_counter = nonce + 1;
        Ok((nonce, ct))
    }

    pub fn decrypt_push(
        &mut self,
        nonce: u64,
        aad: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, GodarkError> {
        let sealed = self
            .sealed
            .as_ref()
            .ok_or_else(|| GodarkError::Session("HPKE session not established".into()))?;
        if self.seen_recv_nonces.contains(&nonce) {
            return Err(GodarkError::Encryption(format!(
                "replay detected: push nonce {nonce} already seen"
            )));
        }
        let pt = sealed.open_s2c(&hpke::nonce_from_u64(nonce), aad, ciphertext)?;
        self.seen_recv_nonces.insert(nonce);
        Ok(pt)
    }

    pub fn reset(&mut self) {
        self.sealed = None;
        self.pending_sealed = None;
        self.pending_conn_id = 0;
        self.send_counter = 1;
        self.conn_id = 0;
        self.seen_recv_nonces.clear();
    }
}

impl Default for CryptoSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hpke::StaticKeyPair;

    #[test]
    fn setup_encrypt_decrypt_roundtrip() {
        let seq = StaticKeyPair::generate().unwrap();
        let user = Uuid::from_u128(1);
        let mut client = CryptoSession::new();
        let enc = client.setup(seq.public_key(), user, 9).unwrap();
        assert!(!client.is_established());
        client.establish().unwrap();
        let server = hpke::open_session(&seq, &enc, &hpke::info_for_conn(user, 9)).unwrap();

        let aad = b"order-header";
        let (nonce, ct) = client.encrypt_order(aad, b"place").unwrap();
        assert_eq!(nonce, 1);
        assert_eq!(
            server
                .open_c2s(&hpke::nonce_from_u64(nonce), aad, &ct)
                .unwrap(),
            b"place"
        );

        let push = server
            .seal_s2c(&hpke::nonce_from_u64(2), b"resp", b"ack")
            .unwrap();
        assert_eq!(client.decrypt_push(2, b"resp", &push).unwrap(), b"ack");
    }
}
