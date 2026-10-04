//! # match-core-array — 数组+价格偏移撮合引擎
//!
//! 核心设计：价格直接映射为数组下标（O(1) 直接寻址），非空档位位图 + CTZ 硬件指令快速跳档，
//! 连续内存布局使 CPU 硬件预取器自动加载后续档位，遍历效率比指针跳转的树结构高 10-100 倍。
//!
//! 与 match-core（BTreeSet）和 match-core-hp（BTreeMap/ART）的对比：
//! - 插入/撤销：O(1) 直接寻址 vs O(log n) 树遍历
//! - 匹配遍历：顺序数组访问（硬件预取）vs 指针跳转（缓存未命中）
//! - 最优价：整数指针读取 vs 树最左/最右节点
//! - 跳档：CTZ 单指令 vs 树迭代器 next
//!
//! 价格范围：通过 `tick_lo` / `tick_hi` 配置，数组大小 = tick_hi - tick_lo + 1。
//! 超出范围的订单被拒绝（返回错误），适用于价格范围可控的交易对。

pub mod book;
pub mod engine;
pub mod level;
pub mod types;

pub use book::ArrayBook;
pub use engine::ArrayEngine;
pub use level::PriceLevel;
pub use types::{ArrayError, BookConfig};

// 复用 hp-engine 的核心类型（零成本 re-export）
pub use match_core_hp::{HpCommand, HpEvent, HpOrder, OrderStore, Side, SymbolScale};
