//! GoDark DEX Rust SDK — HPKE-encrypted trading over gdx-edge.
//!
//! Protocol source of truth: **gdx-edge** + **gdx-sequencer** (HPKE Base,
//! `TradingWsBinaryFrame`, REST one-shot HPKE).

mod access_token;
mod client;
mod config;
mod enums;
mod error;
mod generated;
mod hpke;
mod instruments;
mod order_error_code;
mod proto_bridge;
mod rest_client;
mod rest_transport;
mod session;
mod transport;
mod types;
mod wire;
mod ws_connect;

/// Raw protobuf types (`gdx.sequencer.v1`, `gdx.edge.v1`).
pub mod pb {
    pub mod sequencer {
        pub mod v1 {
            pub use crate::generated::sequencer::v1::*;
        }
    }
    pub mod edge {
        pub mod v1 {
            pub use crate::generated::edge::v1::*;
        }
    }
}

/// Helpers for in-repo integration tests (not part of the supported SDK API).
#[doc(hidden)]
pub mod testing {
    pub use crate::hpke::{
        info_for_conn, nonce_from_u64, open_session, SealedSession, StaticKeyPair, WIRE_VERSION,
    };
    pub use crate::rest_transport::RestTransport;
    pub use crate::session::CryptoSession;

    pub mod proto_bridge {
        pub use crate::proto_bridge::*;
    }
    pub mod wire {
        pub use crate::wire::*;
    }
}

pub use client::GodarkClient;
pub use config::{
    resolve_passphrase, Environment, GodarkConfig, GodarkConfigBuilder, TransportConfig,
};
pub use enums::{
    CancelReason, OrderStatus, OrderType, OrderUpdateType, Side, StpMode, TimeInForce,
};
pub use error::GodarkError;
pub use order_error_code::{find as find_order_error, OrderErrorEntry, ORDER_ERROR_CODES};
pub use rest_client::{GodarkRestClient, GodarkRestClientBuilder};
pub use types::{
    AccountMarginSummary, AccountMarginUpdate, BalanceUpdate, BatchCancelAck, BatchCancelLegResult,
    BatchModifyAck, BatchModifyLegInput, BatchModifyLegResult, Confirmation, FundingRateUpdate,
    CountAck, LeverageSetting, LeverageSettings, MassQuoteAck, MassQuoteLegInput, TpslAck,
    MassQuoteLegResult, MeProfile, OpenOrderRow, OpenOrdersSnapshot, OrderAck, OrderUpdate,
    PlaceOrderOptions, PositionRow,
    PositionsSnapshot, PositionsSnapshotSource, ReconnectEvent, SystemHealthUpdate,
};
