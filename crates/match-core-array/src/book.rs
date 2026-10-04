//! 数组+价格偏移订单簿。
//!
//! 核心机制：
//! 1. **直接寻址**：`idx = tick - tick_lo`，O(1) 定位档位，零指针跳转
//! 2. **非空档位位图**：每 bit 代表一个档位是否有挂单，`CTZ`/`CLZ` 硬件指令 1 周期找下一档
//! 3. **最优价指针**：`best_bid_idx` / `best_ask_idx` 整数，O(1) 读取最优价
//! 4. **连续内存**：数组顺序遍历，CPU 硬件预取器自动加载后续缓存行

use crate::level::PriceLevel;
use crate::types::{ArrayError, BookConfig};
use match_core_hp::{OrderStore, Side};

/// 数组+价格偏移订单簿。
pub struct ArrayBook {
    config: BookConfig,
    /// 买盘档位数组（下标 = tick - tick_lo）。
    bids: Vec<PriceLevel>,
    /// 卖盘档位数组。
    asks: Vec<PriceLevel>,
    /// 买盘非空档位位图（每 bit 一个档位）。
    bid_bitmap: Vec<u64>,
    /// 卖盘非空档位位图。
    ask_bitmap: Vec<u64>,
    /// 最优买价下标（None = 无买盘）。
    best_bid_idx: Option<usize>,
    /// 最优卖价下标（None = 无卖盘）。
    best_ask_idx: Option<usize>,
    /// 订单存储（复用 hp-engine 的 OrderStore）。
    store: OrderStore,
}

impl ArrayBook {
    /// 创建订单簿，预分配数组和位图。
    pub fn new(config: BookConfig) -> Self {
        let n = config.level_count();
        let bw = config.bitmap_words();
        Self {
            config,
            bids: (0..n).map(|_| PriceLevel::default()).collect(),
            asks: (0..n).map(|_| PriceLevel::default()).collect(),
            bid_bitmap: vec![0u64; bw],
            ask_bitmap: vec![0u64; bw],
            best_bid_idx: None,
            best_ask_idx: None,
            store: OrderStore::new(),
        }
    }

    /// 带预分配容量的订单簿。
    pub fn with_capacity(config: BookConfig, order_cap: usize) -> Self {
        let n = config.level_count();
        let bw = config.bitmap_words();
        Self {
            config,
            bids: (0..n).map(|_| PriceLevel::default()).collect(),
            asks: (0..n).map(|_| PriceLevel::default()).collect(),
            bid_bitmap: vec![0u64; bw],
            ask_bitmap: vec![0u64; bw],
            best_bid_idx: None,
            best_ask_idx: None,
            store: OrderStore::with_capacity(order_cap),
        }
    }

    #[inline]
    pub fn config(&self) -> BookConfig {
        self.config
    }

    #[inline]
    pub fn store(&self) -> &OrderStore {
        &self.store
    }

    #[inline]
    pub fn store_mut(&mut self) -> &mut OrderStore {
        &mut self.store
    }

    /// 最优买价 tick。
    #[inline]
    pub fn best_bid_tick(&self) -> Option<i64> {
        self.best_bid_idx.map(|i| self.config.idx_to_tick(i))
    }

    /// 最优卖价 tick。
    #[inline]
    pub fn best_ask_tick(&self) -> Option<i64> {
        self.best_ask_idx.map(|i| self.config.idx_to_tick(i))
    }

    /// 买盘档位引用。
    #[inline]
    pub fn bid_level(&self, tick: i64) -> Option<&PriceLevel> {
        let idx = self.config.tick_to_idx(tick)?;
        let level = &self.bids[idx];
        if level.is_empty() { None } else { Some(level) }
    }

    /// 卖盘档位引用。
    #[inline]
    pub fn ask_level(&self, tick: i64) -> Option<&PriceLevel> {
        let idx = self.config.tick_to_idx(tick)?;
        let level = &self.asks[idx];
        if level.is_empty() { None } else { Some(level) }
    }

