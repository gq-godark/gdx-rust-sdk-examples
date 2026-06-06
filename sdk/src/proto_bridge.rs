// Protobuf builders and parsers — mirrors Python SDK _proto.py

use prost::Message;
use uuid::Uuid;

use crate::enums::{self, CancelReason, OrderStatus, OrderUpdateType, PositionUpdateType, Side};
use crate::error::GodarkError;
use crate::generated::edge::v1 as edge;
use crate::generated::sequencer::v1 as sequencer;
use crate::types::{
    BalanceUpdate, FundingRateUpdate, MarginAlert, OrderUpdate, PositionRow, PositionUpdate,
    PositionsSnapshot, PositionsSnapshotSource, SettlementBatchStatus, SettlementUpdate,
    SystemHealthUpdate,
};

fn correlation_id_to_u128(raw: &[u8]) -> u128 {
    if raw.is_empty() {
        return 0;
    }
    let mut buf = [0u8; 16];
    let len = raw.len().min(16);
    buf[16 - len..].copy_from_slice(&raw[..len]);
    u128::from_be_bytes(buf)
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
    timestamp: u64,
) -> Vec<u8> {
    let place = sequencer::PlaceOrderInput {
        symbol_id,
        side: side.to_proto(),
        order_type: order_type.to_proto(),
        quantity,
        user_commitment: Vec::new(),
        time_in_force: time_in_force.to_proto(),
        aon,
        price,
        min_fill_size,
        expiry_time,
        correlation_id: correlation_id_bytes.to_vec(),
        timestamp,
        user_uuid: user_uuid.to_vec(),
        leverage: 1,
        stp_mode: 0,
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
    let cancel = sequencer::CancelMessage {
        order_id,
        user_commitment: vec![0u8; 32],
        symbol_id,
        sequence: 0,
        correlation_id: correlation_id_bytes.to_vec(),
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::Cancel(cancel)),
    };
    req.encode_to_vec()
}

pub fn build_modify_order_proto(
    order_id: u64,
    user_uuid: &[u8],
    symbol_id: u64,
    new_price: Option<f64>,
    new_quantity: Option<f64>,
    correlation_id_bytes: &[u8],
) -> Vec<u8> {
    let modify = sequencer::ModifyOrderInput {
        order_id,
        user_commitment: Vec::new(),
        symbol_id,
        new_price,
        new_quantity,
        correlation_id: correlation_id_bytes.to_vec(),
        user_uuid: user_uuid.to_vec(),
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
        correlation_id: correlation_id_bytes.to_vec(),
    };
    let req = sequencer::EdgeSequencerRequest {
        inner: Some(sequencer::edge_sequencer_request::Inner::UpdateLeverage(
            update,
        )),
    };
    req.encode_to_vec()
}

pub fn build_order_header_aad(
    user_uuid: &[u8],
    symbol_id: u64,
    request_type: &str,
    nonce: u64,
    body_length: u32,
    correlation_id: &[u8],
) -> Vec<u8> {
    let header = edge::OrderHeader {
        user_uuid: user_uuid.to_vec(),
        symbol_id,
        request_type: enums::request_type_to_proto(request_type),
        nonce,
        body_length,
        correlation_id: correlation_id.to_vec(),
    };
    header.encode_to_vec()
}

pub fn build_response_header_aad(
    user_uuid: &[u8],
    message_type: &str,
    body_length: u32,
    nonce: u64,
    fencing_epoch: u64,
) -> Vec<u8> {
    let header = edge::ResponseHeader {
        user_uuid: user_uuid.to_vec(),
        message_type: enums::response_message_type_to_proto(message_type),
        body_length,
        nonce,
        fencing_epoch,
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
        node_id: u64,
        sequence: u64,
        order_id: u64,
        success: bool,
        error_code: Option<u32>,
        correlation_id: Vec<u8>,
        order_status: Option<OrderStatus>,
    },
    Fill {
        trade_id: u64,
        taker_order_id: u64,
        maker_order_id: u64,
        maker_user_commitment: Vec<u8>,
        symbol_id: u64,
        timestamp: u64,
        correlation_id: Vec<u8>,
    },
    Signing,
    Unknown,
}

