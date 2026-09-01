// Protobuf builders and parsers — mirrors Python SDK _proto.py

use prost::Message;
use uuid::Uuid;

use crate::enums::{
    self, CancelReason, OrderStatus, OrderType, OrderUpdateType, Side, TimeInForce,
};
use crate::error::GodarkError;
use crate::generated::edge::v1 as edge;
use crate::generated::health::v1 as health;
use crate::generated::sequencer::v1 as sequencer;
use crate::types::{
    AccountMarginSummary, AccountMarginUpdate, BalanceUpdate, CountAck, FundingRateUpdate,
    LeverageSetting, LeverageSettings, OpenOrderRow, OpenOrdersSnapshot, OrderUpdate,
    PlaceOrderOptions, PositionRow, PositionsSnapshot, PositionsSnapshotSource, SystemHealthUpdate,
    TpslAck,
};

/// Encode a correlation id (16 raw UUID bytes, big-endian layout) as the
/// little-endian u128 bytes used in `EdgeSequencerRequest` bodies. Matches
/// `gdx_wire::convert::correlation_id_to_bytes` (canonical LE encoding).
fn correlation_id_body_bytes(raw: &[u8]) -> Vec<u8> {
    correlation_id_to_u128_be(raw).to_le_bytes().to_vec()
}

/// Interpret 16 raw UUID bytes (big-endian layout) as a u128. Used for the
/// AAD/header path and as the source value for `correlation_id_body_bytes`.
fn correlation_id_to_u128_be(raw: &[u8]) -> u128 {
    if raw.is_empty() {
        return 0;
    }
    let mut buf = [0u8; 16];
    let len = raw.len().min(16);
    buf[16 - len..].copy_from_slice(&raw[..len]);
    u128::from_be_bytes(buf)
}

/// Decode a correlation id from an `EdgeSequencerRequest`/response body, which
/// carries it as little-endian u128 bytes (see `correlation_id_body_bytes`).
fn correlation_id_to_u128(raw: &[u8]) -> u128 {
    if raw.is_empty() {
        return 0;
    }
    let mut buf = [0u8; 16];
    let len = raw.len().min(16);
    buf[..len].copy_from_slice(&raw[..len]);
    u128::from_le_bytes(buf)
}

fn uuid_from_bytes(raw: &[u8]) -> Uuid {
    if raw.len() == 16 {
        Uuid::from_bytes(raw.try_into().unwrap())
    } else {
        Uuid::nil()
    }
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
pub fn build_place_order_proto(
    symbol_id: u64,
    side: Side,
    order_type: crate::enums::OrderType,
    quantity: f64,
    user_uuid: &[u8],
    price: Option<f64>,
    time_in_force: crate::enums::TimeInForce,
    aon: bool,
    min_fill_size: Option<f64>,
    expiry_time: Option<u64>,
    correlation_id_bytes: &[u8],
    options: PlaceOrderOptions,
    _timestamp: u64,
) -> Vec<u8> {
    let min_fill_size = if aon && min_fill_size.is_none() {
        Some(quantity)
    } else {
        min_fill_size
    };
    let place = sequencer::PlaceOrderInput {
        symbol_id,
        side: side.to_proto(),
        order_type: order_type.to_proto(),
        quantity,
        time_in_force: time_in_force.to_proto(),
        price,
        min_fill_size,
        expiry_time,
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
        user_uuid: user_uuid.to_vec(),
        stp_mode: options.stp_mode.to_proto(),
        post_only: options.post_only,
        reduce_only: options.reduce_only,
        stop_loss_price: options.stop_loss_price,
        take_profit_price: options.take_profit_price,
        peg_offset_bps: options.peg_offset_bps,
        trigger_price: options.trigger_price,
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::Place(place)),
    };
    req.encode_to_vec()
}

pub fn build_cancel_order_proto(
    order_id: u64,
    _user_uuid: &[u8],
    symbol_id: u64,
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let cancel = sequencer::CancelOrderInput {
        order_id,
        symbol_id,
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
        user_uuid: _user_uuid.to_vec(),
        cancel_reason: None,
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::Cancel(cancel)),
    };
    req.encode_to_vec()
}

pub fn build_cancel_all_proto(
    symbol_id: Option<u64>,
    user_uuid: &[u8],
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let cancel_all = sequencer::CancelAllInput {
        symbol_id,
        user_uuid: user_uuid.to_vec(),
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
    };
    encode_edge_request(sequencer::edge_sequencer_request::Inner::CancelAll(
        cancel_all,
    ))
}

pub fn build_close_all_proto(
    symbol_id: Option<u64>,
    user_uuid: &[u8],
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let close_all = sequencer::CloseAllInput {
        symbol_id,
        user_uuid: user_uuid.to_vec(),
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
    };
    encode_edge_request(sequencer::edge_sequencer_request::Inner::CloseAll(
        close_all,
    ))
}

pub fn build_reverse_proto(
    symbol_id: u64,
    user_uuid: &[u8],
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let reverse = sequencer::ReverseInput {
        symbol_id,
        user_uuid: user_uuid.to_vec(),
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
    };
    encode_edge_request(sequencer::edge_sequencer_request::Inner::Reverse(reverse))
}

pub fn build_amend_tpsl_proto(
    user_uuid: &[u8],
    order_id: u64,
    correlation_id_bytes: &[u8],
    take_profit_price: Option<f64>,
    stop_loss_price: Option<f64>,
    symbol_id: Option<u64>,
    position_side: Option<crate::enums::Side>,
) -> Vec<u8> {
    let amend = sequencer::AmendTpslRequest {
        user_uuid: user_uuid.to_vec(),
        order_id,
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
        take_profit_price,
        stop_loss_price,
        symbol_id,
        position_side: position_side.map(crate::enums::Side::to_proto),
    };
    encode_edge_request(sequencer::edge_sequencer_request::Inner::AmendTpsl(amend))
}

pub fn build_cancel_tpsl_proto(
    user_uuid: &[u8],
    order_id: u64,
    correlation_id_bytes: &[u8],
    symbol_id: Option<u64>,
    position_side: Option<crate::enums::Side>,
) -> Vec<u8> {
    let cancel = sequencer::CancelTpslRequest {
        user_uuid: user_uuid.to_vec(),
        order_id,
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
        symbol_id,
        position_side: position_side.map(crate::enums::Side::to_proto),
    };
    encode_edge_request(sequencer::edge_sequencer_request::Inner::CancelTpsl(cancel))
}

pub fn build_modify_order_proto(
    order_id: u64,
    user_uuid: &[u8],
    symbol_id: u64,
    new_price: Option<f64>,
    new_quantity: Option<f64>,
    new_trigger_price: Option<f64>,
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let modify = sequencer::ModifyOrderInput {
        order_id,
        symbol_id,
        new_price,
        new_quantity,
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
        user_uuid: user_uuid.to_vec(),
        new_trigger_price,
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::Modify(modify)),
    };
    req.encode_to_vec()
}

pub fn build_update_leverage_proto(
    user_uuid: &[u8],
    symbol_id: u64,
    leverage: u32,
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let leverage = leverage.max(1);
    let update = sequencer::UpdateLeverageRequest {
        user_uuid: user_uuid.to_vec(),
        symbol_id,
        leverage,
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::UpdateLeverage(
            update,
        )),
    };
    req.encode_to_vec()
}

fn encode_edge_request(inner: sequencer::edge_sequencer_request::Inner) -> Vec<u8> {
    sequencer::EdgeSequencerRequest { inner: Some(inner) }.encode_to_vec()
}

fn user_corr_body(user_uuid: &[u8], correlation_id_bytes: &[u8]) -> (Vec<u8>, Vec<u8>) {
    (
        user_uuid.to_vec(),
        correlation_id_body_bytes(correlation_id_bytes),
    )
}

pub fn build_get_open_orders_proto(user_uuid: &[u8], correlation_id_bytes: &[u8]) -> Vec<u8> {
    let (user_uuid, correlation_id) = user_corr_body(user_uuid, correlation_id_bytes);
    encode_edge_request(sequencer::edge_sequencer_request::Inner::GetOpenOrders(
        sequencer::GetOpenOrdersRequest {
            user_uuid,
            correlation_id,
        },
    ))
}

pub fn build_get_positions_proto(user_uuid: &[u8], correlation_id_bytes: &[u8]) -> Vec<u8> {
    let (user_uuid, correlation_id) = user_corr_body(user_uuid, correlation_id_bytes);
    encode_edge_request(sequencer::edge_sequencer_request::Inner::GetPositions(
        sequencer::GetPositionsRequest {
            user_uuid,
            correlation_id,
        },
    ))
}

pub fn build_get_account_proto(user_uuid: &[u8], correlation_id_bytes: &[u8]) -> Vec<u8> {
    let (user_uuid, correlation_id) = user_corr_body(user_uuid, correlation_id_bytes);
    encode_edge_request(sequencer::edge_sequencer_request::Inner::GetAccount(
        sequencer::GetAccountRequest {
            user_uuid,
            correlation_id,
        },
    ))
}

