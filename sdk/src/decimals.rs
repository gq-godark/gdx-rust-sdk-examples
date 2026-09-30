//! Decimal-string helpers for sequencer wire fields (price / quantity).
//!
//! Public trading APIs take human decimal strings only; this module validates
//! and normalizes them against each instrument's `price_decimals` /
//! `quantity_decimals` before they are placed on the wire. There is no
//! public `f64` / `f32` / integer conversion path for prices or sizes.

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

/// Validate and normalize a non-negative human decimal string.
///
/// Accepts optional leading `+`, a single `.` fractional separator, and no
/// exponent form. Rejects empty input, negatives, and values with more than
/// `decimals` fractional digits (never rounds). Trailing fractional zeros are
/// trimmed (`"1.50"` → `"1.5"`); integer values omit the fractional part
/// (`"10.0"` → `"10"`).
pub fn normalize_decimal(value: &str, decimals: u8) -> Result<String, GodarkError> {
    let s = value.trim();
    if s.is_empty() {
        return Err(GodarkError::Config("empty decimal string".into()));
    }
    let s = s.strip_prefix('+').unwrap_or(s);
    if s.is_empty() || s.starts_with('-') {
        return Err(GodarkError::Config(format!(
            "decimal value must be non-negative, got {value:?}"
        )));
    }
    if s.contains(['e', 'E']) {
        return Err(GodarkError::Config(format!(
            "decimal value must not use exponent form, got {value:?}"
        )));
    }

    let (int_part, frac_part) = match s.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(GodarkError::Config(format!("invalid decimal '{value}'")));
    }
    let frac = frac_part.unwrap_or("");
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return Err(GodarkError::Config(format!("invalid decimal '{value}'")));
    }
    if frac.len() > usize::from(decimals) {
        return Err(GodarkError::Config(format!(
            "value {value:?} has more fractional digits than the venue allows ({decimals})"
        )));
    }

    let int_norm = {
        let trimmed = int_part.trim_start_matches('0');
        if trimmed.is_empty() {
            "0"
        } else {
            trimmed
        }
    };
    let frac_norm = frac.trim_end_matches('0');
    if frac_norm.is_empty() {
        Ok(int_norm.to_string())
    } else {
        Ok(format!("{int_norm}.{frac_norm}"))
    }
}

/// Normalize an optional price-like field (`None` stays absent on the wire).
pub fn normalize_opt_price(
    value: Option<&str>,
    decimals: InstrumentDecimals,
) -> Result<Option<String>, GodarkError> {
    value
        .map(|v| normalize_decimal(v, decimals.price_decimals))
        .transpose()
}

/// Normalize an optional quantity-like field.
pub fn normalize_opt_quantity(
    value: Option<&str>,
    decimals: InstrumentDecimals,
) -> Result<Option<String>, GodarkError> {
    value
        .map(|v| normalize_decimal(v, decimals.quantity_decimals))
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_trims_trailing_zeros() {
        assert_eq!(normalize_decimal("10.50", 4).unwrap(), "10.5");
        assert_eq!(normalize_decimal("10.0", 4).unwrap(), "10");
        assert_eq!(normalize_decimal("0.001", 4).unwrap(), "0.001");
        assert_eq!(normalize_decimal("95000.5", 1).unwrap(), "95000.5");
        assert_eq!(normalize_decimal("0", 4).unwrap(), "0");
        assert_eq!(normalize_decimal("00.1000", 4).unwrap(), "0.1");
        assert_eq!(normalize_decimal("+1.2500", 4).unwrap(), "1.25");
        assert_eq!(normalize_decimal("  2.5  ", 4).unwrap(), "2.5");
    }

    #[test]
    fn normalize_rejects_over_precision() {
        let err = normalize_decimal("0.0011", 3).unwrap_err();
        assert!(err.to_string().contains("more fractional"));
        let err = normalize_decimal("1.25", 0).unwrap_err();
        assert!(err.to_string().contains("more fractional"));
    }

    #[test]
    fn normalize_rejects_invalid_strings() {
        for bad in [
            "",
            "   ",
            "-1.0",
            "-",
            "+",
            "1e2",
            "1E-3",
            "abc",
            "1.2.3",
            "1..2",
            ".5",
            "1,25",
            "0x10",
            "NaN",
            "inf",
            "1 2",
        ] {
            assert!(
                normalize_decimal(bad, 8).is_err(),
                "expected reject for {bad:?}"
            );
        }
    }

    #[test]
    fn normalize_opt_price_and_quantity() {
        let d = InstrumentDecimals {
            price_decimals: 1,
            quantity_decimals: 3,
        };
        assert_eq!(
            normalize_opt_price(Some("100.50"), d).unwrap_err().to_string(),
            normalize_decimal("100.50", 1).unwrap_err().to_string()
        );
        assert_eq!(
            normalize_opt_price(Some("100.5"), d).unwrap().as_deref(),
            Some("100.5")
        );
        assert_eq!(normalize_opt_price(None, d).unwrap(), None);
        assert!(normalize_opt_price(Some("100.55"), d).is_err());
        assert_eq!(
            normalize_opt_quantity(Some("0.010"), d)
                .unwrap()
                .as_deref(),
            Some("0.01")
        );
        assert!(normalize_opt_quantity(Some("0.0001"), d).is_err());
    }
}