pub fn parse_node_response(data: &[u8]) -> Result<NodeResponseKind, GodarkError> {
    let resp = sequencer::NodeResponse::decode(data)?;
    match resp.inner {
        Some(sequencer::node_response::Inner::Ack(ack)) => Ok(NodeResponseKind::Ack {
            node_id: ack.node_id,
            sequence: ack.sequence,
            order_id: ack.order_id,
            success: ack.success,
            error_code: ack.error_code,
            correlation_id: ack.correlation_id,
            order_status: ack.order_status.map(OrderStatus::from_proto),
        }),
        Some(sequencer::node_response::Inner::Fill(fill)) => Ok(NodeResponseKind::Fill {
            trade_id: fill.trade_id,
            taker_order_id: fill.taker_order_id,
            maker_order_id: fill.maker_order_id,
            maker_user_commitment: fill.maker_user_commitment,
            symbol_id: fill.symbol_id,
            timestamp: fill.timestamp,
            correlation_id: fill.correlation_id,
        }),
        Some(sequencer::node_response::Inner::Signing(_)) => Ok(NodeResponseKind::Signing),
        Some(sequencer::node_response::Inner::OpenOrdersSnapshot(_))
        | Some(sequencer::node_response::Inner::OrderHistorySnapshot(_))
        | Some(sequencer::node_response::Inner::FillShareResponse(_))
        | Some(sequencer::node_response::Inner::NodeReady(_))
        | Some(sequencer::node_response::Inner::CatchupApplied(_)) => Ok(NodeResponseKind::Unknown),
        None => Ok(NodeResponseKind::Unknown),
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
        correlation_id: correlation_id_to_u128(&msg.correlation_id),
        timestamp: msg.timestamp,
    })
}

