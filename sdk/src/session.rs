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

    /// Next Noise transport receive counter (implicit; independent of envelope nonce).
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

    /// Decrypt an inbound Noise transport message.
    ///
    /// The sequencer's cleartext `envelope_nonce` equals its Noise transport
    /// send counter for this frame (both advance once per response). The edge
    /// is a blind relay that may drop frames this client is not subscribed to,
    /// so arrival order alone can desync the receive counter. We therefore
    /// align snow's receiving nonce to `envelope_nonce` before each decrypt,
    /// which is robust to any relayed gaps while remaining exact for a
    /// gap-free stream.
    pub fn decrypt_push(
        &mut self,
        envelope_nonce: u32,
        aad: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, GodarkError> {
        let transport = self.require_transport()?;
        transport.set_receiving_nonce(u64::from(envelope_nonce));
        let framed = read_transport(transport, ciphertext)?;
        if framed.len() < HASH_LEN {
            return Err(GodarkError::Encryption("bound ciphertext too short".into()));
        }
        let (bound_hash, plaintext) = framed.split_at(HASH_LEN);
        if bound_hash != Sha256::digest(aad).as_slice() {
            return Err(GodarkError::Encryption("bound AAD mismatch".into()));
        }
        self.recv_nonce = envelope_nonce.saturating_add(1);
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

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};
    use snow::TransportState;

    use super::{build_initiator, prologue_for_user, CryptoSession};
    use crate::error::GodarkError;

    fn noise_client_server() -> (CryptoSession, TransportState) {
        let user = uuid::Uuid::from_u128(7);
        let prologue = prologue_for_user(&user);
        let params: snow::params::NoiseParams = "Noise_XK_25519_AESGCM_SHA256".parse().unwrap();
        let server_key = snow::Builder::new(params.clone())
            .generate_keypair()
            .unwrap();
        let server_public: [u8; 32] = server_key.public.as_slice().try_into().unwrap();
        let mut client_handshake = build_initiator(&server_public, &prologue).unwrap();
        let mut server_handshake = snow::Builder::new(params)
            .local_private_key(&server_key.private)
            .unwrap()
            .prologue(&prologue)
            .unwrap()
            .build_responder()
            .unwrap();
        let mut buffer = vec![0u8; 65_535];
        let len = client_handshake.write_message(&[], &mut buffer).unwrap();
        let msg1 = buffer[..len].to_vec();
        server_handshake.read_message(&msg1, &mut buffer).unwrap();
        let len = server_handshake.write_message(&[], &mut buffer).unwrap();
        let msg2 = buffer[..len].to_vec();
        client_handshake.read_message(&msg2, &mut buffer).unwrap();
        let len = client_handshake.write_message(&[], &mut buffer).unwrap();
        let msg3 = buffer[..len].to_vec();
        server_handshake.read_message(&msg3, &mut buffer).unwrap();

        let mut client = CryptoSession::new();
        client
            .establish(client_handshake.into_transport_mode().unwrap(), 7)
            .expect("establish");
        let server = server_handshake.into_transport_mode().unwrap();
        (client, server)
    }

    fn server_encrypt_bound(server: &mut TransportState, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut framed = Sha256::digest(aad).to_vec();
        framed.extend_from_slice(plaintext);
        let mut ct = vec![0u8; framed.len() + 16];
        let len = server.write_message(&framed, &mut ct).unwrap();
        ct.truncate(len);
        ct
    }

    #[test]
    fn test_new_session_not_established() {
        let s = CryptoSession::new();
        assert!(!s.is_established());
        assert_eq!(s.conn_id(), None);
        assert_eq!(s.recv_nonce(), 0);
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
        assert!(matches!(
            err,
            GodarkError::Session(_) | GodarkError::Encryption(_)
        ));
    }

    #[test]
    fn test_nonce_advances_per_encrypt() {
        let (mut client, _server) = noise_client_server();
        let (n0, _) = client.encrypt_order(b"", b"first").unwrap();
        let (n1, _) = client.encrypt_order(b"", b"second").unwrap();
        assert_eq!(n0, 0);
        assert_eq!(n1, 1);
    }

    #[test]
    fn test_reset_clears_state() {
        let (mut client, _server) = noise_client_server();
        client.encrypt_order(b"", b"x").unwrap();
        client.reset();
        assert!(!client.is_established());
        assert_eq!(client.next_nonce(), 0);
        assert_eq!(client.recv_nonce(), 0);
    }

    /// The cleartext envelope nonce is the authoritative Noise transport index.
    /// A gap (frame dropped by the blind edge relay) must still decrypt, because
    /// `decrypt_push` aligns snow's receiving nonce to the envelope nonce.
    #[test]
    fn decrypt_realigns_receiving_nonce_across_relay_gap() {
        let (mut client, mut server) = noise_client_server();
        assert_eq!(client.recv_nonce(), 0);

        let aad0 = b"envelope-nonce-0";
        let aad1 = b"envelope-nonce-1"; // encrypted by server but "dropped" in transit
        let aad2 = b"envelope-nonce-2";
        let pt0 = b"frame-zero";
        let _pt1 = b"frame-one-dropped";
        let pt2 = b"frame-two";
        let ct0 = server_encrypt_bound(&mut server, aad0, pt0); // snow send 0
        let _ct1 = server_encrypt_bound(&mut server, aad1, _pt1); // snow send 1 (dropped)
        let ct2 = server_encrypt_bound(&mut server, aad2, pt2); // snow send 2

        let out0 = client.decrypt_push(0, aad0, &ct0).expect("frame 0");
        assert_eq!(out0, pt0);
        assert_eq!(client.recv_nonce(), 1);

        // Skip envelope nonce 1 entirely; nonce 2 must still decrypt.
        let out2 = client
            .decrypt_push(2, aad2, &ct2)
            .expect("frame 2 after gap");
        assert_eq!(out2, pt2);
        assert_eq!(client.recv_nonce(), 3);
    }

    #[test]
    fn decrypt_rejects_aad_mismatch() {
        let (mut client, mut server) = noise_client_server();
        let ct = server_encrypt_bound(&mut server, b"correct-aad", b"body");
        let err = client
            .decrypt_push(9, b"wrong-aad", &ct)
            .expect_err("aad must bind");
        assert!(matches!(err, GodarkError::Encryption(_)));
    }
}