    /// 插入限价单到订单簿，返回订单 ID。
    pub fn insert_limit(&mut self, order: match_core_hp::HpOrder) -> Result<u64, ArrayError> {
        let side = order.side;
        let tick = order.price_tick;
        let lot = order.open_lot;
        let idx = self
            .config
            .tick_to_idx(tick)
            .ok_or(ArrayError::PriceOutOfRange {
                tick,
                lo: self.config.tick_lo,
                hi: self.config.tick_hi,
            })?;

        let id = self.store.insert(order);
        match side {
            Side::Buy => {
                let level = &mut self.bids[idx];
                let was_empty = level.is_empty();
                level.push(id, lot);
                if was_empty {
                    set_bit(&mut self.bid_bitmap, idx);
                    if self.best_bid_idx.map_or(true, |b| idx > b) {
                        self.best_bid_idx = Some(idx);
                    }
                }
            }
            Side::Sell => {
                let level = &mut self.asks[idx];
                let was_empty = level.is_empty();
                level.push(id, lot);
                if was_empty {
                    set_bit(&mut self.ask_bitmap, idx);
                    if self.best_ask_idx.map_or(true, |b| idx < b) {
                        self.best_ask_idx = Some(idx);
                    }
                }
            }
        }
        Ok(id)
    }

    /// 撤销订单，返回被撤销的订单。
    pub fn cancel_order(&mut self, id: u64) -> Result<match_core_hp::HpOrder, ArrayError> {
        let order = self
            .store
            .get(id)
            .cloned()
            .ok_or(ArrayError::OrderNotFound(id))?;
        let side = order.side;
        let tick = order.price_tick;
        let idx = self
            .config
            .tick_to_idx(tick)
            .ok_or(ArrayError::PriceOutOfRange {
                tick,
                lo: self.config.tick_lo,
                hi: self.config.tick_hi,
            })?;

        let level_count = self.config.level_count();
        match side {
            Side::Buy => {
                let level = &mut self.bids[idx];
                let removed = level.remove_id(id);
                if !removed {
                    return Err(ArrayError::OrderNotResting(id));
                }
                level.total_lot -= order.open_lot;
                if level.is_empty() {
                    clear_bit(&mut self.bid_bitmap, idx);
                    if self.best_bid_idx == Some(idx) {
                        self.best_bid_idx = find_last_set(&self.bid_bitmap, idx.saturating_sub(1));
                    }
                }
            }
            Side::Sell => {
                let level = &mut self.asks[idx];
                let removed = level.remove_id(id);
                if !removed {
                    return Err(ArrayError::OrderNotResting(id));
                }
                level.total_lot -= order.open_lot;
                if level.is_empty() {
                    clear_bit(&mut self.ask_bitmap, idx);
                    if self.best_ask_idx == Some(idx) {
                        self.best_ask_idx =
                            find_first_set(&self.ask_bitmap, idx + 1, level_count);
                    }
                }
            }
        }
        self.store.remove(id);
        Ok(order)
    }

    /// 匹配限价买单（吃卖盘），返回成交事件数量。
    /// 调用方需传入事件缓冲区。
    pub fn match_bid(
        &mut self,
        price_tick: i64,
        qty_lot: i64,
        taker_id: u64,
        taker_client_id: u64,
        events: &mut Vec<match_core_hp::HpEvent>,
    ) -> i64 {
        let mut remaining = qty_lot;
        let mut cur_idx = match self.best_ask_idx {
            Some(i) => i,
            None => return 0,
        };
        let max_idx = self.config.tick_to_idx(price_tick).unwrap_or(self.config.level_count() - 1);

        while remaining > 0 && cur_idx <= max_idx {
            let level = &mut self.asks[cur_idx];
            while remaining > 0 && !level.is_empty() {
                let maker_id = level.front_id().unwrap();
                let maker = self.store.get(maker_id).cloned().unwrap();
                let fill = remaining.min(maker.open_lot);

                // 更新 maker 剩余量
                let maker_order = self.store.get_mut(maker_id).unwrap();
                maker_order.open_lot -= fill;
                level.total_lot -= fill;
                remaining -= fill;

                events.push(match_core_hp::HpEvent::Fill {
                    maker_id,
                    taker_id,
                    maker_client_id: maker.client_id,
                    taker_client_id,
                    price_tick: maker.price_tick,
                    qty_lot: fill,
                    maker_open_lot: maker.open_lot - fill,
                    taker_open_lot: remaining,
                });

                if maker_order.open_lot == 0 {
                    level.pop_front();
                    self.store.remove(maker_id);
                }
            }

            if level.is_empty() {
                clear_bit(&mut self.ask_bitmap, cur_idx);
                if cur_idx == self.best_ask_idx.unwrap_or(usize::MAX) {
                    self.best_ask_idx =
                        find_first_set(&self.ask_bitmap, cur_idx + 1, self.config.level_count());
                }
            }

            // 找下一个非空卖档
            match find_first_set(&self.ask_bitmap, cur_idx + 1, self.config.level_count()) {
                Some(next) => cur_idx = next,
                None => break,
            }
        }
        qty_lot - remaining
    }

