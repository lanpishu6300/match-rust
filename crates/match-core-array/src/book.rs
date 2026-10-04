//! 数组+价格偏移订单簿（动态滑动窗口版）。
//!
//! 核心机制：
//! 1. **动态窗口**：不预分配全价格范围，从 initial_levels 开始，按需向右扩展到 max_levels
//! 2. **直接寻址**：`idx = tick - window_base`，O(1) 定位档位
//! 3. **非空档位位图**：CTZ/CLZ 硬件指令 1 周期找下一档
//! 4. **最优价指针**：best_bid_idx / best_ask_idx 整数，O(1) 读取
//! 5. **连续内存**：窗口大小控制在 L2 缓存内，随机访问缓存命中率高

use crate::level::PriceLevel;
use crate::types::{ArrayError, BookConfig};
use match_core_hp::{OrderStore, Side};

/// 数组+价格偏移订单簿（动态滑动窗口）。
pub struct ArrayBook {
    config: BookConfig,
    /// 窗口左边界（tick 值），idx = tick - window_base。
    /// None 表示尚未设定（第一笔订单到达时设定）。
    window_base: Option<i64>,
    /// 买盘档位数组。
    bids: Vec<PriceLevel>,
    /// 卖盘档位数组。
    asks: Vec<PriceLevel>,
    /// 买盘非空档位位图。
    bid_bitmap: Vec<u64>,
    /// 卖盘非空档位位图。
    ask_bitmap: Vec<u64>,
    /// 最优买价下标。
    best_bid_idx: Option<usize>,
    /// 最优卖价下标。
    best_ask_idx: Option<usize>,
    /// 订单存储。
    store: OrderStore,
}

