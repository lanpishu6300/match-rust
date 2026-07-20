use bigdecimal::{BigDecimal, RoundingMode, Zero};

/// Weighted average price — Java `PriceUtils.getAveragePrice` (scale 16, HALF_DOWN).
pub fn get_average_price(
    amount: &BigDecimal,
    price: &BigDecimal,
    now_amount: &BigDecimal,
    now_price: &BigDecimal,
) -> BigDecimal {
    let total_amount = price * amount + now_price * now_amount;
    let total_quantity = amount + now_amount;
    if total_quantity.is_zero() {
        return BigDecimal::zero();
    }
    (total_amount / total_quantity)
        .with_scale_round(16, RoundingMode::HalfDown)
        .normalized()
}

#[cfg(test)]
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn dec(s: &str) -> BigDecimal {
        BigDecimal::from_str(s).unwrap()
    }

    #[test]
    fn weighted_average_matches_java_half_down() {
        let avg = get_average_price(&dec("1"), &dec("100"), &dec("1"), &dec("102"));
        assert_eq!(avg, dec("101"));
    }

    #[test]
    fn zero_total_quantity_returns_zero() {
        let avg = get_average_price(
            &BigDecimal::zero(),
            &BigDecimal::zero(),
            &BigDecimal::zero(),
            &BigDecimal::zero(),
        );
        assert_eq!(avg, BigDecimal::zero());
    }
}