    /// 匹配限价卖单（吃买盘）。
    pub fn match_ask(
        &mut self,
        price_tick: i64,
        qty_lot: i64,
        taker_id: u64,
        taker_client_id: u64,
        events: &mut Vec<match_core_hp::HpEvent>,
    ) -> i64 {
        let mut remaining = qty_lot;
        let mut cur_idx = match self.best_bid_idx {
            Some(i) => i,
            None => return 0,
        };
        let min_idx = self.config.tick_to_idx(price_tick).unwrap_or(0);

        while remaining > 0 && cur_idx >= min_idx {
            let level = &mut self.bids[cur_idx];
            while remaining > 0 && !level.is_empty() {
                let maker_id = level.front_id().unwrap();
                let maker = self.store.get(maker_id).cloned().unwrap();
                let fill = remaining.min(maker.open_lot);

                let maker_order = self.store.get_mut(maker_id).unwrap();
                maker_order.open_lot -= fill;
                level.total_lot -= fill;
                remaining -= fill;

                events.push(match_core_hp::HpEvent::Fill {
                    maker_id,
                    taker_id,
                    maker_client_id: maker.client_id,
                    taker_client_id,
                    price_tick: maker.price_tick,
                    qty_lot: fill,
                    maker_open_lot: maker.open_lot - fill,
                    taker_open_lot: remaining,
                });

                if maker_order.open_lot == 0 {
                    level.pop_front();
                    self.store.remove(maker_id);
                }
            }

            if level.is_empty() {
                clear_bit(&mut self.bid_bitmap, cur_idx);
                if cur_idx == self.best_bid_idx.unwrap_or(usize::MAX) {
                    self.best_bid_idx = find_last_set(&self.bid_bitmap, cur_idx.saturating_sub(1));
                }
            }

            if cur_idx == 0 {
                break;
            }
            match find_last_set(&self.bid_bitmap, cur_idx - 1) {
                Some(prev) => cur_idx = prev,
                None => break,
            }
        }
        qty_lot - remaining
    }

    /// 行情快照：前 n 档买卖盘。
    pub fn depth(&self, n: usize) -> (Vec<(i64, i64)>, Vec<(i64, i64)>) {
        let mut bids = Vec::with_capacity(n);
        let mut asks = Vec::with_capacity(n);

        // 买盘：从最优买价向下（价格递减）
        if let Some(mut idx) = self.best_bid_idx {
            for _ in 0..n {
                let level = &self.bids[idx];
                if !level.is_empty() {
                    bids.push((self.config.idx_to_tick(idx), level.total_lot));
                }
                if idx == 0 {
                    break;
                }
                match find_last_set(&self.bid_bitmap, idx - 1) {
                    Some(prev) => idx = prev,
                    None => break,
                }
            }
        }

        // 卖盘：从最优卖价向上（价格递增）
        if let Some(mut idx) = self.best_ask_idx {
            for _ in 0..n {
                let level = &self.asks[idx];
                if !level.is_empty() {
                    asks.push((self.config.idx_to_tick(idx), level.total_lot));
                }
                match find_first_set(&self.ask_bitmap, idx + 1, self.config.level_count()) {
                    Some(next) => idx = next,
                    None => break,
                }
            }
        }

        (bids, asks)
    }

}

// ─── 位图操作（内联，CTZ/CLZ 硬件指令）───

#[inline]
fn set_bit(bitmap: &mut [u64], idx: usize) {
    bitmap[idx >> 6] |= 1u64 << (idx & 63);
}

#[inline]
fn clear_bit(bitmap: &mut [u64], idx: usize) {
    bitmap[idx >> 6] &= !(1u64 << (idx & 63));
}

/// 从 `start` 开始找第一个置位 bit（用于卖盘向上找档）。
/// 使用 `ctz`（Count Trailing Zeros）硬件指令，x86 上为 `TZCNT`/`BSF` 单指令。
#[inline]
fn find_first_set(bitmap: &[u64], start: usize, max: usize) -> Option<usize> {
    if start >= max {
        return None;
    }
    let word_idx = start >> 6;
    let bit_offset = start & 63;
    // 第一个字：屏蔽掉 start 之前的 bit
    let first_word = bitmap[word_idx] >> bit_offset;
    if first_word != 0 {
        let bit = first_word.trailing_zeros() as usize;
        return Some((word_idx << 6) + bit_offset + bit);
    }
    // 后续字：完整字扫描
    for i in (word_idx + 1)..bitmap.len() {
        let w = bitmap[i];
        if w != 0 {
            let bit = w.trailing_zeros() as usize;
            let idx = (i << 6) + bit;
            if idx < max {
                return Some(idx);
            }
            return None;
        }
    }
    None
}