/// Build a `MassQuoteInput` (bulk cancel-replace) wrapped in an
/// `EdgeSequencerRequest`. Each leg becomes its own order and carries a unique
/// 16-byte correlation id (the wire requires exactly 16 bytes per leg).
pub fn build_mass_quote_proto(
    symbol_id: u64,
    user_uuid: &[u8],
    legs: &[crate::types::MassQuoteLegInput],
    correlation_id_bytes: &[u8],
    post_only: Option<bool>,
) -> Vec<u8> {
    let pb_legs = legs
        .iter()
        .map(|leg| {
            let tif = leg.time_in_force.unwrap_or(crate::enums::TimeInForce::Gtc);
            sequencer::MassQuoteLeg {
                // 0 means "pure place" (no cancel target).
                cancel_order_id: leg.cancel_order_id.unwrap_or(0),
                side: leg.side.to_proto(),
                price: leg.price,
                quantity: leg.quantity,
                time_in_force: tif.to_proto(),
                expiry_time: leg.expiry_time,
                correlation_id: Uuid::new_v4().into_bytes().to_vec(),
            }
        })
        .collect();
    let mq = sequencer::MassQuoteInput {
        symbol_id,
        legs: pb_legs,
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
        user_uuid: user_uuid.to_vec(),
        stp_mode: 0,
        // Sequencer requires post_only on the wire; default to post-only when unset.
        // Some(false) enables the relaxed path where a crossing leg takes liquidity.
        post_only: Some(post_only.unwrap_or(true)),
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::MassQuote(mq)),
    };
    req.encode_to_vec()
}

/// Build a `BatchCancelInput` (cancel up to 20 resting orders on one symbol)
/// wrapped in an `EdgeSequencerRequest`.
pub fn build_batch_cancel_proto(
    symbol_id: u64,
    user_uuid: &[u8],
    order_ids: &[u64],
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let bc = sequencer::BatchCancelInput {
        symbol_id,
        order_ids: order_ids.to_vec(),
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
        user_uuid: user_uuid.to_vec(),
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::BatchCancel(bc)),
    };
    req.encode_to_vec()
}

/// Build a `BatchModifyInput` (post-only amend up to 20 resting orders on one
/// symbol) wrapped in an `EdgeSequencerRequest`. Each leg carries a unique
/// 16-byte correlation id.
pub fn build_batch_modify_proto(
    symbol_id: u64,
    user_uuid: &[u8],
    legs: &[crate::types::BatchModifyLegInput],
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let pb_legs = legs
        .iter()
        .map(|leg| sequencer::BatchModifyLeg {
            order_id: leg.order_id,
            new_price: leg.new_price,
            new_quantity: leg.new_quantity,
            correlation_id: Uuid::new_v4().into_bytes().to_vec(),
        })
        .collect();
    let bm = sequencer::BatchModifyInput {
        symbol_id,
        legs: pb_legs,
        correlation_id: correlation_id_body_bytes(correlation_id_bytes),
        user_uuid: user_uuid.to_vec(),
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::BatchModify(bm)),
    };
    req.encode_to_vec()
}

fn mass_quote_leg_status_str(status: i32) -> &'static str {
    match sequencer::MassQuoteLegStatus::try_from(status) {
        Ok(sequencer::MassQuoteLegStatus::Open) => "open",
        Ok(sequencer::MassQuoteLegStatus::Filled) => "filled",
        Ok(sequencer::MassQuoteLegStatus::Failed) => "failed",
        Ok(sequencer::MassQuoteLegStatus::Unspecified) => "unspecified",
        Err(_) => "unknown",
    }
}

fn mass_quote_ack_from_proto(ack: sequencer::MassQuoteAck) -> crate::types::MassQuoteAck {
    let results: Vec<crate::types::MassQuoteLegResult> = ack
        .results
        .into_iter()
        .map(|r| crate::types::MassQuoteLegResult {
            leg_index: r.leg_index,
            status: mass_quote_leg_status_str(r.status).to_string(),
            cancelled_order_id: (r.cancelled_order_id != 0)
                .then(|| r.cancelled_order_id.to_string()),
            new_order_id: (r.new_order_id != 0).then(|| r.new_order_id.to_string()),
            error_code: r.error_code,
            fill_count: r.fill_count,
        })
        .collect();
    let success = !results.is_empty() && results.iter().all(|r| r.status != "failed");
    crate::types::MassQuoteAck {
        success,
        sequence: ack.sequence.to_string(),
        results,
    }
}

fn batch_cancel_ack_from_proto(ack: sequencer::BatchCancelAck) -> crate::types::BatchCancelAck {
    let results: Vec<crate::types::BatchCancelLegResult> = ack
        .results
        .into_iter()
        .map(|r| crate::types::BatchCancelLegResult {
            order_id: r.order_id.to_string(),
            cancelled: r.cancelled,
            error_code: r.error_code,
        })
        .collect();
    let success = !results.is_empty() && results.iter().all(|r| r.cancelled);
    crate::types::BatchCancelAck {
        success,
        sequence: ack.sequence.to_string(),
        results,
    }
}

fn batch_modify_ack_from_proto(ack: sequencer::BatchModifyAck) -> crate::types::BatchModifyAck {
    let results: Vec<crate::types::BatchModifyLegResult> = ack
        .results
        .into_iter()
        .map(|r| crate::types::BatchModifyLegResult {
            order_id: r.order_id.to_string(),
            modified: r.modified,
            error_code: r.error_code,
        })
        .collect();
    let success = !results.is_empty() && results.iter().all(|r| r.modified);
    crate::types::BatchModifyAck {
        success,
        sequence: ack.sequence.to_string(),
        results,
    }
}

/// Decode a MassQuoteAck (legacy NodeResponse wrapper or direct message).
pub fn parse_mass_quote_ack(data: &[u8]) -> Result<crate::types::MassQuoteAck, GodarkError> {
    let (variant, payload) = resolve_rest_payload(data, Some("mass_quote_ack"));
    if variant != "mass_quote_ack" {
        return Err(GodarkError::Order {
            message: format!("Expected mass_quote_ack, got {variant}"),
            error_code: None,
        });
    }
    let ack = sequencer::MassQuoteAck::decode(payload.as_slice())
        .map_err(|e| GodarkError::Encryption(format!("decode MassQuoteAck: {e}")))?;
    let results: Vec<crate::types::MassQuoteLegResult> = ack
        .results
        .into_iter()
        .map(|r| crate::types::MassQuoteLegResult {
            leg_index: r.leg_index,
            status: mass_quote_leg_status_str(r.status).to_string(),
            cancelled_order_id: (r.cancelled_order_id != 0)
                .then(|| r.cancelled_order_id.to_string()),
            new_order_id: (r.new_order_id != 0).then(|| r.new_order_id.to_string()),
            error_code: r.error_code,
            fill_count: r.fill_count,
        })
        .collect();
    let success = !results.is_empty() && results.iter().all(|r| r.status != "failed");
    Ok(crate::types::MassQuoteAck {
        success,
        sequence: ack.sequence.to_string(),
        results,
    })
}

/// Decode a BatchCancelAck (legacy NodeResponse wrapper or direct message).
pub fn parse_batch_cancel_ack(data: &[u8]) -> Result<crate::types::BatchCancelAck, GodarkError> {
    let (variant, payload) = resolve_rest_payload(data, Some("batch_cancel_ack"));
    if variant != "batch_cancel_ack" {
        return Err(GodarkError::Order {
            message: format!("Expected batch_cancel_ack, got {variant}"),
            error_code: None,
        });
    }
    let ack = sequencer::BatchCancelAck::decode(payload.as_slice())
        .map_err(|e| GodarkError::Encryption(format!("decode BatchCancelAck: {e}")))?;
    let results: Vec<crate::types::BatchCancelLegResult> = ack
        .results
        .into_iter()
        .map(|r| crate::types::BatchCancelLegResult {
            order_id: r.order_id.to_string(),
            cancelled: r.cancelled,
            error_code: r.error_code,
        })
        .collect();
    let success = !results.is_empty() && results.iter().all(|r| r.cancelled);
    Ok(crate::types::BatchCancelAck {
        success,
        sequence: ack.sequence.to_string(),
        results,
    })
}

/// Decode a BatchModifyAck (legacy NodeResponse wrapper or direct message).
pub fn parse_batch_modify_ack(data: &[u8]) -> Result<crate::types::BatchModifyAck, GodarkError> {
    let (variant, payload) = resolve_rest_payload(data, Some("batch_modify_ack"));
    if variant != "batch_modify_ack" {
        return Err(GodarkError::Order {
            message: format!("Expected batch_modify_ack, got {variant}"),
            error_code: None,
        });
    }
    let ack = sequencer::BatchModifyAck::decode(payload.as_slice())
        .map_err(|e| GodarkError::Encryption(format!("decode BatchModifyAck: {e}")))?;
    let results: Vec<crate::types::BatchModifyLegResult> = ack
        .results
        .into_iter()
        .map(|r| crate::types::BatchModifyLegResult {
            order_id: r.order_id.to_string(),
            modified: r.modified,
            error_code: r.error_code,
        })
        .collect();
    let success = !results.is_empty() && results.iter().all(|r| r.modified);
    Ok(crate::types::BatchModifyAck {
        success,
        sequence: ack.sequence.to_string(),
        results,
    })
}

