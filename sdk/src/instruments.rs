//! Edge public instruments → symbol map (production SoT).

use std::collections::HashMap;

use serde_json::Value;

use crate::error::{GodarkError, Result};
use crate::rest_transport::RestTransport;

/// Parse `GET /api/v1/instruments` data into symbol → symbol_id.
pub fn symbol_map_from_instruments_data(data: &Value) -> Result<HashMap<String, u64>> {
    let rows = data
        .get("instruments")
        .and_then(|v| v.as_array())
        .ok_or_else(|| {
            GodarkError::Config("instruments response missing instruments array".into())
        })?;
    let mut out = HashMap::new();
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
        out.insert(sym.to_string(), id);
    }
    if out.is_empty() {
        return Err(GodarkError::Config(
            "instruments response contained no usable symbol rows".into(),
        ));
    }
    Ok(out)
}

/// Bundled offline fallback (tests / edge unreachable).
pub fn offline_symbol_map() -> HashMap<String, u64> {
    const DEFAULT_SYMBOLS_JSON: &str = include_str!("../shared/symbols.json");
    serde_json::from_str(DEFAULT_SYMBOLS_JSON).expect("default symbols.json must be valid")
}

/// Fetch symbol map from edge; fall back to offline map on failure.
pub async fn load_symbol_map_from_edge(rest_base_url: &str) -> HashMap<String, u64> {
    let http = RestTransport::new(rest_base_url);
    match http.instruments_public().await {
        Ok(data) => symbol_map_from_instruments_data(&data).unwrap_or_else(|e| {
            tracing::warn!("edge instruments parse failed ({e}); using offline fallback");
            offline_symbol_map()
        }),
        Err(e) => {
            tracing::warn!("edge instruments fetch failed ({e}); using offline fallback");
            offline_symbol_map()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        let map = symbol_map_from_instruments_data(&data).unwrap();
        assert_eq!(map.get("BTC-USDC-PERP"), Some(&1));
        assert_eq!(map.get("ETH-USDC-PERP"), Some(&2));
    }

    #[test]
    fn symbol_map_from_instruments_data_rejects_empty() {
        let data = json!({ "instruments": [] });
        assert!(symbol_map_from_instruments_data(&data).is_err());
    }
}