impl ArrayBook {
    /// 创建订单簿（动态窗口，初始大小由 config 决定）。
    pub fn new(config: BookConfig) -> Self {
        let n = config.initial_levels;
        let bw = (n + 63) / 64;
        Self {
            config,
            window_base: None,
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
        let n = config.initial_levels;
        let bw = (n + 63) / 64;
        Self {
            config,
            window_base: None,
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

    /// 当前窗口左边界 tick。
    #[inline]
    pub fn window_base(&self) -> Option<i64> {
        self.window_base
    }

    /// 当前窗口档位数量。
    #[inline]
    pub fn window_size(&self) -> usize {
        self.bids.len()
    }

    /// 最优买价 tick。
    #[inline]
    pub fn best_bid_tick(&self) -> Option<i64> {
        let base = self.window_base?;
        self.best_bid_idx.map(|i| base + i as i64)
    }

    /// 最优卖价 tick。
    #[inline]
    pub fn best_ask_tick(&self) -> Option<i64> {
        let base = self.window_base?;
        self.best_ask_idx.map(|i| base + i as i64)
    }

    /// tick → 数组下标，确保窗口足够大（按需扩展）。
    /// 返回 (idx, 是否新设定了 window_base)。
    fn ensure_idx(&mut self, tick: i64) -> Result<usize, ArrayError> {
        let base = match self.window_base {
            Some(b) => b,
            None => {
                // 第一笔订单：设定窗口左边界
                self.window_base = Some(tick);
                tick
            }
        };

        if tick < base {
            // 价格低于窗口左边界：需要向左扩展（rebase）
            // 简化处理：把 window_base 移到 tick，已有档位的 idx 全部偏移
            let shift = (base - tick) as usize;
            let new_size = (self.bids.len() + shift).min(self.config.max_levels);
            if new_size < shift + 1 {
                return Err(ArrayError::PriceExceedsMaxWindow {
                    tick,
                    base,
                    max_levels: self.config.max_levels,
                });
            }
            self.rebase_left(tick, shift, new_size);
            return Ok(0);
        }

        let idx = (tick - base) as usize;
        if idx >= self.bids.len() {
            // 超出窗口右边界：向右扩展
            self.expand_right(idx + 1)?;
        }
        Ok(idx)
    }

    /// 向右扩展窗口到至少 `need` 档。
    fn expand_right(&mut self, need: usize) -> Result<(), ArrayError> {
        if need > self.config.max_levels {
            return Err(ArrayError::PriceExceedsMaxWindow {
                tick: self.window_base.unwrap_or(0) + need as i64,
                base: self.window_base.unwrap_or(0),
                max_levels: self.config.max_levels,
            });
        }
        let new_size = need.next_power_of_two().min(self.config.max_levels);
        let extend_by = new_size - self.bids.len();
        if extend_by == 0 {
            return Ok(());
        }
        self.bids
            .extend(std::iter::repeat_with(PriceLevel::default).take(extend_by));
        self.asks
            .extend(std::iter::repeat_with(PriceLevel::default).take(extend_by));
        let new_bw = (new_size + 63) / 64;
        self.bid_bitmap.resize(new_bw, 0);
        self.ask_bitmap.resize(new_bw, 0);
        Ok(())
    }

    /// 向左 rebase：新左边界 `new_base`，偏移 `shift` 档。
    fn rebase_left(&mut self, new_base: i64, shift: usize, new_size: usize) {
        let old_size = self.bids.len();
        let mut new_bids: Vec<PriceLevel> =
            (0..new_size).map(|_| PriceLevel::default()).collect();
        let mut new_asks: Vec<PriceLevel> =
            (0..new_size).map(|_| PriceLevel::default()).collect();
        // 拷贝已有档位到新位置（idx + shift）
        for i in 0..old_size {
            if !self.bids[i].is_empty() {
                new_bids[i + shift] = std::mem::take(&mut self.bids[i]);
            }
            if !self.asks[i].is_empty() {
                new_asks[i + shift] = std::mem::take(&mut self.asks[i]);
            }
        }
        // 重建位图
        let new_bw = (new_size + 63) / 64;
        let mut new_bid_bm = vec![0u64; new_bw];
        let mut new_ask_bm = vec![0u64; new_bw];
        for i in 0..new_size {
            if !new_bids[i].is_empty() {
                set_bit(&mut new_bid_bm, i);
            }
            if !new_asks[i].is_empty() {
                set_bit(&mut new_ask_bm, i);
            }
        }
        self.bids = new_bids;
        self.asks = new_asks;
        self.bid_bitmap = new_bid_bm;
        self.ask_bitmap = new_ask_bm;
        self.window_base = Some(new_base);
        // 最优价下标偏移
        self.best_bid_idx = self.best_bid_idx.map(|i| i + shift);
        self.best_ask_idx = self.best_ask_idx.map(|i| i + shift);
    }

    /// 买盘档位引用。
    #[inline]
    pub fn bid_level(&self, tick: i64) -> Option<&PriceLevel> {
        let base = self.window_base?;
        if tick < base {
            return None;
        }
        let idx = (tick - base) as usize;
        if idx >= self.bids.len() {
            return None;
        }
        let level = &self.bids[idx];
        if level.is_empty() { None } else { Some(level) }
    }

    /// 卖盘档位引用。
    #[inline]
    pub fn ask_level(&self, tick: i64) -> Option<&PriceLevel> {
        let base = self.window_base?;
        if tick < base {
            return None;
        }
        let idx = (tick - base) as usize;
        if idx >= self.asks.len() {
            return None;
        }
        let level = &self.asks[idx];
        if level.is_empty() { None } else { Some(level) }
    }

    /// 插入限价单到订单簿，返回订单 ID。
    pub fn insert_limit(&mut self, order: match_core_hp::HpOrder) -> Result<u64, ArrayError> {
        let side = order.side;
        let tick = order.price_tick;
        let lot = order.open_lot;
        let idx = self.ensure_idx(tick)?;

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
        let base = self.window_base.ok_or(ArrayError::OrderNotResting(id))?;
        if tick < base {
            return Err(ArrayError::OrderNotResting(id));
        }
        let idx = (tick - base) as usize;
        if idx >= self.bids.len() {
            return Err(ArrayError::OrderNotResting(id));
        }

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
                            find_first_set(&self.ask_bitmap, idx + 1, self.asks.len());
                    }
                }
            }
        }
        self.store.remove(id);
        Ok(order)
    }

    /// 匹配限价买单（吃卖盘），返回成交数量。
    pub fn match_bid(
        &mut self,
        price_tick: i64,
        qty_lot: i64,
        taker_id: u64,
        taker_client_id: u64,
        events: &mut Vec<match_core_hp::HpEvent>,
    ) -> i64 {
        let mut remaining = qty_lot;
        let base = match self.window_base {
            Some(b) => b,
            None => return 0,
        };
        // 买单吃卖盘：价格上限 = price_tick，超过则不成交
        let max_idx = if price_tick < base {
            return 0;
        } else {
            (price_tick - base) as usize
        };

        while remaining > 0 {
            let cur_idx = match self.best_ask_idx {
                Some(i) if i <= max_idx => i,
                _ => break,
            };

            let maker_id = match self.asks[cur_idx].front_id() {
                Some(id) => id,
                None => {
                    clear_bit(&mut self.ask_bitmap, cur_idx);
                    self.best_ask_idx = find_first_set(&self.ask_bitmap, cur_idx + 1, self.asks.len());
                    continue;
                }
            };

            let (maker_open, maker_client, maker_price) = {
                let order = self.store.get_mut(maker_id).expect("maker in store");
                let open = order.open_lot;
                let client = order.client_id;
                let price = order.price_tick;
                let fill = remaining.min(open);
                order.open_lot = open - fill;
                (open, client, price)
            };

            let fill = remaining.min(maker_open);
            remaining -= fill;
            self.asks[cur_idx].total_lot -= fill;

            let maker_gone = maker_open <= fill;
            if maker_gone {
                self.asks[cur_idx].pop_front();
                self.store.remove(maker_id);
                if self.asks[cur_idx].is_empty() {
                    clear_bit(&mut self.ask_bitmap, cur_idx);
                    self.best_ask_idx =
                        find_first_set(&self.ask_bitmap, cur_idx + 1, self.asks.len());
                }
            }

            events.push(match_core_hp::HpEvent::Fill {
                maker_id,
                taker_id,
                maker_client_id: maker_client,
                taker_client_id,
                price_tick: maker_price,
                qty_lot: fill,
                maker_open_lot: if maker_gone { 0 } else { maker_open - fill },
                taker_open_lot: remaining,
            });
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
        let base = match self.window_base {
            Some(b) => b,
            None => return 0,
        };
        // 卖单吃买盘：价格下限 = price_tick，低于则不成交
        let min_idx = if price_tick < base {
            0
        } else {
            (price_tick - base) as usize
        };

        while remaining > 0 {
            let cur_idx = match self.best_bid_idx {
                Some(i) if i >= min_idx => i,
                _ => break,
            };

            let maker_id = match self.bids[cur_idx].front_id() {
                Some(id) => id,
                None => {
                    clear_bit(&mut self.bid_bitmap, cur_idx);
                    self.best_bid_idx = find_last_set(&self.bid_bitmap, cur_idx.saturating_sub(1));
                    continue;
                }
            };

            let (maker_open, maker_client, maker_price) = {
                let order = self.store.get_mut(maker_id).expect("maker in store");
                let open = order.open_lot;
                let client = order.client_id;
                let price = order.price_tick;
                let fill = remaining.min(open);
                order.open_lot = open - fill;
                (open, client, price)
            };

            let fill = remaining.min(maker_open);
            remaining -= fill;
            self.bids[cur_idx].total_lot -= fill;

            let maker_gone = maker_open <= fill;
            if maker_gone {
                self.bids[cur_idx].pop_front();
                self.store.remove(maker_id);
                if self.bids[cur_idx].is_empty() {
                    clear_bit(&mut self.bid_bitmap, cur_idx);
                    self.best_bid_idx = find_last_set(&self.bid_bitmap, cur_idx.saturating_sub(1));
                }
            }

            events.push(match_core_hp::HpEvent::Fill {
                maker_id,
                taker_id,
                maker_client_id: maker_client,
                taker_client_id,
                price_tick: maker_price,
                qty_lot: fill,
                maker_open_lot: if maker_gone { 0 } else { maker_open - fill },
                taker_open_lot: remaining,
            });
        }
        qty_lot - remaining
    }

    /// 行情快照：前 n 档买卖盘。
    pub fn depth(&self, n: usize) -> (Vec<(i64, i64)>, Vec<(i64, i64)>) {
        let mut bids = Vec::with_capacity(n);
        let mut asks = Vec::with_capacity(n);
        let base = match self.window_base {
            Some(b) => b,
            None => return (bids, asks),
        };

        if let Some(mut idx) = self.best_bid_idx {
            for _ in 0..n {
                let level = &self.bids[idx];
                if !level.is_empty() {
                    bids.push((base + idx as i64, level.total_lot));
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

        if let Some(mut idx) = self.best_ask_idx {
            for _ in 0..n {
                let level = &self.asks[idx];
                if !level.is_empty() {
                    asks.push((base + idx as i64, level.total_lot));
                }
                match find_first_set(&self.ask_bitmap, idx + 1, self.asks.len()) {
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

/// 从 `start` 开始找第一个置位 bit（卖盘向上找档）。
#[inline]
fn find_first_set(bitmap: &[u64], start: usize, max: usize) -> Option<usize> {
    if start >= max {
        return None;
    }
    let word_idx = start >> 6;
    if word_idx >= bitmap.len() {
        return None;
    }
    let bit_offset = start & 63;
    let first_word = bitmap[word_idx] >> bit_offset;
    if first_word != 0 {
        let bit = first_word.trailing_zeros() as usize;
        let idx = (word_idx << 6) + bit_offset + bit;
        if idx < max {
            return Some(idx);
        }
        return None;
    }
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

/// 从 `end` 开始向前找最后一个置位 bit（买盘向下找档）。
#[inline]
fn find_last_set(bitmap: &[u64], end: usize) -> Option<usize> {
    let word_idx = end >> 6;
    if word_idx >= bitmap.len() {
        return None;
    }
    let bit_offset = end & 63;
    let mask = if bit_offset == 63 { u64::MAX } else { (1u64 << (bit_offset + 1)) - 1 };
    let first_word = bitmap[word_idx] & mask;
    if first_word != 0 {
        let bit = 63 - first_word.leading_zeros() as usize;
        return Some((word_idx << 6) + bit);
    }
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
        BookConfig::default()
    }

    fn make_order(side: Side, tick: i64, lot: i64, client_id: u64) -> HpOrder {
        HpOrder::limit(side, tick, lot, client_id)
    }

    #[test]
    fn insert_and_best() {
        let mut book = ArrayBook::new(test_config());
        book.insert_limit(make_order(Side::Buy, 100, 10, 1)).unwrap();
        book.insert_limit(make_order(Side::Buy, 102, 5, 1)).unwrap();
        book.insert_limit(make_order(Side::Sell, 105, 8, 2)).unwrap();

        assert_eq!(book.best_bid_tick(), Some(102));
        assert_eq!(book.best_ask_tick(), Some(105));
    }

    #[test]
    fn dynamic_window_expand() {
        let mut book = ArrayBook::new(BookConfig::new(4, 1024).unwrap());
        // 第一笔设定 window_base=100
        book.insert_limit(make_order(Side::Buy, 100, 10, 1)).unwrap();
        assert_eq!(book.window_base(), Some(100));
        assert_eq!(book.window_size(), 4);
        // 超出窗口，扩展
        book.insert_limit(make_order(Side::Buy, 110, 5, 1)).unwrap();
        assert!(book.window_size() >= 11);
        assert_eq!(book.best_bid_tick(), Some(110));
    }

    #[test]
    fn dynamic_window_rebase_left() {
        let mut book = ArrayBook::new(BookConfig::new(16, 1024).unwrap());
        book.insert_limit(make_order(Side::Buy, 100, 10, 1)).unwrap();
        book.insert_limit(make_order(Side::Sell, 105, 8, 2)).unwrap();
        // 低于 window_base，触发 rebase
        book.insert_limit(make_order(Side::Buy, 95, 3, 1)).unwrap();
        assert_eq!(book.window_base(), Some(95));
        assert_eq!(book.best_bid_tick(), Some(100)); // 100 > 95
        assert_eq!(book.best_ask_tick(), Some(105));
    }

    #[test]
    fn match_bid_fills_across_levels() {
        let mut book = ArrayBook::new(test_config());
        book.insert_limit(make_order(Side::Sell, 100, 5, 1)).unwrap();
        book.insert_limit(make_order(Side::Sell, 101, 5, 2)).unwrap();
        book.insert_limit(make_order(Side::Sell, 102, 5, 3)).unwrap();

        let mut events = Vec::new();
        let filled = book.match_bid(102, 12, 999, 99, &mut events);
        assert_eq!(filled, 12);
        assert_eq!(events.len(), 3);
        // 剩余卖盘：102 档还剩 5-2=3
        assert_eq!(book.best_ask_tick(), Some(102));
    }

    #[test]
    fn cancel_updates_best() {
        let mut book = ArrayBook::new(test_config());
        let id1 = book.insert_limit(make_order(Side::Buy, 100, 10, 1)).unwrap();
        let _id2 = book.insert_limit(make_order(Side::Buy, 98, 5, 1)).unwrap();
        assert_eq!(book.best_bid_tick(), Some(100));

        book.cancel_order(id1).unwrap();
        assert_eq!(book.best_bid_tick(), Some(98));
    }

    #[test]
    fn price_out_of_range() {
        let config = BookConfig::new(4, 8).unwrap();
        let mut book = ArrayBook::new(config);
        book.insert_limit(make_order(Side::Buy, 100, 10, 1)).unwrap();
        // 超出 max_levels
        let result = book.insert_limit(make_order(Side::Buy, 200, 10, 1));
        assert!(result.is_err());
    }

    #[test]
    fn depth_snapshot() {
        let mut book = ArrayBook::new(test_config());
        book.insert_limit(make_order(Side::Buy, 100, 10, 1)).unwrap();
        book.insert_limit(make_order(Side::Buy, 99, 5, 1)).unwrap();
        book.insert_limit(make_order(Side::Sell, 101, 8, 2)).unwrap();

        let (bids, asks) = book.depth(5);
        assert_eq!(bids.len(), 2);
        assert_eq!(bids[0], (100, 10));
        assert_eq!(bids[1], (99, 5));
        assert_eq!(asks.len(), 1);
        assert_eq!(asks[0], (101, 8));
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
