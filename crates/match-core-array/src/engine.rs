//! 数组撮合引擎：封装 ArrayBook，提供与 HpEngine 对齐的命令接口。
//!
//! 支持 Limit / Cancel / Market 命令，输出 HpEvent 事件。
//! 内部维护 client_id → 在途订单列表映射，支持多在途订单。

use crate::book::ArrayBook;
use crate::types::{ArrayError, BookConfig};
use match_core_hp::{HpCommand, HpEvent, HpOrder, Side};
use rustc_hash::FxHashMap;

/// 临时 taker ID 基址（高位区间，与 store slot id（从 1 开始的小整数）不冲突）。
const TAKER_ID_BASE: u64 = 1u64 << 48;

/// 数组+价格偏移撮合引擎。
pub struct ArrayEngine {
    book: ArrayBook,
    /// client_id → 在途订单 ID 列表（支持多在途）。
    client_to_ids: FxHashMap<u64, Vec<u64>>,
    /// 事件输出缓冲区（复用，避免每单分配）。
    events: Vec<HpEvent>,
    /// 自增序列号（时间优先）。
    seq: u64,
}

impl ArrayEngine {
    /// 创建引擎，默认价格范围 0..=1_000_000 tick。
    pub fn new() -> Self {
        let config = BookConfig::new(0, 1_000_000).expect("default config valid");
        Self::with_config(config, 4096)
    }

    /// 带配置和预分配容量创建。
    pub fn with_config(config: BookConfig, order_cap: usize) -> Self {
        Self {
            book: ArrayBook::with_capacity(config, order_cap),
            client_to_ids: FxHashMap::default(),
            events: Vec::with_capacity(256),
            seq: 0,
        }
    }

    /// 订单簿引用。
    #[inline]
    pub fn book(&self) -> &ArrayBook {
        &self.book
    }

    /// 订单簿可变引用。
    #[inline]
    pub fn book_mut(&mut self) -> &mut ArrayBook {
        &mut self.book
    }

    /// 最优买价。
    #[inline]
    pub fn best_bid_tick(&self) -> Option<i64> {
        self.book.best_bid_tick()
    }

    /// 最优卖价。
    #[inline]
    pub fn best_ask_tick(&self) -> Option<i64> {
        self.book.best_ask_tick()
    }

    /// 在途订单数。
    #[inline]
    pub fn live_orders(&self) -> usize {
        self.book.store().live_len()
    }

    /// client 在途订单列表。
    #[inline]
    pub fn client_orders(&self, client_id: u64) -> &[u64] {
        self.client_to_ids.get(&client_id).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// 处理一条命令，返回事件切片（引用内部缓冲区，下一次 on_order 前有效）。
    pub fn on_order(&mut self, cmd: HpCommand) -> Result<&[HpEvent], ArrayError> {
        self.events.clear();
        match cmd {
            HpCommand::Limit {
                side,
                price_tick,
                qty_lot,
                ts,
                client_id,
            } => self.handle_limit(side, price_tick, qty_lot, ts, client_id)?,
            HpCommand::Cancel { id } => self.handle_cancel(id)?,
            HpCommand::Market {
                side,
                qty_lot,
                ts: _ts,
                max_fills,
                client_id,
            } => self.handle_market(side, qty_lot, max_fills, client_id)?,
        }
        Ok(&self.events)
    }

    fn next_seq(&mut self) -> u64 {
        self.seq += 1;
        self.seq
    }

    fn handle_limit(
        &mut self,
        side: Side,
        price_tick: i64,
        qty_lot: i64,
        _ts: u64,
        client_id: u64,
    ) -> Result<(), ArrayError> {
        let seq = self.next_seq();

        // 判断是否可吃对手盘
        let can_cross = match side {
            Side::Buy => self.book.best_ask_tick().map_or(false, |ask| price_tick >= ask),
            Side::Sell => self.book.best_bid_tick().map_or(false, |bid| price_tick <= bid),
        };

        let remaining = if can_cross {
            // 优化：taker 不 insert 到 store，用高位临时 ID（u64::MAX/2 以上区间，与 store slot id 不冲突）
            // match_bid/match_ask 只用到 taker_id 填充事件，不从 store 读取 taker
            let taker_id = TAKER_ID_BASE + seq;
            let filled = match side {
                Side::Buy => {
                    self.book.match_bid(price_tick, qty_lot, taker_id, client_id, &mut self.events)
                }
                Side::Sell => {
                    self.book.match_ask(price_tick, qty_lot, taker_id, client_id, &mut self.events)
                }
            };
            qty_lot - filled
        } else {
            qty_lot
        };

        // 有剩余则挂单（cross_full 场景 remaining=0，跳过 insert，零开销）
        if remaining > 0 {
            let order = HpOrder {
                id: 0,
                side,
                price_tick,
                qty_lot,
                open_lot: remaining,
                ts: seq,
                client_id,
            };
            let id = self.book.insert_limit(order)?;
            self.client_to_ids.entry(client_id).or_default().push(id);
            self.events.push(HpEvent::Rest {
                id,
                client_id,
                side,
                price_tick,
                qty_lot: remaining,
            });
        }
        Ok(())
    }

    fn handle_cancel(&mut self, id: u64) -> Result<(), ArrayError> {
        let order = self.book.cancel_order(id)?;
        // 从 client 列表移除
        if let Some(ids) = self.client_to_ids.get_mut(&order.client_id) {
            ids.retain(|&x| x != id);
        }
        self.events.push(HpEvent::Revoke {
            id,
            client_id: order.client_id,
            reason: 0, // user cancel
        });
        Ok(())
    }

    fn handle_market(
        &mut self,
        side: Side,
        qty_lot: i64,
        _max_fills: Option<u32>,
        client_id: u64,
    ) -> Result<(), ArrayError> {
        let seq = self.next_seq();
        // 市价单：用极端价格吃对手盘
        let cross_price = match side {
            Side::Buy => self.book.config().tick_hi,
            Side::Sell => self.book.config().tick_lo,
        };
        // 优化：taker 不 insert store，用临时 ID
        let taker_id = TAKER_ID_BASE + seq;

        let filled = match side {
            Side::Buy => self.book.match_bid(cross_price, qty_lot, taker_id, client_id, &mut self.events),
            Side::Sell => self.book.match_ask(cross_price, qty_lot, taker_id, client_id, &mut self.events),
        };

        // 市价单未成交部分自动撤销
        if filled < qty_lot {
            self.events.push(HpEvent::Revoke {
                id: taker_id,
                client_id,
                reason: 1, // market leftover
            });
        }
        Ok(())
    }
}

impl Default for ArrayEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::BookConfig;

