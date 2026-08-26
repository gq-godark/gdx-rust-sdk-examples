//! HPKE Base (RFC 9180) for trading E2E — matches `gdx_crypto::hpke`.
//!
//! Suite: DHKEM(X25519, HKDF-SHA256) + HKDF-SHA256 + AES-256-GCM.
//! After setup, peers export directional keys and seal each message with an
//! explicit 96-bit nonce (`0u32_be ‖ counter_be`).

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use hpke::{
    aead::AesGcm256, kdf::HkdfSha256, kem::X25519HkdfSha256, setup_receiver, setup_sender,
    Deserializable, Kem as KemTrait, OpModeR, OpModeS, Serializable,
};
use uuid::Uuid;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::error::GodarkError;

pub const KEY_LEN: usize = 32;
pub const ENCAPPED_KEY_LEN: usize = 32;
pub const TAG_LEN: usize = 16;
pub const WIRE_VERSION: u32 = 2;

pub const INFO_DOMAIN: &[u8] = b"gdx-hpke/v1\0";
pub const INFO_DOMAIN_REST: &[u8] = b"gdx-hpke/v1/rest\0";
pub const EXPORT_C2S: &[u8] = b"gdx-hpke/v1 c2s";
pub const EXPORT_S2C: &[u8] = b"gdx-hpke/v1 s2c";

type Kem = X25519HkdfSha256;
type Kdf = HkdfSha256;
type AeadAlg = AesGcm256;

/// `gdx-hpke/v1\0 ‖ user_uuid ‖ conn_id_be`
pub fn info_for_conn(user_uuid: Uuid, conn_id: u64) -> Vec<u8> {
    let mut info = Vec::with_capacity(INFO_DOMAIN.len() + 16 + 8);
    info.extend_from_slice(INFO_DOMAIN);
    info.extend_from_slice(user_uuid.as_bytes());
    info.extend_from_slice(&conn_id.to_be_bytes());
    info
}

/// `gdx-hpke/v1/rest\0 ‖ user_uuid ‖ request_id_be`
pub fn info_for_rest_request(user_uuid: Uuid, request_id: u64) -> Vec<u8> {
    let mut info = Vec::with_capacity(INFO_DOMAIN_REST.len() + 16 + 8);
    info.extend_from_slice(INFO_DOMAIN_REST);
    info.extend_from_slice(user_uuid.as_bytes());
    info.extend_from_slice(&request_id.to_be_bytes());
    info
}

/// Pack a monotonic u64 into a 96-bit GCM nonce: `0u32_be ‖ counter_be`.
pub fn nonce_from_u64(counter: u64) -> [u8; 12] {
    let mut out = [0u8; 12];
    out[4..].copy_from_slice(&counter.to_be_bytes());
    out
}

fn seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Result<Vec<u8>, GodarkError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .encrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|_| GodarkError::Encryption("AES-GCM seal failed".into()))
}

fn open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, GodarkError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| GodarkError::Encryption("AES-GCM open failed".into()))
}

/// Application keys after HPKE export.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct SealedSession {
    k_c2s: [u8; KEY_LEN],
    k_s2c: [u8; KEY_LEN],
}

impl SealedSession {
    fn from_exported_keys(k_c2s: [u8; KEY_LEN], k_s2c: [u8; KEY_LEN]) -> Self {
        Self { k_c2s, k_s2c }
    }

    pub fn seal_c2s(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, GodarkError> {
        seal(&self.k_c2s, nonce, aad, plaintext)
    }

    pub fn open_c2s(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, GodarkError> {
        open(&self.k_c2s, nonce, aad, ciphertext)
    }

    pub fn seal_s2c(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        plaintext: &[u8],
    ) -> Result<Vec<u8>, GodarkError> {
        seal(&self.k_s2c, nonce, aad, plaintext)
    }

    pub fn open_s2c(
        &self,
        nonce: &[u8; 12],
        aad: &[u8],
        ciphertext: &[u8],
    ) -> Result<Vec<u8>, GodarkError> {
        open(&self.k_s2c, nonce, aad, ciphertext)
    }
}

impl std::fmt::Debug for SealedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SealedSession").finish_non_exhaustive()
    }
}

/// Sequencer static recipient keypair (tests / mock edge).
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct StaticKeyPair {
    private: [u8; KEY_LEN],
    #[zeroize(skip)]
    public: [u8; KEY_LEN],
}

impl StaticKeyPair {
    pub fn generate() -> Result<Self, GodarkError> {
        let (sk, pk) = Kem::gen_keypair();
        let mut private = [0u8; KEY_LEN];
        let mut public = [0u8; KEY_LEN];
        private.copy_from_slice(&sk.to_bytes());
        public.copy_from_slice(&pk.to_bytes());
        Ok(Self { private, public })
    }