fn read_varint(data: &[u8], mut i: usize) -> Result<(u64, usize), GodarkError> {
    let mut shift = 0u32;
    let mut result = 0u64;
    while i < data.len() {
        let b = data[i];
        i += 1;
        result |= u64::from(b & 0x7F) << shift;
        if b & 0x80 == 0 {
            return Ok((result, i));
        }
        shift += 7;
        if shift >= 64 {
            return Err(GodarkError::Encryption("varint overflow".into()));
        }
    }
    Err(GodarkError::Encryption("truncated varint".into()))
}

fn write_varint(mut value: u64) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut b = (value & 0x7F) as u8;
        value >>= 7;
        if value != 0 {
            b |= 0x80;
        }
        out.push(b);
        if value == 0 {
            break;
        }
    }
    out
}

fn legacy_node_response_field_num(variant: &str) -> Option<u32> {
    match variant {
        "ack" => Some(1),
        "fill" => Some(2),
        "open_orders_snapshot" => Some(3),
        "node_ready" => Some(4),
        "mass_quote_ack" => Some(5),
        "batch_cancel_ack" => Some(6),
        "batch_modify_ack" => Some(7),
        "positions_snapshot" => Some(8),
        "account_margin_update" => Some(9),
        "cancel_all_ack" => Some(10),
        "close_all_ack" => Some(11),
        "reverse_ack" => Some(12),
        _ => None,
    }
}

fn legacy_node_response_field_name(field_num: u32) -> Option<&'static str> {
    match field_num {
        1 => Some("ack"),
        2 => Some("fill"),
        3 => Some("open_orders_snapshot"),
        4 => Some("node_ready"),
        5 => Some("mass_quote_ack"),
        6 => Some("batch_cancel_ack"),
        7 => Some("batch_modify_ack"),
        8 => Some("positions_snapshot"),
        9 => Some("account_margin_update"),
        10 => Some("cancel_all_ack"),
        11 => Some("close_all_ack"),
        12 => Some("reverse_ack"),
        _ => None,
    }
}

pub fn wrap_legacy_node_response(variant: &str, inner: &[u8]) -> Vec<u8> {
    let field_num = legacy_node_response_field_num(variant).expect("known legacy variant");
    let mut out = vec![((field_num << 3) | 2) as u8];
    out.extend(write_varint(inner.len() as u64));
    out.extend_from_slice(inner);
    out
}

fn unwrap_legacy_node_response(data: &[u8]) -> Option<(String, Vec<u8>)> {
    if data.is_empty() {
        return None;
    }
    let tag = data[0];
    let wire_type = tag & 0x07;
    let field_num = u32::from(tag >> 3);
    let variant = legacy_node_response_field_name(field_num)?.to_string();
    if wire_type != 2 {
        return None;
    }
    let (length, i) = read_varint(data, 1).ok()?;
    let end = i + length as usize;
    if end != data.len() {
        return None;
    }
    Some((variant, data[i..end].to_vec()))
}

fn is_direct_hotpath_count_ack(expected: Option<&str>) -> bool {
    matches!(
        expected,
        Some("cancel_all_ack") | Some("close_all_ack") | Some("reverse_ack")
    )
}

fn resolve_rest_payload(data: &[u8], expected: Option<&str>) -> (String, Vec<u8>) {
    // Hotpath count acks are usually direct protobuf; field 3 collides with legacy snapshot wrap.
    if is_direct_hotpath_count_ack(expected) {
        let expected = expected.unwrap();
        if let Some((variant, inner)) = unwrap_legacy_node_response(data) {
            if variant == expected {
                return (variant, inner);
            }
        }
        return (expected.to_string(), data.to_vec());
    }
    if let Some((variant, inner)) = unwrap_legacy_node_response(data) {
        return (variant, inner);
    }
    if let Some(variant) = expected {
        return (variant.to_string(), data.to_vec());
    }
    ("ack".to_string(), data.to_vec())
}

fn ack_from_proto(ack: sequencer::AckMessage) -> NodeResponseKind {
    let (success, error_code, order_status) = match ack.ack_outcome {
        Some(o) => {
            let mut success = o.kind == sequencer::AckOutcomeKind::Applied as i32;
            let error_code = o.business_error_code.or(o.system_error_code);
            if error_code.is_some() {
                success = false;
            }
            (
                success,
                error_code,
                o.order_status.map(OrderStatus::from_proto),
            )
        }
        None => (false, None, None),
    };
    NodeResponseKind::Ack {
        sequence: ack.sequence,
        order_id: ack.order_id,
        success,
        error_code,
        reject_text: ack.reject_text,
        correlation_id: ack.correlation_id,
        order_status,
    }
}

pub fn build_order_header_aad(
    user_uuid: &[u8],
    symbol_id: u64,
    request_type: &str,
    nonce: u64,
    body_length: u32,
    correlation_id: &[u8],
    conn_id: u64,
) -> Vec<u8> {
    let header = edge::OrderHeader {
        user_uuid: user_uuid.to_vec(),
        symbol_id,
        request_type: enums::request_type_to_proto(request_type),
        nonce,
        body_length,
        correlation_id: correlation_id.to_vec(),
        conn_id,
    };
    header.encode_to_vec()
}

#[allow(clippy::too_many_arguments)]
pub fn build_response_header_aad(
    user_uuid: &[u8],
    message_type: &str,
    body_length: u32,
    nonce: u64,
    fencing_epoch: u64,
    correlation_id: &[u8],
    session_seq: u64,
    conn_id: u64,
) -> Vec<u8> {
    let header = edge::ResponseHeader {
        user_uuid: user_uuid.to_vec(),
        message_type: enums::response_message_type_to_proto(message_type),
        body_length,
        nonce,
        fencing_epoch,
        correlation_id: correlation_id.to_vec(),
        session_seq,
        conn_id,
    };
    header.encode_to_vec()
}

// ---------------------------------------------------------------------------
// Parsers
// ---------------------------------------------------------------------------

/// Parsed variant from a NodeResponse.
#[derive(Debug)]
pub enum NodeResponseKind {
    Ack {
        sequence: u64,
        order_id: u64,
        success: bool,
        error_code: Option<u32>,
        reject_text: Option<String>,
        correlation_id: Vec<u8>,
        order_status: Option<OrderStatus>,
    },
    Fill {
        trade_id: u64,
        taker_order_id: u64,
        maker_order_id: u64,
        symbol_id: u64,
        timestamp: u64,
        correlation_id: Vec<u8>,
    },
    Signing,
    OpenOrdersSnapshot(OpenOrdersSnapshot),
    PositionsSnapshot(PositionsSnapshot),
    AccountMarginUpdate(AccountMarginUpdate),
    MassQuoteAck(crate::types::MassQuoteAck),
    BatchCancelAck(crate::types::BatchCancelAck),
    BatchModifyAck(crate::types::BatchModifyAck),
    CountAck(CountAck),
    TpslAck(TpslAck),
    Unknown,
}

pub fn parse_node_response(data: &[u8]) -> Result<NodeResponseKind, GodarkError> {
    parse_node_response_with_expected(data, Some("ack"))
}

