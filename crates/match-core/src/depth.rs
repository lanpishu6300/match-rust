use bigdecimal::{BigDecimal, Zero};

use crate::order::BbOrder;

/// Build up to `limit` price levels with qty aggregated per level (Java `NoDealProducer.getDepth`).
pub fn depth_levels_from_orders(
    orders: impl IntoIterator<Item = BbOrder>,
    limit: usize,
) -> Vec<(BigDecimal, BigDecimal)> {
    if limit == 0 {
        return Vec::new();
    }

    let mut levels: Vec<(BigDecimal, BigDecimal)> = Vec::new();
    for order in orders {
        if order.remaining_number <= BigDecimal::zero() {
            continue;
        }
        push_depth_level(
            &mut levels,
            limit,
            &order.trust_price,
            &order.remaining_number,
        );
    }
    levels
}

fn push_depth_level(
    levels: &mut Vec<(BigDecimal, BigDecimal)>,
    limit: usize,
    price: &BigDecimal,
    qty: &BigDecimal,
) {
    if let Some((last_price, last_qty)) = levels.last_mut() {
        if last_price == price {
            *last_qty += qty;
            return;
        }
    }
    if levels.len() < limit {
        levels.push((price.clone(), qty.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::order::Side;
    use std::str::FromStr;

    fn dec(s: &str) -> BigDecimal {
        BigDecimal::from_str(s).unwrap()
    }

    #[test]
    fn aggregates_same_price_and_limits_levels() {
        let orders = vec![
            BbOrder::test_limit(Side::Buy, dec("100"), "1", 1, "1"),
            BbOrder::test_limit(Side::Buy, dec("100"), "2", 2, "2"),
            BbOrder::test_limit(Side::Buy, dec("99"), "3", 3, "1"),
        ];
        let levels = depth_levels_from_orders(orders, 1);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].0, dec("100"));
        assert_eq!(levels[0].1, dec("3"));
    }

    #[test]
    fn skips_zero_remaining() {
        let mut o = BbOrder::test_limit(Side::Buy, dec("100"), "1", 1, "1");
        o.remaining_number = BigDecimal::zero();
        let levels = depth_levels_from_orders(vec![o], 20);
        assert!(levels.is_empty());
    }
}
