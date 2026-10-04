//! 引擎配置与错误类型。

/// 订单簿价格窗口配置（动态滑动窗口）。
///
/// 不再预分配全价格范围，而是从 `initial_levels` 开始，按需扩展到 `max_levels`。
/// 窗口左边界 `window_base` 随第一笔订单价格动态设定，后续订单超出窗口时向右扩展。
/// 这样实际内存占用 = 活跃价格跨度 × 单档大小，而不是全价格范围。
#[derive(Debug, Clone, Copy)]
pub struct BookConfig {
    /// 初始预分配档位数量。
    pub initial_levels: usize,
    /// 最大档位数量（硬上限，防极端价格导致内存爆炸）。
    pub max_levels: usize,
}

impl Default for BookConfig {
    fn default() -> Self {
        Self {
            initial_levels: 512,   // 初始 512 档（~16KB，L1 缓存内）
            max_levels: 65_536,    // 最大 65536 档（~2MB，L2/L3 内）
        }
    }
}

impl BookConfig {
    /// 创建配置，校验合法性。
    pub fn new(initial_levels: usize, max_levels: usize) -> Result<Self, ArrayError> {
        if initial_levels == 0 {
            return Err(ArrayError::InvalidConfig("initial_levels must be > 0"));
        }
        if max_levels < initial_levels {
            return Err(ArrayError::InvalidConfig("max_levels must be >= initial_levels"));
        }
        if max_levels > 1 << 24 {
            return Err(ArrayError::InvalidConfig("max_levels too large (>16M)"));
        }
        Ok(Self { initial_levels, max_levels })
    }

    /// 兼容旧 API：固定范围配置（内部转为初始窗口 = 范围大小）。
    pub fn fixed(tick_lo: i64, tick_hi: i64) -> Result<Self, ArrayError> {
        if tick_lo > tick_hi {
            return Err(ArrayError::InvalidRange { tick_lo, tick_hi });
        }
        let levels = (tick_hi - tick_lo + 1) as usize;
        Self::new(levels.next_power_of_two(), levels.max(4096))
    }
}

/// 数组撮合引擎错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ArrayError {
    #[error("invalid price range: tick_lo={tick_lo} > tick_hi={tick_hi}")]
    InvalidRange { tick_lo: i64, tick_hi: i64 },

    #[error("invalid config: {0}")]
    InvalidConfig(&'static str),

    #[error("price tick {tick} exceeds max window (base={base}, max_levels={max_levels})")]
    PriceExceedsMaxWindow { tick: i64, base: i64, max_levels: usize },

    #[error("order id {0} not found")]
    OrderNotFound(u64),

    #[error("cancel order id {0} is already filled/revoked")]
    OrderNotResting(u64),
}