/// Decode REST snapshot/ack plaintext using the JSON ``message_type`` hint.
pub fn parse_node_response_with_expected(
    data: &[u8],
    expected: Option<&str>,
) -> Result<NodeResponseKind, GodarkError> {
    let expected = expected.map(|s| {
        let s = s.replace('-', "_");
        if s == "account_margin" || s == "account_update" {
            "account_margin_update".to_string()
        } else {
            s
        }
    });
    let (variant, payload) = resolve_rest_payload(data, expected.as_deref());
    match variant.as_str() {
        "ack" => {
            let ack = sequencer::AckMessage::decode(payload.as_slice())?;
            Ok(ack_from_proto(ack))
        }
        "fill" => {
            let fill = sequencer::TradeMessage::decode(payload.as_slice())?;
            Ok(NodeResponseKind::Fill {
                trade_id: fill.trade_id,
                taker_order_id: fill.taker_order_id,
                maker_order_id: fill.maker_order_id,
                symbol_id: fill.symbol_id,
                timestamp: fill.timestamp,
                correlation_id: fill.correlation_id,
            })
        }
        "open_orders_snapshot" => {
            let s = sequencer::OpenOrdersSnapshot::decode(payload.as_slice())?;
            Ok(NodeResponseKind::OpenOrdersSnapshot(
                parse_open_orders_snapshot(s),
            ))
        }
        "positions_snapshot" => {
            let s = sequencer::PositionsSnapshot::decode(payload.as_slice())?;
            Ok(NodeResponseKind::PositionsSnapshot(
                parse_positions_snapshot(s),
            ))
        }
        "account_margin_update" | "account_update" => {
            let s = sequencer::AccountMarginUpdate::decode(payload.as_slice())?;
            Ok(NodeResponseKind::AccountMarginUpdate(
                parse_account_margin_update(s),
            ))
        }
        "mass_quote_ack" => {
            let a = sequencer::MassQuoteAck::decode(payload.as_slice())?;
            Ok(NodeResponseKind::MassQuoteAck(mass_quote_ack_from_proto(a)))
        }
        "batch_cancel_ack" => {
            let a = sequencer::BatchCancelAck::decode(payload.as_slice())?;
            Ok(NodeResponseKind::BatchCancelAck(
                batch_cancel_ack_from_proto(a),
            ))
        }
        "batch_modify_ack" => {
            let a = sequencer::BatchModifyAck::decode(payload.as_slice())?;
            Ok(NodeResponseKind::BatchModifyAck(
                batch_modify_ack_from_proto(a),
            ))
        }
        "cancel_all_ack" => {
            let a = sequencer::CancelAllAck::decode(payload.as_slice())?;
            Ok(NodeResponseKind::CountAck(count_ack_from_cancel_all(a)))
        }
        "close_all_ack" => {
            let a = sequencer::CloseAllAck::decode(payload.as_slice())?;
            Ok(NodeResponseKind::CountAck(count_ack_from_close_all(a)))
        }
        "reverse_ack" => {
            let a = sequencer::ReverseAck::decode(payload.as_slice())?;
            Ok(NodeResponseKind::CountAck(count_ack_from_reverse(a)))
        }
        "tpsl_ack" => {
            let a = sequencer::TpslAck::decode(payload.as_slice())?;
            Ok(NodeResponseKind::TpslAck(tpsl_ack_from_proto(a)))
        }
        "node_ready" => Ok(NodeResponseKind::Unknown),
        _ => Ok(NodeResponseKind::Unknown),
    }
}

fn tpsl_ack_from_proto(ack: sequencer::TpslAck) -> TpslAck {
    TpslAck {
        parent_order_id: ack.parent_order_id.to_string(),
        take_profit: ack.take_profit,
        stop_loss: ack.stop_loss,
        error_code: ack.error_code,
        reject_text: ack.reject_text,
    }
}

/// Decode a `tpsl_ack` plaintext body.
pub fn parse_tpsl_ack(data: &[u8]) -> Result<TpslAck, GodarkError> {
    match parse_node_response_with_expected(data, Some("tpsl_ack"))? {
        NodeResponseKind::TpslAck(ack) => Ok(ack),
        NodeResponseKind::Ack {
            success: false,
            error_code,
            reject_text,
            ..
        } => Err(crate::order_error_code::make_order_error_from_code(
            error_code,
            reject_text.as_deref(),
        )),
        other => Err(GodarkError::Order {
            message: format!("Expected tpsl_ack, got {other:?}"),
            error_code: None,
        }),
    }
}

fn count_ack_from_cancel_all(ack: sequencer::CancelAllAck) -> CountAck {
    CountAck {
        sequence: ack.sequence.to_string(),
        count: ack.cancelled,
        order_ids: ack
            .cancelled_order_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
        error_code: ack.error_code,
        reject_text: ack.reject_text,
    }
}

fn count_ack_from_close_all(ack: sequencer::CloseAllAck) -> CountAck {
    CountAck {
        sequence: ack.sequence.to_string(),
        count: ack.closed,
        order_ids: ack
            .close_order_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
        error_code: ack.error_code,
        reject_text: ack.reject_text,
    }
}

fn count_ack_from_reverse(ack: sequencer::ReverseAck) -> CountAck {
    CountAck {
        sequence: ack.sequence.to_string(),
        count: ack.reversed,
        order_ids: ack
            .reverse_order_ids
            .into_iter()
            .map(|id| id.to_string())
            .collect(),
        error_code: ack.error_code,
        reject_text: ack.reject_text,
    }
}

/// Decode a `cancel_all_ack` / `close_all_ack` / `reverse_ack` plaintext body.
pub fn parse_count_ack(data: &[u8], expected: &str) -> Result<CountAck, GodarkError> {
    match parse_node_response_with_expected(data, Some(expected))? {
        NodeResponseKind::CountAck(ack) => Ok(ack),
        NodeResponseKind::Ack {
            success: false,
            error_code,
            reject_text,
            ..
        } => Err(crate::order_error_code::make_order_error_from_code(
            error_code,
            reject_text.as_deref(),
        )),
        other => Err(GodarkError::Order {
            message: format!("Expected count ack, got {other:?}"),
            error_code: None,
        }),
    }
}

pub fn parse_order_update(data: &[u8]) -> Result<OrderUpdate, GodarkError> {
    let msg = sequencer::OrderUpdateMessage::decode(data)?;
    Ok(OrderUpdate {
        order_id: msg.order_id.to_string(),
        user_uuid: uuid_from_bytes(&msg.user_uuid),
        symbol_id: msg.symbol_id,
        side: Side::from_proto(msg.side),
        status: OrderStatus::from_proto(msg.order_status),
        update_type: OrderUpdateType::from_proto(msg.message_type),
        price: msg.price,
        quantity: msg.quantity,
        filled_qty: msg.filled_qty,
        remaining_qty: msg.remaining_qty,
        cum_fill: msg.cum_fill,
        cancel_reason: msg.cancel_reason.and_then(CancelReason::from_proto),
        reject_reason: msg.reject_reason_code.map(|c: u32| c.to_string()),
        msg: msg.msg,
        reduce_only: msg.reduce_only,
        post_only: msg.post_only,
        correlation_id: correlation_id_to_u128(&msg.correlation_id),
        timestamp: msg.timestamp,
    })
}

/// Parsed message from sequencer→edge.
///
/// Each variant maps 1:1 to a `oneof inner` arm in
/// `gdx.sequencer.v1.SequencerToEdgeMessage`. The enum is non-exhaustive so
/// future variants added on the sequencer side won't break user `match`
/// statements; current callers should always include a `_` arm.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum EdgeMessage {
    OrderUpdate(OrderUpdate),
    PositionsSnapshot(PositionsSnapshot),
    SystemHealth(SystemHealthUpdate),
    BalanceUpdate(BalanceUpdate),
    FundingRateUpdate(FundingRateUpdate),
    AccountMarginUpdate(AccountMarginUpdate),
    BalanceAndPosition {
        balance: Option<BalanceUpdate>,
        positions: Option<PositionsSnapshot>,
    },
    LeverageSettings(LeverageSettings),
    /// Recognized proto variant that this SDK build doesn't decode.
    Unknown,
}

fn parse_positions_snapshot_source(value: i32) -> PositionsSnapshotSource {
    match value {
        1 => PositionsSnapshotSource::Initial,
        2 => PositionsSnapshotSource::Periodic,
        3 => PositionsSnapshotSource::Event,
        _ => PositionsSnapshotSource::Unspecified,
    }
}

pub fn parse_open_orders_snapshot(msg: sequencer::OpenOrdersSnapshot) -> OpenOrdersSnapshot {
    OpenOrdersSnapshot {
        rows: msg.rows.into_iter().map(parse_open_order_row).collect(),
        server_timestamp: msg.server_timestamp,
        correlation_id: correlation_id_to_u128(&msg.correlation_id),
    }
}

fn parse_open_order_row(row: sequencer::OpenOrderRow) -> OpenOrderRow {
    OpenOrderRow {
        order_id: row.order_id.to_string(),
        symbol_id: row.symbol_id,
        side: Side::from_proto(row.side),
        order_type: OrderType::from_proto(row.order_type),
        price: row.price,
        quantity: row.quantity,
        filled_qty: row.filled_qty,
        remaining_qty: row.remaining_qty,
        order_status: OrderStatus::from_proto(row.order_status),
        time_in_force: TimeInForce::from_proto(row.time_in_force),
        leverage: row.leverage,
        timestamp: row.timestamp,
        correlation_id: correlation_id_to_u128(&row.correlation_id),
        expiry_time: row.expiry_time,
        reduce_only: row.reduce_only,
        post_only: row.post_only,
        take_profit: row.take_profit,
        stop_loss: row.stop_loss,
    }
}

fn parse_position_row(row: sequencer::PositionRow) -> PositionRow {
    PositionRow {
        symbol_id: row.symbol_id,
        side: Side::from_proto(row.side),
        size: row.size,
        entry_price: row.entry_price,
        leverage: row.leverage,
        mark_price: row.mark_price,
        unrealized_pnl: row.unrealized_pnl,
        notional: row.notional,
        mark_publish_time_sec: row.mark_publish_time_sec,
    }
}

pub fn parse_positions_snapshot(msg: sequencer::PositionsSnapshot) -> PositionsSnapshot {
    PositionsSnapshot {
        user_uuid: uuid_from_bytes(&msg.user_uuid),
        rows: msg.rows.into_iter().map(parse_position_row).collect(),
        server_timestamp: msg.server_timestamp,
        source: parse_positions_snapshot_source(msg.source),
        correlation_id: msg.correlation_id.as_deref().map(correlation_id_to_u128),
    }
}

