//! GoDark Rust Trading SDK
//!
//! Programmatic access to the GoDark DEX — encrypted trading over WebSocket.

pub mod client;
pub mod config;
pub mod crypto;
pub mod enums;
pub mod error;
mod generated;
/// Raw Protobuf types for the sequencer wire protocol (`gdx.sequencer.v1`).
pub mod pb {
    pub mod sequencer {
        pub mod v1 {
            pub use crate::generated::sequencer::v1::*;
        }
    }
}
pub mod instruments;
pub mod order_error_code;
pub mod proto_bridge;
pub mod rest_client;
pub mod rest_transport;
pub mod session;
pub mod transport;
pub mod types;
mod ws_connect;

pub use client::GodarkClient;
pub use config::{
    gomarket_url, resolve_market_data_ws_url, resolve_passphrase, ws_url, Environment,
    GodarkConfig, GodarkConfigBuilder, TransportConfig,
};
pub use enums::{
    CancelReason, OrderStatus, OrderType, OrderUpdateType, PositionUpdateType, Side, TimeInForce,
};
pub use error::GodarkError;
pub use order_error_code::{find as find_order_error, OrderErrorEntry, ORDER_ERROR_CODES};
pub use rest_client::{GodarkRestClient, GodarkRestClientBuilder};
pub use rest_transport::RestTransport;
pub use types::{
    AccountMarginSummary, AccountMarginUpdate, Balance, BalanceUpdate, BatchCancelAck,
    BatchCancelLegResult, BatchModifyAck, BatchModifyLegInput, BatchModifyLegResult, Confirmation,
    FundingRateUpdate, LeverageSetting, LeverageSettings, MarginAlert, MassQuoteAck,
    MassQuoteLegInput, MassQuoteLegResult, MeProfile, OpenOrderRow, OpenOrdersSnapshot, OrderAck,
    OrderUpdate, PositionRow, PositionUpdate, PositionsSnapshot, PositionsSnapshotSource,
    ReconnectEvent, SettlementBatchStatus, SettlementUpdate, SystemHealthUpdate,
};
