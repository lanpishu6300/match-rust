//! 单个价格档位：聚合数量 + FIFO 订单 ID 队列。
//!
//! 布局紧凑（32 字节），适合大规模数组分配。
//! 订单 ID 存在 `OrderStore` 中，这里只存 ID 和聚合数量。

use std::collections::VecDeque;

/// 一个价格档位的状态。
#[derive(Debug, Default)]
pub struct PriceLevel {
    /// 该档位总挂单量（lot）。
    pub total_lot: i64,
    /// FIFO 订单 ID 队列（OrderStore 的 1-based slot id）。
    pub ids: VecDeque<u64>,
}

impl PriceLevel {
    /// 追加订单到队尾。
    #[inline]
    pub fn push(&mut self, id: u64, lot: i64) {
        self.ids.push_back(id);
        self.total_lot += lot;
    }

    /// 队首订单 ID（不弹出）。
    #[inline]
    pub fn front_id(&self) -> Option<u64> {
        self.ids.front().copied()
    }

    /// 弹出队首订单 ID。
    #[inline]
    pub fn pop_front(&mut self) -> Option<u64> {
        self.ids.pop_front()
    }

    /// 从队列中移除指定订单 ID（O(k)，k 为该档订单数）。
    /// 返回是否找到并移除。
    pub fn remove_id(&mut self, id: u64) -> bool {
        if let Some(pos) = self.ids.iter().position(|&x| x == id) {
            self.ids.remove(pos);
            true
        } else {
            false
        }
    }

    /// 该档订单数。
    #[inline]
    pub fn order_count(&self) -> usize {
        self.ids.len()
    }

    /// 档位是否为空。
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    /// 清空档位。
    #[inline]
    pub fn clear(&mut self) {
        self.total_lot = 0;
        self.ids.clear();
    }
}