pub fn parse_system_health(msg: health::HealthReport) -> SystemHealthUpdate {
    SystemHealthUpdate {
        component_id: msg.component_id,
        state: msg.state,
        serving: msg.serving,
        cause: msg.cause,
        updated_at_nanos: msg.updated_at_nanos,
        sequence: msg.sequence,
        schema_version: msg.schema_version,
    }
}

pub fn parse_balance_update(msg: sequencer::BalanceUpdateMessage) -> BalanceUpdate {
    BalanceUpdate {
        user_uuid: uuid_from_bytes(&msg.user_uuid),
        balance_raw: msg.balance_raw,
        timestamp: msg.timestamp,
        balance: msg.balance,
        signed_balance_8dp: msg.signed_balance_8dp,
        free_collateral_8dp: msg.free_collateral_8dp,
    }
}

pub fn parse_funding_rate_update(msg: sequencer::FundingRateUpdateMessage) -> FundingRateUpdate {
    FundingRateUpdate {
        symbol_id: msg.symbol_id,
        funding_rate: msg.funding_rate.clone(),
        timestamp: msg.timestamp,
        last_funding_rate: msg.last_funding_rate.clone(),
    }
}

pub fn parse_funding_rate_snapshot_json(val: &serde_json::Value) -> Vec<FundingRateUpdate> {
    if val.get("type").and_then(|v| v.as_str()) != Some("funding_rate_snapshot") {
        return vec![];
    }
    let Some(rows) = val.get("rows").and_then(|v| v.as_array()) else {
        return vec![];
    };
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let Some(obj) = row.as_object() else {
            continue;
        };
        let funding_rate = obj
            .get("funding_rate")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if funding_rate.is_empty() {
            continue;
        }
        let symbol_id = obj.get("symbol_id").and_then(|v| v.as_u64()).unwrap_or(0);
        let last_funding_rate = obj
            .get("last_funding_rate")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let timestamp = obj.get("timestamp").and_then(|v| v.as_u64()).unwrap_or(0);
        out.push(FundingRateUpdate {
            symbol_id,
            funding_rate: funding_rate.to_string(),
            timestamp,
            last_funding_rate,
        });
    }
    out
}

pub fn parse_account_margin_update(msg: sequencer::AccountMarginUpdate) -> AccountMarginUpdate {
    AccountMarginUpdate {
        user_uuid: uuid_from_bytes(&msg.user_uuid),
        server_timestamp: msg.server_timestamp,
        account: msg.account.map(|a| AccountMarginSummary {
            total_collateral: a.total_collateral,
            position_margin: a.position_margin,
            reserved_order_margin: a.reserved_order_margin,
            free_collateral: a.free_collateral,
            isolated_margin: a.isolated_margin,
            isolated_equity: a.isolated_equity,
            cross_im: a.cross_im,
        }),
    }
}

pub fn parse_leverage_settings(msg: sequencer::LeverageSettings) -> LeverageSettings {
    LeverageSettings {
        settings: msg
            .settings
            .into_iter()
            .map(|row| LeverageSetting {
                symbol_id: row.symbol_id,
                leverage: row.leverage,
            })
            .collect(),
        user_uuid: (!msg.user_uuid.is_empty()).then(|| uuid_from_bytes(&msg.user_uuid)),
        server_timestamp: Some(msg.server_timestamp).filter(|ts| *ts != 0),
    }
}

