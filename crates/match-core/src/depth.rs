use bigdecimal::{BigDecimal, Zero};

use crate::order::BbOrder;

/// Build up to `limit` price levels with qty aggregated per level (Java `NoDealProducer.getDepth`).
///
/// Iterates borrowed orders — the resting book is not deep-copied per level;
/// only the two BigDecimal values pushed into the output levels are cloned.
pub fn depth_levels_from_orders<'a>(
    orders: impl IntoIterator<Item = &'a BbOrder>,
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
#[cfg_attr(any(coverage, coverage_nightly), coverage(off))]
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
        let levels = depth_levels_from_orders(orders.iter(), 1);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].0, dec("100"));
        assert_eq!(levels[0].1, dec("3"));
    }

    #[test]
    fn skips_zero_remaining() {
        let mut o = BbOrder::test_limit(Side::Buy, dec("100"), "1", 1, "1");
        o.remaining_number = BigDecimal::zero();
        let levels = depth_levels_from_orders(vec![o].iter(), 20);
        assert!(levels.is_empty());
    }

    #[test]
    fn limit_zero_returns_empty() {
        let orders = vec![BbOrder::test_limit(Side::Buy, dec("100"), "1", 1, "1")];
        assert!(depth_levels_from_orders(orders.iter(), 0).is_empty());
    }

    #[test]
    fn stops_at_limit_without_aggregating_beyond() {
        let orders = vec![
            BbOrder::test_limit(Side::Sell, dec("100"), "1", 1, "1"),
            BbOrder::test_limit(Side::Sell, dec("101"), "2", 2, "1"),
            BbOrder::test_limit(Side::Sell, dec("102"), "3", 3, "1"),
        ];
        let levels = depth_levels_from_orders(orders.iter(), 2);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].0, dec("100"));
        assert_eq!(levels[1].0, dec("101"));
    }
}
