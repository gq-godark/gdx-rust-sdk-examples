//! Edge public instruments → symbol map + decimal scales (production SoT).

use std::collections::HashMap;

use serde_json::Value;

use crate::decimals::InstrumentDecimals;
use crate::error::{GodarkError, Result};
use crate::rest_transport::RestTransport;

/// Parsed `GET /api/v1/instruments` catalog.
#[derive(Debug, Clone, Default)]
pub struct InstrumentCatalog {
    pub symbol_map: HashMap<String, u64>,
    /// Keyed by `symbol_id`.
    pub decimals: HashMap<u64, InstrumentDecimals>,
}

/// Parse `GET /api/v1/instruments` data into symbol map + decimal scales.
pub fn catalog_from_instruments_data(data: &Value) -> Result<InstrumentCatalog> {
    let rows = data
        .get("instruments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            GodarkError::Config("instruments response missing instruments array".into())
        })?;
    let mut symbol_map = HashMap::new();
    let mut decimals = HashMap::new();
    for row in rows {
        let Some(obj) = row.as_object() else {
            continue;
        };
        let Some(sym) = obj.get("symbol").and_then(|v| v.as_str()) else {
            continue;
        };
        let Some(id) = obj.get("symbol_id").and_then(|v| v.as_u64()) else {
            continue;
        };
        symbol_map.insert(sym.to_string(), id);
        let price_decimals = obj
            .get("price_decimals")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .min(38) as u8;
        let quantity_decimals = obj
            .get("quantity_decimals")
            .and_then(|v| v.as_u64())
            .unwrap_or(8)
            .min(38) as u8;
        decimals.insert(
            id,
            InstrumentDecimals {
                price_decimals,
                quantity_decimals,
            },
        );
    }
    if symbol_map.is_empty() {
        return Err(GodarkError::Config(
            "instruments response contained no usable symbol rows".into(),
        ));
    }
    Ok(InstrumentCatalog {
        symbol_map,
        decimals,
    })
}

/// Bundled offline fallback (tests / edge unreachable).
pub fn offline_symbol_map() -> HashMap<String, u64> {
    const DEFAULT_SYMBOLS_JSON: &str = include_str!("../shared/symbols.json");
    serde_json::from_str(DEFAULT_SYMBOLS_JSON).expect("default symbols.json must be valid")
}

/// Offline decimal scales for symbols in [`offline_symbol_map`].
pub fn offline_instrument_decimals() -> HashMap<u64, InstrumentDecimals> {
    // Matches public Devnet instruments as of proto string-price rollout.
    let mut m = HashMap::new();
    m.insert(
        1,
        InstrumentDecimals {
            price_decimals: 1,
            quantity_decimals: 4,
        },
    );
    m.insert(
        2,
        InstrumentDecimals {
            price_decimals: 2,
            quantity_decimals: 3,
        },
    );
    m.insert(
        5,
        InstrumentDecimals {
            price_decimals: 2,
            quantity_decimals: 2,
        },
    );
    m
}

/// Offline catalog (symbol map + decimals).
pub fn offline_catalog() -> InstrumentCatalog {
    InstrumentCatalog {
        symbol_map: offline_symbol_map(),
        decimals: offline_instrument_decimals(),
    }
}

/// Fetch instrument catalog from edge; fall back to offline on failure.
pub async fn load_catalog_from_edge(rest_base_url: &str) -> InstrumentCatalog {
    let http = RestTransport::new(rest_base_url);
    match http.instruments_public().await {
        Ok(data) => catalog_from_instruments_data(&data).unwrap_or_else(|e| {
            tracing::warn!("edge instruments parse failed ({e}); using offline fallback");
            offline_catalog()
        }),
        Err(e) => {
            tracing::warn!("edge instruments fetch failed ({e}); using offline fallback");
            offline_catalog()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn catalog_parses_decimals() {
        let data = json!({
            "instruments": [
                {
                    "symbol": "BTC-USDC-PERP",
                    "symbol_id": 1,
                    "price_decimals": 1,
                    "quantity_decimals": 4
                },
                { "symbol": "ETH-USDC-PERP", "symbol_id": 2, "price_decimals": 2, "quantity_decimals": 3 }
            ]
        });
        let cat = catalog_from_instruments_data(&data).unwrap();
        assert_eq!(cat.symbol_map.get("BTC-USDC-PERP"), Some(&1));
        assert_eq!(
            cat.decimals.get(&1),
            Some(&InstrumentDecimals {
                price_decimals: 1,
                quantity_decimals: 4
            })
        );
    }

    #[test]
    fn symbol_map_from_instruments_data_parses_wire_shape() {
        let data = json!({
            "instruments": [
                {
                    "symbol": "BTC-USDC-PERP",
                    "symbol_id": 1,
                    "tick_size": 0.5,
                    "max_leverage": 10
                },
                { "symbol": "ETH-USDC-PERP", "symbol_id": 2 }
            ]
        });
        let map = catalog_from_instruments_data(&data).unwrap().symbol_map;
        assert_eq!(map.get("BTC-USDC-PERP"), Some(&1));
        assert_eq!(map.get("ETH-USDC-PERP"), Some(&2));
    }

    #[test]
    fn symbol_map_from_instruments_data_rejects_empty() {
        let data = json!({ "instruments": [] });
        assert!(catalog_from_instruments_data(&data).is_err());
    }
}