/// 从 `end` 开始向前找最后一个置位 bit（用于买盘向下找档）。
/// 使用 `clz`（Count Leading Zeros）硬件指令，x86 上为 `LZCNT`/`BSR` 单指令。
#[inline]
fn find_last_set(bitmap: &[u64], end: usize) -> Option<usize> {
    let word_idx = end >> 6;
    if word_idx >= bitmap.len() {
        return None;
    }
    let bit_offset = end & 63;
    // 第一个字：屏蔽掉 end 之后的 bit
    let mask = if bit_offset == 63 { u64::MAX } else { (1u64 << (bit_offset + 1)) - 1 };
    let first_word = bitmap[word_idx] & mask;
    if first_word != 0 {
        let bit = 63 - first_word.leading_zeros() as usize;
        return Some((word_idx << 6) + bit);
    }
    // 向前扫描完整字
    for i in (0..word_idx).rev() {
        let w = bitmap[i];
        if w != 0 {
            let bit = 63 - w.leading_zeros() as usize;
            return Some((i << 6) + bit);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use match_core_hp::{HpOrder, Side};

    fn test_config() -> BookConfig {
        BookConfig::new(0, 10000).unwrap()
    }

    fn make_order(side: Side, tick: i64, lot: i64, client_id: u64) -> HpOrder {
        HpOrder::limit(side, tick, lot, client_id)
    }

    #[test]
    fn insert_and_best() {
        let mut book = ArrayBook::new(test_config());
        book.insert_limit(make_order(Side::Buy, 5000, 10, 1)).unwrap();
        book.insert_limit(make_order(Side::Buy, 4999, 5, 2)).unwrap();
        book.insert_limit(make_order(Side::Sell, 5001, 8, 3)).unwrap();

        assert_eq!(book.best_bid_tick(), Some(5000));
        assert_eq!(book.best_ask_tick(), Some(5001));
    }

    #[test]
    fn match_bid_fills_across_levels() {
        let mut book = ArrayBook::new(test_config());
        book.insert_limit(make_order(Side::Sell, 5001, 5, 1)).unwrap();
        book.insert_limit(make_order(Side::Sell, 5002, 5, 2)).unwrap();

        let mut events = Vec::new();
        let filled = book.match_bid(5002, 8, 99, 100, &mut events);
        assert_eq!(filled, 8);
        assert_eq!(events.len(), 2);
        // 5001 档全部成交，5002 档成交 3
        assert_eq!(book.best_ask_tick(), Some(5002));
    }

    #[test]
    fn cancel_updates_best() {
        let mut book = ArrayBook::new(test_config());
        let id1 = book.insert_limit(make_order(Side::Buy, 5000, 10, 1)).unwrap();
        let _id2 = book.insert_limit(make_order(Side::Buy, 4999, 5, 2)).unwrap();

        assert_eq!(book.best_bid_tick(), Some(5000));
        book.cancel_order(id1).unwrap();
        assert_eq!(book.best_bid_tick(), Some(4999));
    }

    #[test]
    fn price_out_of_range() {
        let book = ArrayBook::new(test_config());
        let result = book.config.tick_to_idx(10001);
        assert!(result.is_none());
    }

    #[test]
    fn depth_snapshot() {
        let mut book = ArrayBook::new(test_config());
        book.insert_limit(make_order(Side::Buy, 5000, 10, 1)).unwrap();
        book.insert_limit(make_order(Side::Buy, 4999, 5, 2)).unwrap();
        book.insert_limit(make_order(Side::Sell, 5001, 8, 3)).unwrap();

        let (bids, asks) = book.depth(5);
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0], (5000, 10));
        assert_eq!(bids[1], (4999, 5));
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0], (5001, 8));
    }

    #[test]
    fn bitmap_find_first_set() {
        let mut bm = vec![0u64; 2];
        set_bit(&mut bm, 5);
        set_bit(&mut bm, 70);
        assert_eq!(find_first_set(&bm, 0, 128), Some(5));
        assert_eq!(find_first_set(&bm, 6, 128), Some(70));
        assert_eq!(find_first_set(&bm, 71, 128), None);
    }

    #[test]
    fn bitmap_find_last_set() {
        let mut bm = vec![0u64; 2];
        set_bit(&mut bm, 5);
        set_bit(&mut bm, 70);
        assert_eq!(find_last_set(&bm, 127), Some(70));
        assert_eq!(find_last_set(&bm, 69), Some(5));
        assert_eq!(find_last_set(&bm, 4), None);
    }
}