    pub fn public_key(&self) -> &[u8; KEY_LEN] {
        &self.public
    }

    fn kem_private(&self) -> Result<<Kem as KemTrait>::PrivateKey, GodarkError> {
        <Kem as KemTrait>::PrivateKey::from_bytes(&self.private)
            .map_err(|e| GodarkError::Encryption(format!("HPKE private key: {e}")))
    }
}

/// Client (initiator): encapsulate to sequencer pubkey.
pub fn setup_session(
    recipient_public: &[u8; KEY_LEN],
    info: &[u8],
) -> Result<(Vec<u8>, SealedSession), GodarkError> {
    let pk = <Kem as KemTrait>::PublicKey::from_bytes(recipient_public)
        .map_err(|e| GodarkError::Encryption(format!("HPKE public key: {e}")))?;
    let (encapped, ctx) = setup_sender::<AeadAlg, Kdf, Kem>(&OpModeS::Base, &pk, info)
        .map_err(|e| GodarkError::Encryption(format!("HPKE setup_sender: {e}")))?;
    let enc = encapped.to_bytes().to_vec();
    let mut k_c2s = [0u8; KEY_LEN];
    let mut k_s2c = [0u8; KEY_LEN];
    ctx.export(EXPORT_C2S, &mut k_c2s)
        .map_err(|e| GodarkError::Encryption(format!("HPKE export c2s: {e}")))?;
    ctx.export(EXPORT_S2C, &mut k_s2c)
        .map_err(|e| GodarkError::Encryption(format!("HPKE export s2c: {e}")))?;
    Ok((enc, SealedSession::from_exported_keys(k_c2s, k_s2c)))
}

/// Sequencer (recipient): open encapped key with static private key.
pub fn open_session(
    recipient: &StaticKeyPair,
    encapped_key: &[u8],
    info: &[u8],
) -> Result<SealedSession, GodarkError> {
    if encapped_key.len() != ENCAPPED_KEY_LEN {
        return Err(GodarkError::Encryption(format!(
            "encapped key must be {ENCAPPED_KEY_LEN} bytes, got {}",
            encapped_key.len()
        )));
    }
    let sk = recipient.kem_private()?;
    let enc = <Kem as KemTrait>::EncappedKey::from_bytes(encapped_key)
        .map_err(|e| GodarkError::Encryption(format!("HPKE encapped key: {e}")))?;
    let ctx = setup_receiver::<AeadAlg, Kdf, Kem>(&OpModeR::Base, &sk, &enc, info)
        .map_err(|e| GodarkError::Encryption(format!("HPKE setup_receiver: {e}")))?;
    let mut k_c2s = [0u8; KEY_LEN];
    let mut k_s2c = [0u8; KEY_LEN];
    ctx.export(EXPORT_C2S, &mut k_c2s)
        .map_err(|e| GodarkError::Encryption(format!("HPKE export c2s: {e}")))?;
    ctx.export(EXPORT_S2C, &mut k_s2c)
        .map_err(|e| GodarkError::Encryption(format!("HPKE export s2c: {e}")))?;
    Ok(SealedSession::from_exported_keys(k_c2s, k_s2c))
}

pub fn parse_pinned_static_public_key(hex_str: &str) -> Result<[u8; KEY_LEN], GodarkError> {
    let bytes = hex::decode(hex_str.trim())
        .map_err(|e| GodarkError::Config(format!("HPKE static public key must be hex: {e}")))?;
    if bytes.len() != KEY_LEN {
        return Err(GodarkError::Config(format!(
            "HPKE static public key must be {KEY_LEN} bytes, got {}",
            bytes.len()
        )));
    }
    let mut out = [0u8; KEY_LEN];
    out.copy_from_slice(&bytes);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn setup_open_roundtrip_and_seal() {
        let seq = StaticKeyPair::generate().unwrap();
        let user = Uuid::from_u128(7);
        let info = info_for_conn(user, 42);
        let (enc, client) = setup_session(seq.public_key(), &info).unwrap();
        assert_eq!(enc.len(), ENCAPPED_KEY_LEN);
        let server = open_session(&seq, &enc, &info).unwrap();

        let nonce = nonce_from_u64(1);
        let ct = client.seal_c2s(&nonce, b"aad", b"place").unwrap();
        assert_eq!(server.open_c2s(&nonce, b"aad", &ct).unwrap(), b"place");

        let nonce2 = nonce_from_u64(2);
        let ct2 = server.seal_s2c(&nonce2, b"rh", b"ack").unwrap();
        assert_eq!(client.open_s2c(&nonce2, b"rh", &ct2).unwrap(), b"ack");
    }
}