    fn engine() -> ArrayEngine {
        ArrayEngine::with_config(BookConfig::new(0, 100000).unwrap(), 1024)
    }

    #[test]
    fn limit_order_rests() {
        let mut e = engine();
        let events = e
            .on_order(HpCommand::Limit {
                side: Side::Buy,
                price_tick: 5000,
                qty_lot: 10,
                ts: 0,
                client_id: 1,
            })
            .unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], HpEvent::Rest { .. }));
        assert_eq!(e.live_orders(), 1);
        assert_eq!(e.best_bid_tick(), Some(5000));
    }

    #[test]
    fn limit_order_matches() {
        let mut e = engine();
        // 挂卖单
        e.on_order(HpCommand::Limit {
            side: Side::Sell,
            price_tick: 5001,
            qty_lot: 5,
            ts: 0,
            client_id: 1,
        })
        .unwrap();
        // 买单吃卖单
        let events = e
            .on_order(HpCommand::Limit {
                side: Side::Buy,
                price_tick: 5001,
                qty_lot: 3,
                ts: 0,
                client_id: 2,
            })
            .unwrap();
        let fills: Vec<_> = events.iter().filter(|e| matches!(e, HpEvent::Fill { .. })).collect();
        assert_eq!(fills.len(), 1);
        if let HpEvent::Fill { qty_lot, .. } = fills[0] {
            assert_eq!(*qty_lot, 3);
        }
        // 卖单还剩 2
        assert_eq!(e.live_orders(), 1);
    }

    #[test]
    fn cancel_order() {
        let mut e = engine();
        let events = e
            .on_order(HpCommand::Limit {
                side: Side::Buy,
                price_tick: 5000,
                qty_lot: 10,
                ts: 0,
                client_id: 1,
            })
            .unwrap();
        let id = match events[0] {
            HpEvent::Rest { id, .. } => id,
            _ => panic!("expected Rest"),
        };
        let cancel_events = e.on_order(HpCommand::Cancel { id }).unwrap();
        assert!(matches!(cancel_events[0], HpEvent::Revoke { reason: 0, .. }));
        assert_eq!(e.live_orders(), 0);
    }

    #[test]
    fn multiple_resting_orders_per_client() {
        let mut e = engine();
        e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 5000,
            qty_lot: 1,
            ts: 0,
            client_id: 42,
        })
        .unwrap();
        e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 4999,
            qty_lot: 1,
            ts: 0,
            client_id: 42,
        })
        .unwrap();
        assert_eq!(e.client_orders(42).len(), 2);
        assert_eq!(e.live_orders(), 2);
    }

    #[test]
    fn price_out_of_range_rejected() {
        let mut e = engine();
        let result = e.on_order(HpCommand::Limit {
            side: Side::Buy,
            price_tick: 999999,
            qty_lot: 1,
            ts: 0,
            client_id: 1,
        });
        assert!(result.is_err());
    }
}