pub fn parse_sequencer_to_edge_message(data: &[u8]) -> Result<EdgeMessage, GodarkError> {
    let msg = sequencer::SequencerToEdgeMessage::decode(data)?;
    match msg.inner {
        Some(sequencer::sequencer_to_edge_message::Inner::OrderUpdate(ou)) => {
            let bytes = ou.encode_to_vec();
            Ok(EdgeMessage::OrderUpdate(parse_order_update(&bytes)?))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::PositionsSnapshot(ps)) => {
            Ok(EdgeMessage::PositionsSnapshot(parse_positions_snapshot(ps)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::HealthReport(h)) => {
            Ok(EdgeMessage::SystemHealth(parse_system_health(h)))
        }

        Some(sequencer::sequencer_to_edge_message::Inner::BalanceUpdate(b)) => {
            Ok(EdgeMessage::BalanceUpdate(parse_balance_update(b)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::FundingRateUpdate(f)) => {
            Ok(EdgeMessage::FundingRateUpdate(parse_funding_rate_update(f)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::AccountMarginUpdate(a)) => Ok(
            EdgeMessage::AccountMarginUpdate(parse_account_margin_update(a)),
        ),
        Some(sequencer::sequencer_to_edge_message::Inner::BalanceAndPosition(bp)) => {
            Ok(EdgeMessage::BalanceAndPosition {
                balance: bp.bal_data.map(parse_balance_update),
                positions: bp.pos_data.map(parse_positions_snapshot),
            })
        }
        Some(sequencer::sequencer_to_edge_message::Inner::TpslUpdate(_))
        | Some(sequencer::sequencer_to_edge_message::Inner::InstrumentUpdate(_)) => {
            Ok(EdgeMessage::Unknown)
        }
        Some(sequencer::sequencer_to_edge_message::Inner::LeverageSettings(ls)) => {
            Ok(EdgeMessage::LeverageSettings(parse_leverage_settings(ls)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::OrderHistoryInsert(_))
        | Some(sequencer::sequencer_to_edge_message::Inner::OpenInterestUpdate(_))
        | Some(sequencer::sequencer_to_edge_message::Inner::FundingPayment(_)) => {
            Ok(EdgeMessage::Unknown)
        }
        None => Ok(EdgeMessage::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use prost::Message;
    use uuid::Uuid;

    use crate::enums::{CancelReason, OrderStatus, OrderType, OrderUpdateType, Side, TimeInForce};
    use crate::generated::health::v1 as health;
    use crate::generated::sequencer::v1 as sequencer;

    use super::*;

    const TEST_UUID: [u8; 16] = [
        0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44, 0x00,
        0x00,
    ];

    #[test]
    fn test_build_place_order_roundtrip() {
        let bytes = build_place_order_proto(
            42,
            Side::Buy,
            OrderType::Limit,
            10.5,
            &TEST_UUID,
            Some(1.25),
            TimeInForce::Gtc,
            true,
            Some(0.5),
            Some(999),
            &TEST_UUID,
            PlaceOrderOptions::default(),
            1_234_567_890,
        );
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let place = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::Place(p)) => p,
            other => panic!("expected Place, got {:?}", other),
        };
        assert_eq!(place.symbol_id, 42);
        assert_eq!(place.side, Side::Buy.to_proto());
        assert_eq!(place.order_type, OrderType::Limit.to_proto());
        assert_eq!(place.quantity, 10.5);
        assert_eq!(place.user_uuid, TEST_UUID.as_slice());
        assert_eq!(place.price, Some(1.25));
        assert_eq!(place.time_in_force, TimeInForce::Gtc.to_proto());
        assert_eq!(place.min_fill_size, Some(0.5));
        assert_eq!(place.expiry_time, Some(999));
        assert_eq!(
            place.correlation_id,
            u128::from_be_bytes(TEST_UUID).to_le_bytes()
        );
    }

    #[test]
    fn test_build_place_order_respects_options() {
        let options = PlaceOrderOptions {
            reduce_only: true,
            post_only: true,
            stp_mode: crate::enums::StpMode::CancelAggressor,
            peg_offset_bps: None,
            trigger_price: None,
            take_profit_price: None,
            stop_loss_price: None,
        };
        let bytes = build_place_order_proto(
            42,
            Side::Buy,
            OrderType::Limit,
            1.0,
            &TEST_UUID,
            Some(100.0),
            TimeInForce::Gtc,
            false,
            None,
            None,
            &TEST_UUID,
            options,
            0,
        );
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let place = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::Place(p)) => p,
            other => panic!("expected Place, got {:?}", other),
        };
        assert!(place.reduce_only);
        assert!(place.post_only);
        assert_eq!(
            place.stp_mode,
            crate::enums::StpMode::CancelAggressor.to_proto()
        );
    }

    #[test]
    fn test_build_place_order_peg_stop_and_tpsl_fields() {
        let options = PlaceOrderOptions {
            reduce_only: false,
            post_only: false,
            stp_mode: crate::enums::StpMode::Unspecified,
            peg_offset_bps: Some(12),
            trigger_price: Some(95.5),
            take_profit_price: Some(110.0),
            stop_loss_price: Some(90.0),
        };
        let bytes = build_place_order_proto(
            42,
            Side::Buy,
            OrderType::Peg,
            1.0,
            &TEST_UUID,
            None,
            TimeInForce::Gtc,
            false,
            None,
            None,
            &TEST_UUID,
            options,
            0,
        );
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let place = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::Place(p)) => p,
            other => panic!("expected Place, got {:?}", other),
        };
        assert_eq!(place.peg_offset_bps, Some(12));
        assert_eq!(place.trigger_price, Some(95.5));
        assert_eq!(place.take_profit_price, Some(110.0));
        assert_eq!(place.stop_loss_price, Some(90.0));
    }

    #[test]
    fn test_build_modify_order_new_trigger_price() {
        let bytes = build_modify_order_proto(7, &TEST_UUID, 9, None, None, Some(88.25), &TEST_UUID);
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let modify = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::Modify(m)) => m,
            other => panic!("expected Modify, got {:?}", other),
        };
        assert_eq!(modify.new_trigger_price, Some(88.25));
    }

    #[test]
    fn test_parse_cancel_all_ack_roundtrip() {
        let ack = sequencer::CancelAllAck {
            node_id: 1,
            sequence: 99,
            correlation_id: vec![],
            error_code: None,
            reject_text: None,
            cancelled: 2,
            cancelled_order_ids: vec![10, 20],
        };
        let wire = wrap_legacy_node_response("cancel_all_ack", &ack.encode_to_vec());
        let parsed = parse_count_ack(&wire, "cancel_all_ack").expect("parse");
        assert_eq!(parsed.sequence, "99");
        assert_eq!(parsed.count, 2);
        assert_eq!(parsed.order_ids, vec!["10".to_string(), "20".to_string()]);
    }

    #[test]
    fn test_build_cancel_all_roundtrip() {
        let bytes = build_cancel_all_proto(Some(7), &TEST_UUID, &TEST_UUID);
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let cancel_all = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::CancelAll(c)) => c,
            other => panic!("expected CancelAll, got {:?}", other),
        };
        assert_eq!(cancel_all.symbol_id, Some(7));
    }

    #[test]
    fn test_build_cancel_order_roundtrip() {
        let bytes = build_cancel_order_proto(10, &TEST_UUID, 30, &TEST_UUID);
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let cancel = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::Cancel(c)) => c,
            other => panic!("expected Cancel, got {:?}", other),
        };
        assert_eq!(cancel.order_id, 10);
        assert_eq!(cancel.symbol_id, 30);
        assert_eq!(
            cancel.correlation_id,
            u128::from_be_bytes(TEST_UUID).to_le_bytes()
        );
    }

    #[test]
    fn test_build_modify_order_roundtrip() {
        let bytes =
            build_modify_order_proto(7, &TEST_UUID, 9, Some(2.25), Some(3.5), None, &TEST_UUID);
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let modify = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::Modify(m)) => m,
            other => panic!("expected Modify, got {:?}", other),
        };
        assert_eq!(modify.order_id, 7);
        assert_eq!(modify.user_uuid, TEST_UUID.as_slice());
        assert_eq!(modify.symbol_id, 9);
        assert_eq!(modify.new_price, Some(2.25));
        assert_eq!(modify.new_quantity, Some(3.5));
        assert_eq!(
            modify.correlation_id,
            u128::from_be_bytes(TEST_UUID).to_le_bytes()
        );
    }

    type InnerUserCorr = fn(sequencer::edge_sequencer_request::Inner) -> Option<(Vec<u8>, Vec<u8>)>;

    fn assert_user_corr_rpc(bytes: &[u8], expected: InnerUserCorr) {
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes).expect("decode");
        let inner = decoded.inner.expect("inner");
        let (user_uuid, correlation_id) = expected(inner).expect("wrong inner");
        assert_eq!(user_uuid, TEST_UUID.as_slice());
        assert_eq!(correlation_id, u128::from_be_bytes(TEST_UUID).to_le_bytes());
    }

    #[test]
    fn test_build_get_snapshot_rpcs_roundtrip() {
        assert_user_corr_rpc(
            &build_get_open_orders_proto(&TEST_UUID, &TEST_UUID),
            |inner| match inner {
                sequencer::edge_sequencer_request::Inner::GetOpenOrders(r) => {
                    Some((r.user_uuid, r.correlation_id))
                }
                _ => None,
            },
        );
        assert_user_corr_rpc(
            &build_get_positions_proto(&TEST_UUID, &TEST_UUID),
            |inner| match inner {
                sequencer::edge_sequencer_request::Inner::GetPositions(r) => {
                    Some((r.user_uuid, r.correlation_id))
                }
                _ => None,
            },
        );
        assert_user_corr_rpc(
            &build_get_account_proto(&TEST_UUID, &TEST_UUID),
            |inner| match inner {
                sequencer::edge_sequencer_request::Inner::GetAccount(r) => {
                    Some((r.user_uuid, r.correlation_id))
                }
                _ => None,
            },
        );
    }

    #[test]
    fn test_build_update_leverage_roundtrip() {
        let bytes = build_update_leverage_proto(&TEST_UUID, 42, 5, &TEST_UUID);
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let update = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::UpdateLeverage(u)) => u,
            other => panic!("expected UpdateLeverage, got {:?}", other),
        };
        assert_eq!(update.user_uuid, TEST_UUID.as_slice());
        assert_eq!(update.symbol_id, 42);
        assert_eq!(update.leverage, 5);
        assert_eq!(
            update.correlation_id,
            u128::from_be_bytes(TEST_UUID).to_le_bytes()
        );
    }

    #[test]
    fn test_build_update_leverage_clamped_to_one() {
        let bytes = build_update_leverage_proto(&TEST_UUID, 1, 0, b"");
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let update = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::UpdateLeverage(u)) => u,
            other => panic!("expected UpdateLeverage, got {:?}", other),
        };
        assert_eq!(update.leverage, 1);
    }

    #[test]
    fn test_build_mass_quote_proto_roundtrip() {
        let legs = vec![
            crate::types::MassQuoteLegInput {
                side: Side::Buy,
                price: 100.5,
                quantity: 1.0,
                cancel_order_id: Some(42),
                time_in_force: None,
                expiry_time: None,
            },
            crate::types::MassQuoteLegInput {
                side: Side::Sell,
                price: 200.0,
                quantity: 2.0,
                cancel_order_id: None,
                time_in_force: Some(TimeInForce::Gtc),
                expiry_time: None,
            },
        ];
        let bytes = build_mass_quote_proto(7, &TEST_UUID, &legs, &TEST_UUID, None);
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let mq = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::MassQuote(m)) => m,
            other => panic!("expected MassQuote, got {:?}", other),
        };
        assert_eq!(mq.symbol_id, 7);
        assert_eq!(mq.user_uuid, TEST_UUID.as_slice());
        assert_eq!(
            mq.correlation_id,
            u128::from_be_bytes(TEST_UUID).to_le_bytes()
        );
        assert_eq!(mq.legs.len(), 2);
        assert_eq!(mq.legs[0].cancel_order_id, 42);
        assert_eq!(mq.legs[0].price, 100.5);
        // Pure-place leg defaults the cancel target to 0.
        assert_eq!(mq.legs[1].cancel_order_id, 0);
        // Each leg carries a unique 16-byte correlation id.
        assert_eq!(mq.legs[0].correlation_id.len(), 16);
        assert_ne!(mq.legs[0].correlation_id, mq.legs[1].correlation_id);
        // Default encodes post_only=true (sequencer requires the field).
        assert_eq!(mq.post_only, Some(true));
    }

    #[test]
    fn test_build_mass_quote_proto_relaxed_post_only() {
        let legs = vec![crate::types::MassQuoteLegInput {
            side: Side::Buy,
            price: 100.0,
            quantity: 1.0,
            cancel_order_id: None,
            time_in_force: None,
            expiry_time: None,
        }];
        let bytes = build_mass_quote_proto(1, &TEST_UUID, &legs, &[0u8; 16], Some(false));
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let mq = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::MassQuote(m)) => m,
            other => panic!("expected MassQuote, got {:?}", other),
        };
        assert_eq!(mq.post_only, Some(false));
    }

    #[test]
    fn test_build_batch_cancel_proto_roundtrip() {
        let bytes = build_batch_cancel_proto(9, &TEST_UUID, &[11, 22, 33], &TEST_UUID);
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let bc = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::BatchCancel(b)) => b,
            other => panic!("expected BatchCancel, got {:?}", other),
        };
        assert_eq!(bc.symbol_id, 9);
        assert_eq!(bc.user_uuid, TEST_UUID.as_slice());
        assert_eq!(bc.order_ids, vec![11, 22, 33]);
        assert_eq!(
            bc.correlation_id,
            u128::from_be_bytes(TEST_UUID).to_le_bytes()
        );
    }

    #[test]
    fn test_build_batch_modify_proto_roundtrip() {
        let legs = vec![
            crate::types::BatchModifyLegInput {
                order_id: 5,
                new_price: Some(101.0),
                new_quantity: None,
            },
            crate::types::BatchModifyLegInput {
                order_id: 6,
                new_price: None,
                new_quantity: Some(4.0),
            },
        ];
        let bytes = build_batch_modify_proto(9, &TEST_UUID, &legs, &TEST_UUID);
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let bm = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::BatchModify(b)) => b,
            other => panic!("expected BatchModify, got {:?}", other),
        };
        assert_eq!(bm.symbol_id, 9);
        assert_eq!(
            bm.correlation_id,
            u128::from_be_bytes(TEST_UUID).to_le_bytes()
        );
        assert_eq!(bm.legs.len(), 2);
        assert_eq!(bm.legs[0].order_id, 5);
        assert_eq!(bm.legs[0].new_price, Some(101.0));
        assert_eq!(bm.legs[0].new_quantity, None);
        assert_eq!(bm.legs[1].new_quantity, Some(4.0));
        assert_eq!(bm.legs[0].correlation_id.len(), 16);
    }

    #[test]
    fn test_parse_mass_quote_ack_roundtrip() {
        let ack = sequencer::MassQuoteAck {
            node_id: 1,
            sequence: 99,
            correlation_id: vec![],
            error_code: None,
            reject_text: None,
            results: vec![
                sequencer::MassQuoteLegResult {
                    leg_index: 0,
                    cancelled_order_id: 42,
                    new_order_id: 77,
                    status: sequencer::MassQuoteLegStatus::Open as i32,
                    error_code: None,
                    fill_count: 0,
                },
                sequencer::MassQuoteLegResult {
                    leg_index: 1,
                    cancelled_order_id: 0,
                    new_order_id: 0,
                    status: sequencer::MassQuoteLegStatus::Failed as i32,
                    error_code: Some(2018),
                    fill_count: 0,
                },
                sequencer::MassQuoteLegResult {
                    leg_index: 2,
                    cancelled_order_id: 0,
                    new_order_id: 88,
                    status: sequencer::MassQuoteLegStatus::Filled as i32,
                    error_code: None,
                    fill_count: 3,
                },
            ],
        };
        let wire = wrap_legacy_node_response("mass_quote_ack", &ack.encode_to_vec());
        let parsed = parse_mass_quote_ack(&wire).expect("parse");
        assert!(!parsed.success);
        assert_eq!(parsed.sequence, "99");
        assert_eq!(parsed.results.len(), 3);
        assert_eq!(parsed.results[0].status, "open");
        assert_eq!(parsed.results[0].cancelled_order_id.as_deref(), Some("42"));
        assert_eq!(parsed.results[0].new_order_id.as_deref(), Some("77"));
        assert_eq!(parsed.results[0].fill_count, 0);
        assert_eq!(parsed.results[1].status, "failed");
        assert_eq!(parsed.results[1].cancelled_order_id, None);
        assert_eq!(parsed.results[1].error_code, Some(2018));
        // Relaxed taker leg surfaces its fill count.
        assert_eq!(parsed.results[2].status, "filled");
        assert_eq!(parsed.results[2].fill_count, 3);
    }

    #[test]
    fn test_parse_batch_cancel_ack_roundtrip() {
        let ack = sequencer::BatchCancelAck {
            node_id: 1,
            sequence: 5,
            correlation_id: vec![],
            error_code: None,
            reject_text: None,
            results: vec![
                sequencer::BatchCancelLegResult {
                    order_id: 11,
                    cancelled: true,
                    error_code: None,
                },
                sequencer::BatchCancelLegResult {
                    order_id: 22,
                    cancelled: false,
                    error_code: Some(2003),
                },
            ],
        };
        let wire = wrap_legacy_node_response("batch_cancel_ack", &ack.encode_to_vec());
        let parsed = parse_batch_cancel_ack(&wire).expect("parse");
        assert!(!parsed.success);
        assert_eq!(parsed.results[0].order_id, "11");
        assert!(parsed.results[0].cancelled);
        assert!(!parsed.results[1].cancelled);
        assert_eq!(parsed.results[1].error_code, Some(2003));
    }

    #[test]
    fn test_parse_batch_modify_ack_roundtrip() {
        let ack = sequencer::BatchModifyAck {
            node_id: 1,
            sequence: 7,
            correlation_id: vec![],
            error_code: None,
            reject_text: None,
            results: vec![sequencer::BatchModifyLegResult {
                order_id: 5,
                modified: true,
                error_code: None,
            }],
        };
        let wire = wrap_legacy_node_response("batch_modify_ack", &ack.encode_to_vec());
        let parsed = parse_batch_modify_ack(&wire).expect("parse");
        assert!(parsed.success);
        assert_eq!(parsed.sequence, "7");
        assert_eq!(parsed.results[0].order_id, "5");
        assert!(parsed.results[0].modified);
    }

    #[test]
    fn test_build_order_header_aad_deterministic() {
        let a = build_order_header_aad(&TEST_UUID, 2, "place", 3, 400, b"", 7);
        let b = build_order_header_aad(&TEST_UUID, 2, "place", 3, 400, b"", 7);
        assert_eq!(a, b);
    }

    #[test]
    fn test_build_response_header_aad_deterministic() {
        let a = build_response_header_aad(&TEST_UUID, "ack", 100, 11, 12, &TEST_UUID, 42, 7);
        let b = build_response_header_aad(&TEST_UUID, "ack", 100, 11, 12, &TEST_UUID, 42, 7);
        assert_eq!(a, b);
        let header = edge::ResponseHeader::decode(a.as_slice()).expect("decode");
        assert_eq!(header.correlation_id, TEST_UUID);
        assert_eq!(header.session_seq, 42);
    }

    #[test]
    fn test_parse_node_response_ack() {
        let ack = sequencer::AckMessage {
            sequence: 8,
            order_id: 9,
            correlation_id: vec![1, 2, 3],
            ack_outcome: Some(sequencer::AckOutcomeWire {
                kind: sequencer::AckOutcomeKind::Applied as i32,
                order_status: Some(OrderStatus::New.to_proto()),
                business_error_code: Some(404),
                system_error_code: None,
            }),
            ..Default::default()
        };
        let bytes = wrap_legacy_node_response("ack", &ack.encode_to_vec());
        match parse_node_response(&bytes).expect("parse") {
            NodeResponseKind::Ack {
                sequence,
                order_id,
                success,
                error_code,
                reject_text,
                correlation_id,
                order_status,
            } => {
                assert_eq!(sequence, 8);
                assert_eq!(order_id, 9);
                assert!(!success);
                assert_eq!(error_code, Some(404));
                assert!(reject_text.is_none());
                assert_eq!(correlation_id, vec![1, 2, 3]);
                assert_eq!(order_status, Some(OrderStatus::New));
            }
            other => panic!("expected Ack, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_node_response_ack_reject_text() {
        let ack = sequencer::AckMessage {
            sequence: 2,
            order_id: 3,
            reject_text: Some("price too far from mark".into()),
            correlation_id: vec![],
            ack_outcome: Some(sequencer::AckOutcomeWire {
                kind: sequencer::AckOutcomeKind::SystemFailed as i32,
                order_status: None,
                business_error_code: Some(2007),
                system_error_code: None,
            }),
            ..Default::default()
        };
        match parse_node_response(&wrap_legacy_node_response("ack", &ack.encode_to_vec()))
            .expect("parse")
        {
            NodeResponseKind::Ack {
                success,
                error_code,
                reject_text,
                ..
            } => {
                assert!(!success);
                assert_eq!(error_code, Some(2007));
                assert_eq!(reject_text.as_deref(), Some("price too far from mark"));
            }
            other => panic!("expected Ack, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_node_response_fill() {
        let trade = sequencer::TradeMessage {
            trade_id: 100,
            taker_order_id: 200,
            maker_order_id: 300,
            symbol_id: 500,
            price: 0,
            quantity: 0,
            timestamp: 9_999,
            taker_side: Side::Sell.to_proto(),
            correlation_id: vec![1, 2],
            maker_remaining_qty: None,
        };
        let bytes = wrap_legacy_node_response("fill", &trade.encode_to_vec());
        match parse_node_response(&bytes).expect("parse") {
            NodeResponseKind::Fill {
                trade_id,
                taker_order_id,
                maker_order_id,
                symbol_id,
                timestamp,
                correlation_id,
            } => {
                assert_eq!(trade_id, 100);
                assert_eq!(taker_order_id, 200);
                assert_eq!(maker_order_id, 300);
                assert_eq!(symbol_id, 500);
                assert_eq!(timestamp, 9_999);
                assert_eq!(correlation_id, vec![1, 2]);
            }
            other => panic!("expected Fill, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_order_update() {
        let msg = sequencer::OrderUpdateMessage {
            message_type: OrderUpdateType::Cancelled.to_proto(),
            order_id: 4242,
            user_uuid: TEST_UUID.to_vec(),
            symbol_id: 200,
            order_status: OrderStatus::Cancelled.to_proto(),
            price: "1.5".to_string(),
            quantity: "10".to_string(),
            side: Side::Sell.to_proto(),
            filled_qty: "2".to_string(),
            remaining_qty: "8".to_string(),
            cum_fill: "3".to_string(),
            cancel_reason: Some(CancelReason::Expired.to_proto()),
            reject_reason_code: Some(42),
            correlation_id: vec![1, 2, 3, 4],
            timestamp: 1_700_000_000,
            leverage: 1,
            realized_pnl: None,
            order_type: OrderType::Limit.to_proto(),
            msg: None,
            avg_fill_price: None,
            trading_fee: None,
            take_profit: None,
            stop_loss: None,
            tpsl_status: None,
            peg_offset_bps: None,
            trigger_price: None,
            reduce_only: true,
            post_only: false,
        };
        let bytes = msg.encode_to_vec();
        let u = parse_order_update(&bytes).expect("parse");
        assert_eq!(u.order_id, "4242");
        assert_eq!(u.user_uuid, Uuid::from_bytes(TEST_UUID));
        assert_eq!(u.symbol_id, 200);
        assert_eq!(u.side, Side::Sell);
        assert_eq!(u.status, OrderStatus::Cancelled);
        assert_eq!(u.update_type, OrderUpdateType::Cancelled);
        assert_eq!(u.price, "1.5");
        assert_eq!(u.quantity, "10");
        assert_eq!(u.filled_qty, "2");
        assert_eq!(u.remaining_qty, "8");
        assert_eq!(u.cum_fill, "3");
        assert_eq!(u.cancel_reason, Some(CancelReason::Expired));
        assert_eq!(u.reject_reason.as_deref(), Some("42"));
        assert!(u.reduce_only);
        assert!(!u.post_only);
        assert_eq!(u.correlation_id, 0x0403_0201);
        assert_eq!(u.timestamp, 1_700_000_000);
    }

    #[test]
    fn test_parse_sequencer_to_edge_order_update() {
        let inner = sequencer::OrderUpdateMessage {
            message_type: OrderUpdateType::Open.to_proto(),
            order_id: 1,
            user_uuid: TEST_UUID.to_vec(),
            symbol_id: 3,
            order_status: OrderStatus::New.to_proto(),
            price: "1".to_string(),
            quantity: "2".to_string(),
            side: Side::Buy.to_proto(),
            filled_qty: "0".to_string(),
            remaining_qty: "2".to_string(),
            cum_fill: "0".to_string(),
            cancel_reason: None,
            reject_reason_code: None,
            correlation_id: vec![],
            timestamp: 100,
            leverage: 1,
            realized_pnl: None,
            order_type: OrderType::Limit.to_proto(),
            msg: None,
            avg_fill_price: None,
            trading_fee: None,
            take_profit: None,
            stop_loss: None,
            tpsl_status: None,
            peg_offset_bps: None,
            trigger_price: None,
            reduce_only: false,
            post_only: true,
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::OrderUpdate(
                inner,
            )),
        };
        let bytes = msg.encode_to_vec();
        match parse_sequencer_to_edge_message(&bytes).expect("parse") {
            EdgeMessage::OrderUpdate(u) => {
                assert_eq!(u.order_id, "1");
                assert_eq!(u.user_uuid, Uuid::from_bytes(TEST_UUID));
                assert_eq!(u.symbol_id, 3);
                assert_eq!(u.update_type, OrderUpdateType::Open);
            }
            other => panic!("expected OrderUpdate, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_sequencer_to_edge_system_health() {
        let health = health::HealthReport {
            role: 2,
            component_id: "sequencer-1".to_string(),
            state: 4,
            serving: true,
            signals: None,
            cause: String::new(),
            updated_at_nanos: 1,
            sequence: 2,
            schema_version: 1,
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::HealthReport(
                health,
            )),
        };
        let bytes = msg.encode_to_vec();
        match parse_sequencer_to_edge_message(&bytes).expect("parse") {
            EdgeMessage::SystemHealth(h) => {
                assert_eq!(h.component_id, "sequencer-1");
                assert_eq!(h.state, 4);
                assert!(h.serving);
                assert_eq!(h.updated_at_nanos, 1);
            }
            other => panic!("expected SystemHealth, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_positions_snapshot_round_trip() {
        let row = sequencer::PositionRow {
            symbol_id: 7,
            side: Side::Buy.to_proto(),
            size: "1.5".to_string(),
            entry_price: "85000".to_string(),
            leverage: 5,
            mark_price: Some("85100".to_string()),
            unrealized_pnl: Some("15000".to_string()),
            notional: Some("12765000".to_string()),
            mark_publish_time_sec: Some(1_700_000_010),
            liquidation_price: None,
            adl_indicator: None,
            position_status: None,
            margin: None,
            mgn_mode: 0,
            mmr: None,
            mgn_ratio: None,
            margin_ratio_bps: None,
            take_profit: None,
            stop_loss: None,
            tpsl_status: None,
            tpsl_parent_order_id: None,
        };
        let snap = sequencer::PositionsSnapshot {
            user_uuid: TEST_UUID.to_vec(),
            rows: vec![row],
            server_timestamp: 1_700_000_001,
            source: 2, // Periodic
            correlation_id: Some(vec![0xde, 0xad, 0xbe, 0xef]),
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::PositionsSnapshot(snap)),
        };
        match parse_sequencer_to_edge_message(&msg.encode_to_vec()).expect("parse") {
            EdgeMessage::PositionsSnapshot(p) => {
                assert_eq!(p.user_uuid, Uuid::from_bytes(TEST_UUID));
                assert_eq!(p.source, PositionsSnapshotSource::Periodic);
                assert_eq!(p.server_timestamp, 1_700_000_001);
                assert_eq!(p.correlation_id, Some(0xefbe_adde));
                assert_eq!(p.rows.len(), 1);
                assert_eq!(p.rows[0].symbol_id, 7);
                assert_eq!(p.rows[0].side, Side::Buy);
                assert_eq!(p.rows[0].leverage, 5);
                assert_eq!(p.rows[0].mark_price.as_deref(), Some("85100"));
            }
            other => panic!("expected PositionsSnapshot, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_balance_update_round_trip() {
        let bal = sequencer::BalanceUpdateMessage {
            user_uuid: TEST_UUID.to_vec(),
            balance_raw: 123_456_789,
            timestamp: 1_700_000_002,
            balance: "123.456789".to_string(),
            signed_balance_8dp: 0,
            free_collateral_8dp: 0,
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::BalanceUpdate(
                bal,
            )),
        };
        match parse_sequencer_to_edge_message(&msg.encode_to_vec()).expect("parse") {
            EdgeMessage::BalanceUpdate(b) => {
                assert_eq!(b.user_uuid, Uuid::from_bytes(TEST_UUID));
                assert_eq!(b.balance_raw, 123_456_789);
                assert_eq!(b.timestamp, 1_700_000_002);
            }
            other => panic!("expected BalanceUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_funding_rate_update_round_trip() {
        let f = sequencer::FundingRateUpdateMessage {
            symbol_id: 1,
            funding_rate: "0.0001".to_string(),
            last_funding_rate: "0.0002".to_string(),
            timestamp: 1_700_000_004,
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::FundingRateUpdate(f)),
        };
        match parse_sequencer_to_edge_message(&msg.encode_to_vec()).expect("parse") {
            EdgeMessage::FundingRateUpdate(u) => {
                assert_eq!(u.symbol_id, 1);
                assert_eq!(u.funding_rate, "0.0001");
                assert_eq!(u.last_funding_rate, "0.0002");
                assert_eq!(u.timestamp, 1_700_000_004);
            }
            other => panic!("expected FundingRateUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_leverage_settings_roundtrip() {
        let ls = sequencer::LeverageSettings {
            user_uuid: TEST_UUID.to_vec(),
            server_timestamp: 1_700_000_005,
            settings: vec![
                sequencer::LeverageSettingRow {
                    symbol_id: 42,
                    leverage: 5,
                },
                sequencer::LeverageSettingRow {
                    symbol_id: 7,
                    leverage: 10,
                },
            ],
            margin_mode_settings: vec![],
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::LeverageSettings(ls)),
        };
        match parse_sequencer_to_edge_message(&msg.encode_to_vec()).expect("parse") {
            EdgeMessage::LeverageSettings(settings) => {
                assert_eq!(settings.settings.len(), 2);
                assert_eq!(settings.settings[0].symbol_id, 42);
                assert_eq!(settings.settings[0].leverage, 5);
                assert_eq!(settings.settings[1].symbol_id, 7);
                assert_eq!(settings.server_timestamp, Some(1_700_000_005));
                assert_eq!(settings.user_uuid, Some(Uuid::from_bytes(TEST_UUID)));
            }
            other => panic!("expected LeverageSettings, got {other:?}"),
        }
    }

    #[test]
    fn test_correlation_id_to_u128() {
        // Response/body bodies carry the id as little-endian u128 bytes.
        let raw: [u8; 16] = [
            0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44,
            0x00, 0x00,
        ];
        assert_eq!(
            super::correlation_id_to_u128(&raw),
            u128::from_le_bytes(raw)
        );
    }

    #[test]
    fn test_correlation_id_body_roundtrip_le() {
        // Builders take raw UUID bytes (big-endian layout) and must emit the
        // canonical little-endian body encoding; decoding it must round-trip.
        let uuid_bytes = TEST_UUID;
        let body = super::correlation_id_body_bytes(&uuid_bytes);
        assert_eq!(body.len(), 16);
        assert_eq!(body, u128::from_be_bytes(uuid_bytes).to_le_bytes());
        assert_eq!(
            super::correlation_id_to_u128(&body),
            u128::from_be_bytes(uuid_bytes)
        );
    }
}
