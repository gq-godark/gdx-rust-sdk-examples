//! Decimal-string helpers for sequencer wire fields (price / quantity).

use crate::error::GodarkError;

/// Per-instrument fractional digit limits from `GET /api/v1/instruments`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstrumentDecimals {
    pub price_decimals: u8,
    pub quantity_decimals: u8,
}

impl InstrumentDecimals {
    /// Permissive offline fallback when instruments cannot be loaded.
    pub const FALLBACK: Self = Self {
        price_decimals: 8,
        quantity_decimals: 8,
    };
}

/// Format a non-negative `f64` as a human decimal string with at most `decimals`
/// fractional digits. Rejects values that need more precision than `decimals`.
///
/// Trailing fractional zeros are trimmed (`1.50` → `"1.5"`); integer values omit
/// the fractional part (`10.0` → `"10"`).
pub fn format_decimal(value: f64, decimals: u8) -> Result<String, GodarkError> {
    if !value.is_finite() {
        return Err(GodarkError::Config(format!(
            "decimal value must be finite, got {value}"
        )));
    }
    if value.is_sign_negative() {
        return Err(GodarkError::Config(format!(
            "decimal value must be non-negative, got {value}"
        )));
    }
    if decimals == 0 {
        let nearest = value.round();
        if (value - nearest).abs() > 1e-9 {
            return Err(GodarkError::Config(format!(
                "value {value} has more fractional digits than the venue allows (0)"
            )));
        }
        if nearest > u64::MAX as f64 {
            return Err(GodarkError::Config("decimal value out of range".into()));
        }
        return Ok((nearest as u64).to_string());
    }

    let scale = 10u128
        .checked_pow(u32::from(decimals))
        .ok_or_else(|| GodarkError::Config("decimal scale overflow".into()))?;
    let scaled = value * (scale as f64);
    let nearest = scaled.round();
    // Tolerate float noise; reject true over-precision.
    let tol = (1e-6_f64).max(scaled.abs() * 1e-12);
    if (scaled - nearest).abs() > tol {
        return Err(GodarkError::Config(format!(
            "value {value} has more fractional digits than the venue allows ({decimals})"
        )));
    }
    if nearest < 0.0 || nearest > (u128::MAX as f64) {
        return Err(GodarkError::Config("decimal value out of range".into()));
    }
    let n = nearest as u128;
    Ok(format_scaled(n, decimals))
}

/// Parse a wire decimal string back to `f64` (SDK public numeric types).
pub fn parse_decimal(s: &str) -> Result<f64, GodarkError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(GodarkError::Config("empty decimal string".into()));
    }
    s.parse::<f64>()
        .map_err(|e| GodarkError::Config(format!("invalid decimal '{s}': {e}")))
}

fn format_scaled(n: u128, decimals: u8) -> String {
    if decimals == 0 {
        return n.to_string();
    }
    let scale = 10u128.pow(u32::from(decimals));
    let integer = n / scale;
    let fraction = n % scale;
    if fraction == 0 {
        return integer.to_string();
    }
    let mut frac = format!("{:0>width$}", fraction, width = decimals as usize);
    while frac.ends_with('0') {
        frac.pop();
    }
    format!("{integer}.{frac}")
}

/// Format an optional price-like field (`None` stays absent on the wire).
pub fn format_opt_price(
    value: Option<f64>,
    decimals: InstrumentDecimals,
) -> Result<Option<String>, GodarkError> {
    value
        .map(|v| format_decimal(v, decimals.price_decimals))
        .transpose()
}

/// Format an optional quantity-like field.
pub fn format_opt_quantity(
    value: Option<f64>,
    decimals: InstrumentDecimals,
) -> Result<Option<String>, GodarkError> {
    value
        .map(|v| format_decimal(v, decimals.quantity_decimals))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_trims_trailing_zeros() {
        assert_eq!(format_decimal(10.5, 4).unwrap(), "10.5");
        assert_eq!(format_decimal(10.0, 4).unwrap(), "10");
        assert_eq!(format_decimal(0.001, 4).unwrap(), "0.001");
        assert_eq!(format_decimal(95000.5, 1).unwrap(), "95000.5");
        assert_eq!(format_decimal(0.0, 4).unwrap(), "0");
    }

    #[test]
    fn format_rejects_over_precision() {
        let err = format_decimal(0.0011, 3).unwrap_err();
        assert!(err.to_string().contains("more fractional"));
        let err = format_decimal(1.25, 0).unwrap_err();
        assert!(err.to_string().contains("more fractional"));
    }

    #[test]
    fn parse_roundtrips() {
        let s = format_decimal(123.45, 2).unwrap();
        assert_eq!(parse_decimal(&s).unwrap(), 123.45);
    }

    #[test]
    fn format_rejects_negative_and_nan() {
        assert!(format_decimal(-1.0, 2).is_err());
        assert!(format_decimal(f64::NAN, 2).is_err());
    }
}