pub fn parse_position_update(data: &[u8]) -> Result<PositionUpdate, GodarkError> {
    let msg = sequencer::PositionUpdateMessage::decode(data)?;
    Ok(PositionUpdate {
        user_uuid: uuid_from_bytes(&msg.user_uuid),
        symbol_id: msg.symbol_id,
        side: Side::from_proto(msg.side),
        update_type: PositionUpdateType::from_proto(msg.update_type),
        size: msg.size,
        entry_price: msg.entry_price,
        previous_size: msg.previous_size,
        fill_price: msg.fill_price.unwrap_or_default(),
        fill_qty: msg.fill_qty.unwrap_or_default(),
        correlation_id: correlation_id_to_u128(msg.correlation_id.as_deref().unwrap_or_default()),
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
    PositionUpdate(PositionUpdate),
    PositionsSnapshot(PositionsSnapshot),
    SystemHealth(SystemHealthUpdate),
    BalanceUpdate(BalanceUpdate),
    MarginAlert(MarginAlert),
    FundingRateUpdate(FundingRateUpdate),
    SettlementUpdate(SettlementUpdate),
    /// Recognized proto variant that this SDK build doesn't decode (e.g. a
    /// brand-new oneof arm added on the sequencer side after this build).
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

fn parse_settlement_batch_status(value: i32) -> SettlementBatchStatus {
    match value {
        1 => SettlementBatchStatus::Submitted,
        2 => SettlementBatchStatus::Confirmed,
        3 => SettlementBatchStatus::Failed,
        _ => SettlementBatchStatus::Unspecified,
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

pub fn parse_system_health(msg: sequencer::SystemHealthMessage) -> SystemHealthUpdate {
    SystemHealthUpdate {
        total_nodes: msg.total_nodes,
        accepting_orders: msg.accepting_orders,
        ready: msg.ready,
        degraded: msg.degraded,
        exhausted: msg.exhausted,
        warming: msg.warming,
        draining: msg.draining,
        waiting: msg.waiting,
    }
}

pub fn parse_balance_update(msg: sequencer::BalanceUpdateMessage) -> BalanceUpdate {
    BalanceUpdate {
        user_uuid: uuid_from_bytes(&msg.user_uuid),
        shielded_balance_raw: msg.shielded_balance_raw,
        timestamp: msg.timestamp,
    }
}

pub fn parse_margin_alert(msg: sequencer::MarginAlertMessage) -> MarginAlert {
    MarginAlert {
        owner: uuid_from_bytes(&msg.owner),
        symbol_id: msg.symbol_id,
        tier: msg.tier,
        margin_ratio_bps: msg.margin_ratio_bps,
        mark_price_bps: msg.mark_price_bps,
        liquidation_price_bps: msg.liquidation_price_bps,
        ts: msg.ts,
        state_version: msg.state_version,
        recovered: msg.recovered,
    }
}

pub fn parse_funding_rate_update(msg: sequencer::FundingRateUpdateMessage) -> FundingRateUpdate {
    FundingRateUpdate {
        symbol_id: msg.symbol_id,
        current_rate: msg.current_rate,
        predicted_rate: msg.predicted_rate,
        next_funding_time: msg.next_funding_time,
        timestamp: msg.timestamp,
    }
}

pub fn parse_settlement_update(msg: sequencer::SettlementUpdateMessage) -> SettlementUpdate {
    SettlementUpdate {
        batch_id: msg.batch_id,
        status: parse_settlement_batch_status(msg.status),
        tx_signature: msg.tx_signature,
        timestamp: msg.timestamp,
        affected_user_uuids: msg
            .affected_user_uuids
            .iter()
            .map(|b| uuid_from_bytes(b))
            .collect(),
    }
}

pub fn parse_sequencer_to_edge_message(data: &[u8]) -> Result<EdgeMessage, GodarkError> {
    let msg = sequencer::SequencerToEdgeMessage::decode(data)?;
    match msg.inner {
        Some(sequencer::sequencer_to_edge_message::Inner::OrderUpdate(ou)) => {
            let bytes = ou.encode_to_vec();
            Ok(EdgeMessage::OrderUpdate(parse_order_update(&bytes)?))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::PositionUpdate(pu)) => {
            let bytes = pu.encode_to_vec();
            Ok(EdgeMessage::PositionUpdate(parse_position_update(&bytes)?))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::PositionsSnapshot(ps)) => {
            Ok(EdgeMessage::PositionsSnapshot(parse_positions_snapshot(ps)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::SystemHealth(h)) => {
            Ok(EdgeMessage::SystemHealth(parse_system_health(h)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::BalanceUpdate(b)) => {
            Ok(EdgeMessage::BalanceUpdate(parse_balance_update(b)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::MarginAlert(m)) => {
            Ok(EdgeMessage::MarginAlert(parse_margin_alert(m)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::FundingRateUpdate(f)) => {
            Ok(EdgeMessage::FundingRateUpdate(parse_funding_rate_update(f)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::SettlementUpdate(s)) => {
            Ok(EdgeMessage::SettlementUpdate(parse_settlement_update(s)))
        }
        Some(sequencer::sequencer_to_edge_message::Inner::OrderHistoryInsert(_))
        | Some(sequencer::sequencer_to_edge_message::Inner::OpenInterestUpdate(_))
        | Some(sequencer::sequencer_to_edge_message::Inner::VolumeUpdate(_)) => {
            Ok(EdgeMessage::Unknown)
        }
        None => Ok(EdgeMessage::Unknown),
    }
}

#[cfg(test)]
mod tests {
    use prost::Message;
    use uuid::Uuid;

    use crate::enums::{
        CancelReason, OrderStatus, OrderType, OrderUpdateType, PositionUpdateType, Side,
        TimeInForce,
    };
    use crate::generated::edge::v1 as edge;
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
            b"cid",
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
        assert!(place.user_commitment.is_empty());
        assert_eq!(place.price, Some(1.25));
        assert_eq!(place.time_in_force, TimeInForce::Gtc.to_proto());
        assert!(place.aon);
        assert_eq!(place.min_fill_size, Some(0.5));
        assert_eq!(place.expiry_time, Some(999));
        assert_eq!(place.correlation_id, b"cid".as_slice());
        assert_eq!(place.timestamp, 1_234_567_890);
    }

    #[test]
    fn test_build_cancel_order_roundtrip() {
        let bytes = build_cancel_order_proto(10, &TEST_UUID, 30, b"corr");
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let cancel = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::Cancel(c)) => c,
            other => panic!("expected Cancel, got {:?}", other),
        };
        assert_eq!(cancel.order_id, 10);
        assert_eq!(cancel.user_commitment, vec![0u8; 32]);
        assert_eq!(cancel.symbol_id, 30);
        assert_eq!(cancel.sequence, 0);
        assert_eq!(cancel.correlation_id, b"corr".as_slice());
    }

    #[test]
    fn test_build_modify_order_roundtrip() {
        let bytes = build_modify_order_proto(7, &TEST_UUID, 9, Some(2.25), Some(3.5), b"m");
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let modify = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::Modify(m)) => m,
            other => panic!("expected Modify, got {:?}", other),
        };
        assert_eq!(modify.order_id, 7);
        assert!(modify.user_commitment.is_empty());
        assert_eq!(modify.user_uuid, TEST_UUID.as_slice());
        assert_eq!(modify.symbol_id, 9);
        assert_eq!(modify.new_price, Some(2.25));
        assert_eq!(modify.new_quantity, Some(3.5));
        assert_eq!(modify.correlation_id, b"m".as_slice());
    }

    #[test]
    fn test_build_update_leverage_roundtrip() {
        let bytes = build_update_leverage_proto(&TEST_UUID, 42, 5, b"corr");
        let decoded = sequencer::EdgeSequencerRequest::decode(bytes.as_slice()).expect("decode");
        let update = match decoded.inner {
            Some(sequencer::edge_sequencer_request::Inner::UpdateLeverage(u)) => u,
            other => panic!("expected UpdateLeverage, got {:?}", other),
        };
        assert_eq!(update.user_uuid, TEST_UUID.as_slice());
        assert_eq!(update.symbol_id, 42);
        assert_eq!(update.leverage, 5);
        assert_eq!(update.correlation_id, b"corr".as_slice());
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
    fn test_build_order_header_aad_update_leverage() {
        let bytes = build_order_header_aad(&TEST_UUID, 1, "update_leverage", 3, 128, b"");
        let header = edge::OrderHeader::decode(bytes.as_slice()).expect("decode");
        assert_eq!(header.request_type, 8);
    }

    #[test]
    fn test_build_order_header_aad_deterministic() {
        let a = build_order_header_aad(&TEST_UUID, 2, "place", 3, 400, b"");
        let b = build_order_header_aad(&TEST_UUID, 2, "place", 3, 400, b"");
        assert_eq!(a, b);
    }

    #[test]
    fn test_build_response_header_aad_deterministic() {
        let a = build_response_header_aad(&TEST_UUID, "ack", 100, 11, 12);
        let b = build_response_header_aad(&TEST_UUID, "ack", 100, 11, 12);
        assert_eq!(a, b);
    }

    #[test]
    fn test_parse_node_response_ack() {
        let ack = sequencer::AckMessage {
            node_id: 7,
            sequence: 8,
            order_id: 9,
            success: true,
            error_code: Some(404),
            correlation_id: vec![1, 2, 3],
            order_status: Some(OrderStatus::New.to_proto()),
            ..Default::default()
        };
        let resp = sequencer::NodeResponse {
            inner: Some(sequencer::node_response::Inner::Ack(ack)),
        };
        let bytes = resp.encode_to_vec();
        match parse_node_response(&bytes).expect("parse") {
            NodeResponseKind::Ack {
                node_id,
                sequence,
                order_id,
                success,
                error_code,
                correlation_id,
                order_status,
            } => {
                assert_eq!(node_id, 7);
                assert_eq!(sequence, 8);
                assert_eq!(order_id, 9);
                assert!(success);
                assert_eq!(error_code, Some(404));
                assert_eq!(correlation_id, vec![1, 2, 3]);
                assert_eq!(order_status, Some(OrderStatus::New));
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
            maker_user_commitment: TEST_UUID.to_vec(),
            symbol_id: 500,
            price: None,
            quantity: None,
            timestamp: 9_999,
            taker_side: Side::Sell.to_proto(),
            correlation_id: vec![1, 2],
            maker_remaining_qty_share: None,
            taker_user_commitment: None,
        };
        let resp = sequencer::NodeResponse {
            inner: Some(sequencer::node_response::Inner::Fill(trade)),
        };
        let bytes = resp.encode_to_vec();
        match parse_node_response(&bytes).expect("parse") {
            NodeResponseKind::Fill {
                trade_id,
                taker_order_id,
                maker_order_id,
                maker_user_commitment,
                symbol_id,
                timestamp,
                correlation_id,
            } => {
                assert_eq!(trade_id, 100);
                assert_eq!(taker_order_id, 200);
                assert_eq!(maker_order_id, 300);
                assert_eq!(maker_user_commitment, TEST_UUID.as_slice());
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
        assert_eq!(u.correlation_id, 0x0102_0304);
        assert_eq!(u.timestamp, 1_700_000_000);
    }

    #[test]
    fn test_parse_position_update() {
        let msg = sequencer::PositionUpdateMessage {
            update_type: PositionUpdateType::Increase.to_proto(),
            user_uuid: TEST_UUID.to_vec(),
            symbol_id: 2,
            side: Side::Buy.to_proto(),
            size: "100".to_string(),
            entry_price: "2.5".to_string(),
            previous_size: "50".to_string(),
            fill_price: Some("2.6".to_string()),
            fill_qty: Some("50".to_string()),
            correlation_id: Some(vec![0xab, 0xcd]),
            timestamp: 555,
            funding_rate: None,
        };
        let bytes = msg.encode_to_vec();
        let p = parse_position_update(&bytes).expect("parse");
        assert_eq!(p.user_uuid, Uuid::from_bytes(TEST_UUID));
        assert_eq!(p.symbol_id, 2);
        assert_eq!(p.side, Side::Buy);
        assert_eq!(p.update_type, PositionUpdateType::Increase);
        assert_eq!(p.size, "100");
        assert_eq!(p.entry_price, "2.5");
        assert_eq!(p.previous_size, "50");
        assert_eq!(p.fill_price, "2.6");
        assert_eq!(p.fill_qty, "50");
        assert_eq!(p.correlation_id, 0x0000000000000000000000000000abcd);
        assert_eq!(p.timestamp, 555);
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
        let health = sequencer::SystemHealthMessage {
            total_nodes: 10,
            accepting_orders: true,
            ready: 8,
            degraded: 1,
            exhausted: 0,
            warming: 1,
            draining: 0,
            waiting: 0,
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::SystemHealth(
                health,
            )),
        };
        let bytes = msg.encode_to_vec();
        match parse_sequencer_to_edge_message(&bytes).expect("parse") {
            EdgeMessage::SystemHealth(h) => {
                assert_eq!(h.total_nodes, 10);
                assert!(h.accepting_orders);
                assert_eq!(h.ready, 8);
                assert_eq!(h.degraded, 1);
                assert_eq!(h.exhausted, 0);
                assert_eq!(h.warming, 1);
                assert_eq!(h.draining, 0);
                assert_eq!(h.waiting, 0);
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
                assert_eq!(p.correlation_id, Some(0xdead_beef));
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
            shielded_balance_raw: 123_456_789,
            timestamp: 1_700_000_002,
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::BalanceUpdate(
                bal,
            )),
        };
        match parse_sequencer_to_edge_message(&msg.encode_to_vec()).expect("parse") {
            EdgeMessage::BalanceUpdate(b) => {
                assert_eq!(b.user_uuid, Uuid::from_bytes(TEST_UUID));
                assert_eq!(b.shielded_balance_raw, 123_456_789);
                assert_eq!(b.timestamp, 1_700_000_002);
            }
            other => panic!("expected BalanceUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_margin_alert_round_trip() {
        let alert = sequencer::MarginAlertMessage {
            owner: TEST_UUID.to_vec(),
            symbol_id: 1,
            tier: 2,
            margin_ratio_bps: 750,
            mark_price_bps: 8_510_000,
            liquidation_price_bps: 7_500_000,
            ts: 1_700_000_003,
            state_version: 9,
            recovered: false,
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::MarginAlert(
                alert,
            )),
        };
        match parse_sequencer_to_edge_message(&msg.encode_to_vec()).expect("parse") {
            EdgeMessage::MarginAlert(a) => {
                assert_eq!(a.owner, Uuid::from_bytes(TEST_UUID));
                assert_eq!(a.symbol_id, 1);
                assert_eq!(a.tier, 2);
                assert_eq!(a.margin_ratio_bps, 750);
                assert!(!a.recovered);
            }
            other => panic!("expected MarginAlert, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_funding_rate_update_round_trip() {
        let f = sequencer::FundingRateUpdateMessage {
            symbol_id: 1,
            current_rate: "0.0001".to_string(),
            predicted_rate: "0.0002".to_string(),
            next_funding_time: 1_700_003_600,
            timestamp: 1_700_000_004,
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::FundingRateUpdate(f)),
        };
        match parse_sequencer_to_edge_message(&msg.encode_to_vec()).expect("parse") {
            EdgeMessage::FundingRateUpdate(u) => {
                assert_eq!(u.symbol_id, 1);
                assert_eq!(u.current_rate, "0.0001");
                assert_eq!(u.predicted_rate, "0.0002");
                assert_eq!(u.next_funding_time, 1_700_003_600);
            }
            other => panic!("expected FundingRateUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_settlement_update_round_trip() {
        let s = sequencer::SettlementUpdateMessage {
            batch_id: 42,
            status: 2, // Confirmed
            tx_signature: "5xy...abc".to_string(),
            timestamp: 1_700_000_005,
            affected_user_uuids: vec![TEST_UUID.to_vec()],
        };
        let msg = sequencer::SequencerToEdgeMessage {
            inner: Some(sequencer::sequencer_to_edge_message::Inner::SettlementUpdate(s)),
        };
        match parse_sequencer_to_edge_message(&msg.encode_to_vec()).expect("parse") {
            EdgeMessage::SettlementUpdate(u) => {
                assert_eq!(u.batch_id, 42);
                assert_eq!(u.status, SettlementBatchStatus::Confirmed);
                assert_eq!(u.tx_signature, "5xy...abc");
                assert_eq!(u.affected_user_uuids.len(), 1);
                assert_eq!(u.affected_user_uuids[0], Uuid::from_bytes(TEST_UUID));
            }
            other => panic!("expected SettlementUpdate, got {other:?}"),
        }
    }

    #[test]
    fn test_correlation_id_to_u128() {
        let uuid: [u8; 16] = [
            0x55, 0x0e, 0x84, 0x00, 0xe2, 0x9b, 0x41, 0xd4, 0xa7, 0x16, 0x44, 0x66, 0x55, 0x44,
            0x00, 0x00,
        ];
        let expected = u128::from_be_bytes(uuid);
        assert_eq!(super::correlation_id_to_u128(&uuid), expected);
    }
}
