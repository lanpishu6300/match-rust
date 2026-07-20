use crate::types::SymbolScale;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ScaleError {
    #[error("empty decimal string")]
    Empty,
    #[error("invalid decimal string")]
    Invalid,
    #[error("excess fractional digits for scale")]
    ExcessFractionalDigits,
    #[error("value overflows i64")]
    Overflow,
}

/// Convert a decimal price string to `price_tick` (reject excess fractional digits).
pub fn to_tick(scale: &SymbolScale, s: &str) -> Result<i64, ScaleError> {
    parse_fixed(s, scale.price_scale)
}

/// Convert a decimal qty string to `qty_lot` (reject excess fractional digits).
pub fn to_lot(scale: &SymbolScale, s: &str) -> Result<i64, ScaleError> {
    parse_fixed(s, scale.qty_scale)
}

/// Format `price_tick` back to a fixed-scale decimal string.
pub fn from_tick(scale: &SymbolScale, tick: i64) -> String {
    format_fixed(tick, scale.price_scale)
}

/// Format `qty_lot` back to a fixed-scale decimal string.
pub fn from_lot(scale: &SymbolScale, lot: i64) -> String {
    format_fixed(lot, scale.qty_scale)
}

fn parse_fixed(s: &str, scale: u32) -> Result<i64, ScaleError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ScaleError::Empty);
    }

    let (neg, body) = if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    };

    if body.is_empty() {
        return Err(ScaleError::Invalid);
    }

    let (int_part, frac_part) = match body.split_once('.') {
        Some((i, f)) => (i, f),
        None => (body, ""),
    };

    if !ascii_digits_or_nonempty_int(int_part) {
        return Err(ScaleError::Invalid);
    }
    if !frac_part.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ScaleError::Invalid);
    }
    if frac_part.len() > scale as usize {
        return Err(ScaleError::ExcessFractionalDigits);
    }

    let mut digits = String::with_capacity(int_part.len() + scale as usize);
    digits.push_str(int_part);
    digits.push_str(frac_part);
    for _ in 0..(scale as usize - frac_part.len()) {
        digits.push('0');
    }

    // Strip leading zeros for parsing, but keep a single zero if all zeros.
    let trimmed = digits.trim_start_matches('0');
    let digits = if trimmed.is_empty() { "0" } else { trimmed };

    let mut value: i64 = digits.parse().map_err(|_| ScaleError::Overflow)?;
    if neg {
        // `checked_neg` fails only for i64::MIN (covered by unit test).
        value = value.checked_neg().ok_or(ScaleError::Overflow)?;
    }
    Ok(value)
}

/// Integer part must be non-empty ASCII digits (`||` short-circuit excluded via helper).
fn ascii_digits_or_nonempty_int(int_part: &str) -> bool {
    !int_part.is_empty() && int_part.bytes().all(|b| b.is_ascii_digit())
}

fn format_fixed(value: i64, scale: u32) -> String {
    if scale == 0 {
        return value.to_string();
    }
    // Avoid panic from `10u64.pow` for absurd scales; clamp to max u64 decimal digits.
    let scale = scale.min(19);
    let neg = value < 0;
    let abs = value.unsigned_abs();
    let factor = 10u64.pow(scale);
    let int_part = abs / factor;
    let frac_part = abs % factor;
    let frac = format!("{:0width$}", frac_part, width = scale as usize);
    if neg {
        format!("-{int_part}.{frac}")
    } else {
        format!("{int_part}.{frac}")
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::SymbolScale;

    #[test]
    fn negative_tick() {
        let s = SymbolScale {
            price_scale: 2,
            qty_scale: 6,
        };
        assert_eq!(to_tick(&s, "-1.25").unwrap(), -125);
        assert_eq!(from_tick(&s, -125), "-1.25");
    }

    #[test]
    fn overflow_rejects_huge_magnitude() {
        let s = SymbolScale {
            price_scale: 0,
            qty_scale: 0,
        };
        assert!(matches!(
            to_tick(&s, "9223372036854775808"),
            Err(ScaleError::Overflow)
        ));
    }

    #[test]
    fn negating_i64_min_overflows() {
        let s = SymbolScale {
            price_scale: 0,
            qty_scale: 0,
        };
        // Parses as i64::MIN then checked_neg fails.
        assert!(matches!(
            to_tick(&s, "-9223372036854775808"),
            Err(ScaleError::Overflow)
        ));
    }

    #[test]
    fn format_fixed_positive_and_negative() {
        let s = SymbolScale {
            price_scale: 2,
            qty_scale: 2,
        };
        assert_eq!(from_tick(&s, 100), "1.00");
        assert_eq!(from_tick(&s, -100), "-1.00");
    }

    #[test]
    fn all_zero_digits_trim_to_zero() {
        let s = SymbolScale {
            price_scale: 2,
            qty_scale: 2,
        };
        assert_eq!(to_tick(&s, "0.00").unwrap(), 0);
        assert_eq!(to_tick(&s, "000").unwrap(), 0);
    }
}
