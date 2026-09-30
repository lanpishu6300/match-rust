//! Handicap depth builder aligned with bf-match `NoDealProducer.getDepth`.

use bigdecimal::{BigDecimal, Zero};
use match_protocol::BbOrder;

use crate::outbound::DepthLevel;

fn dec_str(d: &BigDecimal) -> String {
    d.normalized().to_string()
}

/// Walk resting orders (price-time order) and build up to `limit` depth levels.
///
/// Returns `(levels, sum_trust_number)` where the sum uses Java
/// `allCumulativeCommissionQuantity.add(bbOrder.getTrustNumber())`.
pub fn build_depth_levels(
    orders: impl IntoIterator<Item = BbOrder>,
    limit: usize,
) -> (Vec<DepthLevel>, BigDecimal) {
    let mut send_list: Vec<DepthLevel> = Vec::new();
    let mut sum_trust = BigDecimal::zero();
    let mut orders = orders.into_iter();

    while send_list.len() < limit {
        let Some(order) = orders.next() else {
            break;
        };
        if order.remaining_number <= BigDecimal::zero() {
            continue;
        }
        sum_trust += &order.trust_number;

        let level = DepthLevel {
            trust_price: dec_str(&order.trust_price),
            cumulative_transaction_volume: dec_str(&order.consumer_all_number),
            cumulative_commission_quantity: dec_str(&order.remaining_number),
        };

        if let Some(prev) = send_list.last_mut() {
            if prev.trust_price == level.trust_price {
                merge_same_price_level(prev, &level);
                continue;
            }
        }
        send_list.push(level);
    }

    (send_list, sum_trust)
}

#[cfg_attr(coverage, coverage(off))]
fn merge_same_price_level(prev: &mut DepthLevel, level: &DepthLevel) {
    if let (Ok(a), Ok(b)) = (
        prev.cumulative_transaction_volume.parse::<BigDecimal>(),
        level.cumulative_transaction_volume.parse::<BigDecimal>(),
    ) {
        prev.cumulative_transaction_volume = dec_str(&(a + b));
    }
    if let (Ok(a), Ok(b)) = (
        prev.cumulative_commission_quantity.parse::<BigDecimal>(),
        level.cumulative_commission_quantity.parse::<BigDecimal>(),
    ) {
        prev.cumulative_commission_quantity = dec_str(&(a + b));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use match_core::Side;
    use std::str::FromStr;

    fn order(no: &str, price: &str, trust: &str, consumed: &str, remaining: &str) -> BbOrder {
        let mut o = match_core::BbOrder::test_limit(
            Side::Buy,
            BigDecimal::from_str(price).unwrap(),
            no,
            1,
            trust,
        )
        .0;
        o.consumer_all_number = BigDecimal::from_str(consumed).unwrap();
        o.remaining_number = BigDecimal::from_str(remaining).unwrap();
        o
    }

    #[test]
    fn merges_same_price_and_sums_trust_number() {
        let orders = vec![
            order("1", "10", "5", "1", "4"),
            order("2", "10", "5", "1", "4"),
            order("3", "12", "5", "0", "5"),
        ];
        let (levels, sum) = build_depth_levels(orders, 25);
        assert_eq!(levels.len(), 2);
        assert_eq!(levels[0].trust_price, "10");
        assert_eq!(levels[0].cumulative_commission_quantity, "8");
        assert_eq!(levels[0].cumulative_transaction_volume, "2");
        assert_eq!(sum, BigDecimal::from_str("15").unwrap());
    }

    #[test]
    fn stops_at_no_deal_limit() {
        let orders: Vec<BbOrder> = (0..30)
            .map(|i| {
                order(
                    &format!("o{i}"),
                    &format!("{}", 100 + i),
                    "1",
                    "0",
                    "1",
                )
            })
            .collect();
        let (levels, _) = build_depth_levels(orders, 25);
        assert_eq!(levels.len(), 25);
    }

    #[test]
    fn different_price_levels_do_not_merge() {
        let orders = vec![
            order("1", "10", "5", "1", "4"),
            order("2", "12", "5", "0", "5"),
        ];
        let (levels, _) = build_depth_levels(orders, 25);
        assert_eq!(levels.len(), 2);
    }

    #[test]
    fn empty_input_returns_empty_levels() {
        let (levels, sum) = build_depth_levels(std::iter::empty::<BbOrder>(), 25);
        assert!(levels.is_empty());
        assert_eq!(sum, BigDecimal::zero());
    }

    #[test]
    fn skips_zero_remaining_orders() {
        let orders = vec![
            order("1", "10", "5", "5", "0"),
            order("2", "11", "3", "0", "3"),
        ];
        let (levels, sum) = build_depth_levels(orders, 25);
        assert_eq!(levels.len(), 1);
        assert_eq!(levels[0].trust_price, "11");
        assert_eq!(sum, BigDecimal::from_str("3").unwrap());
    }

    #[test]
    fn exactly_limit_distinct_prices() {
        let orders: Vec<BbOrder> = (0..25)
            .map(|i| order(&format!("o{i}"), &format!("{}", 100 + i), "1", "0", "1"))
            .collect();
        let (levels, _) = build_depth_levels(orders.clone(), 25);
        assert_eq!(levels.len(), 25);
        let (levels, _) = build_depth_levels(orders, 24);
        assert_eq!(levels.len(), 24);
    }

    #[test]
    fn sum_uses_trust_number_not_remaining() {
        let orders = vec![order("1", "10", "7", "2", "5")];
        let (_, sum) = build_depth_levels(orders, 25);
        assert_eq!(sum, BigDecimal::from_str("7").unwrap());
    }
}
