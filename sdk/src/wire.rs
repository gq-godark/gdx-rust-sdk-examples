//! Trading WebSocket binary frames (`TradingWsBinaryFrame`).

use prost::Message;

use crate::error::GodarkError;
use crate::generated::edge::v1 as edge;
use crate::hpke::WIRE_VERSION;

pub fn encode_hpke_setup(user_uuid: &[u8], conn_id: u64, encapped_key: &[u8]) -> Vec<u8> {
    let frame = edge::TradingWsBinaryFrame {
        subscription_epoch: 0,
        stream_seq: 0,
        body: Some(edge::trading_ws_binary_frame::Body::HpkeSetup(
            edge::HpkeSetup {
                user_uuid: user_uuid.to_vec(),
                conn_id,
                encapped_key: encapped_key.to_vec(),
            },
        )),
    };
    frame.encode_to_vec()
}

pub fn encode_encrypted_order(req: edge::EncryptedEdgeRequest) -> Vec<u8> {
    let frame = edge::TradingWsBinaryFrame {
        subscription_epoch: 0,
        stream_seq: 0,
        body: Some(edge::trading_ws_binary_frame::Body::EncryptedOrder(req)),
    };
    frame.encode_to_vec()
}

pub fn encrypted_order_request(
    header: edge::OrderHeader,
    encrypted_body: Vec<u8>,
) -> edge::EncryptedEdgeRequest {
    edge::EncryptedEdgeRequest {
        version: WIRE_VERSION,
        header: Some(header),
        encrypted_body,
    }
}

pub enum DecodedBinary {
    EncryptedPush(edge::EncryptedEdgeResponse),
    EncryptedOrder(edge::EncryptedEdgeRequest),
    HpkeSetup(edge::HpkeSetup),
    HpkeSetupReply { conn_id: u64, established: bool },
    Ignored,
}

pub fn encrypted_push_to_json(push: &edge::EncryptedEdgeResponse) -> Option<serde_json::Value> {
    let h = push.header.as_ref()?;
    let message_type = match h.message_type {
        1 => "order_update",
        2 => "system_health",
        3 => "ack",
        4 => "open_orders_snapshot",
        5 => "positions_snapshot",
        6 => "balance_and_position",
        7 => "account_margin_update",
        8 => "mass_quote_ack",
        9 => "batch_cancel_ack",
        10 => "batch_modify_ack",
        11 => "tpsl_update",
        12 => "leverage_settings",
        16 => "tpsl_ack",
        _ => "unknown",
    };
    let corr = if h.correlation_id.is_empty() {
        serde_json::Value::Null
    } else {
        let mut buf = [0u8; 16];
        let n = h.correlation_id.len().min(16);
        buf[16 - n..].copy_from_slice(&h.correlation_id[..n]);
        serde_json::Value::String(format!("{:032x}", u128::from_be_bytes(buf)))
    };
    Some(serde_json::json!({
        "type": "encrypted_push",
        "message_type": message_type,
        "encrypted_body": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &push.encrypted_body),
        "nonce": h.nonce,
        "fencing_epoch": h.fencing_epoch,
        "correlation_id": corr,
        "session_seq": h.session_seq,
        "conn_id": h.conn_id,
        "body_length": h.body_length,
    }))
}

pub fn decode_binary_frame(bytes: &[u8]) -> Result<DecodedBinary, GodarkError> {
    let frame = match edge::TradingWsBinaryFrame::decode(bytes) {
        Ok(frame) => frame,
        Err(_) => return Ok(DecodedBinary::Ignored),
    };
    Ok(match frame.body {
        Some(edge::trading_ws_binary_frame::Body::EncryptedPush(push)) => {
            DecodedBinary::EncryptedPush(push)
        }
        Some(edge::trading_ws_binary_frame::Body::HpkeSetupReply(r)) => {
            DecodedBinary::HpkeSetupReply {
                conn_id: r.conn_id,
                established: r.established,
            }
        }
        Some(edge::trading_ws_binary_frame::Body::EncryptedOrder(req)) => {
            DecodedBinary::EncryptedOrder(req)
        }
        Some(edge::trading_ws_binary_frame::Body::HpkeSetup(setup)) => {
            DecodedBinary::HpkeSetup(setup)
        }
        _ => DecodedBinary::Ignored,
    })
}

pub fn encode_hpke_setup_reply(conn_id: u64, established: bool) -> Vec<u8> {
    let frame = edge::TradingWsBinaryFrame {
        subscription_epoch: 0,
        stream_seq: 0,
        body: Some(edge::trading_ws_binary_frame::Body::HpkeSetupReply(
            edge::HpkeSetupReply {
                conn_id,
                established,
            },
        )),
    };
    frame.encode_to_vec()
}

pub fn encode_encrypted_push(resp: edge::EncryptedEdgeResponse) -> Vec<u8> {
    let frame = edge::TradingWsBinaryFrame {
        subscription_epoch: 0,
        stream_seq: 0,
        body: Some(edge::trading_ws_binary_frame::Body::EncryptedPush(resp)),
    };
    frame.encode_to_vec()
}
