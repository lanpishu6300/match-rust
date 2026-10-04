//! 引擎配置与错误类型。

/// 订单簿价格范围配置。
///
/// 数组大小 = `(tick_hi - tick_lo + 1)` 个档位，每档一个 [`crate::PriceLevel`]。
/// 价格超出范围的订单会被拒绝。
#[derive(Debug, Clone, Copy)]
pub struct BookConfig {
    /// 最低价格 tick（含）。
    pub tick_lo: i64,
    /// 最高价格 tick（含）。
    pub tick_hi: i64,
}

impl BookConfig {
    /// 创建配置，校验范围合法性。
    pub fn new(tick_lo: i64, tick_hi: i64) -> Result<Self, ArrayError> {
        if tick_lo > tick_hi {
            return Err(ArrayError::InvalidRange { tick_lo, tick_hi });
        }
        let span = (tick_hi - tick_lo) as u64;
        if span > 1 << 31 {
            return Err(ArrayError::RangeTooLarge { span });
        }
        Ok(Self { tick_lo, tick_hi })
    }

    /// 档位总数。
    #[inline]
    pub fn level_count(&self) -> usize {
        (self.tick_hi - self.tick_lo + 1) as usize
    }

    /// tick → 数组下标，越界返回 None。
    #[inline]
    pub fn tick_to_idx(&self, tick: i64) -> Option<usize> {
        if tick < self.tick_lo || tick > self.tick_hi {
            None
        } else {
            Some((tick - self.tick_lo) as usize)
        }
    }

    /// 数组下标 → tick。
    #[inline]
    pub fn idx_to_tick(&self, idx: usize) -> i64 {
        self.tick_lo + idx as i64
    }

    /// 位图 u64 字数。
    #[inline]
    pub fn bitmap_words(&self) -> usize {
        (self.level_count() + 63) / 64
    }
}

/// 数组撮合引擎错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArrayError {
    #[error("invalid price range: tick_lo={tick_lo} > tick_hi={tick_hi}")]
    InvalidRange { tick_lo: i64, tick_hi: i64 },

    #[error("price range too large: span={span} (max 2^31)")]
    RangeTooLarge { span: u64 },

    #[error("price tick {tick} out of range [{lo}, {hi}]")]
    PriceOutOfRange { tick: i64, lo: i64, hi: i64 },

    #[error("order id {0} not found")]
    OrderNotFound(u64),

    #[error("cancel order id {0} is already filled/revoked")]
    OrderNotResting(u64),
}
