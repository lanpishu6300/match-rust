# 数组+价格偏移撮合引擎设计文档（match-core-array）

> 版本：v1.0 | 日期：2026-10-04 | 状态：已实现，基准验证中
>
> 参考：《高性能撮合引擎架构设计与面试指南》§2.1 订单簿数据结构选型

---

## 1. 设计目标

在 match-rust 已有的两个撮合引擎（`match-core` BTreeSet、`match-core-hp` BTreeMap/ART）之外，实现第三个引擎版本 `match-core-array`，采用**数组+价格偏移（Price Offset Array）**方案，验证 O(1) 直接寻址 + 位图+CTZ 硬件指令 + 连续内存硬件预取带来的性能收益。

### 1.1 与现有引擎的定位差异

| 引擎 | 价格索引 | 订单存储 | 复杂度 | 适用场景 |
|------|---------|---------|--------|---------|
| `match-core` | `BTreeSet<BuyEntry/SellEntry>` | 内嵌在树节点 | O(log n) 全字段比较 | 原型验证、低吞吐 |
| `match-core-hp` | `BTreeMap<i64, Level>` / ART 基数树 | `OrderStore` 分离（slab） | O(log n) 树遍历 | 生产默认，17-29M ops/s |
| `match-core-array` | **`Vec<PriceLevel>` 直接寻址** | `OrderStore` 复用 | **O(1) 下标计算** | 价格范围可控、极致低延迟 |

---

## 2. 核心原理

### 2.1 直接寻址（O(1) 的本质）

价格不是任意值，而是**离散的、有界的整数刻度**。给定交易对的价格范围 `[tick_lo, tick_hi]` 和最小变动单位 tick，价格本身就是天然的数组下标：

```
idx = tick - tick_lo
```

一次减法直接定位档位，不需要哈希、不需要比较、不需要树遍历。这比任何基于比较的数据结构（红黑树 O(log n) 次比较）或哈希表（O(1) 但有哈希计算+冲突处理）都快。

### 2.2 非空档位位图 + CTZ 硬件指令

价格范围可能有 100 万档，但实际有挂单的只有几百档。逐个检查数组元素是否为空会浪费大量时间。解决方案：

- **位图**：`Vec<u64>`，每 bit 代表一个档位是否有挂单
- **CTZ（Count Trailing Zeros）**：x86 上 `TZCNT`/`BSF` 单指令，1 时钟周期找到 64 位字中第一个置位 bit
- **CLZ（Count Leading Zeros）**：`LZCNT`/`BSR` 单指令，找到最后一个置位 bit

```rust
// 找下一个非空卖档（从 start 向上）
fn find_first_set(bitmap: &[u64], start: usize, max: usize) -> Option<usize> {
    let word_idx = start >> 6;
    let bit_offset = start & 63;
    let first_word = bitmap[word_idx] >> bit_offset;
    if first_word != 0 {
        let bit = first_word.trailing_zeros() as usize;  // CTZ 硬件指令
        return Some((word_idx << 6) + bit_offset + bit);
    }
    for i in (word_idx + 1)..bitmap.len() {
        if bitmap[i] != 0 {
            let bit = bitmap[i].trailing_zeros() as usize;
            return Some((i << 6) + bit);
        }
    }
    None
}
```

### 2.3 最优价指针

维护 `best_bid_idx: Option<usize>` 和 `best_ask_idx: Option<usize>` 两个整数：

- 插入时：如果新价格更优，更新指针（O(1) 比较）
- 撤销时：如果撤销的是最优档且该档清空，用 CTZ/CLZ 找下一个非空档
- 读取最优价：纯整数读取，O(1)

### 2.4 连续内存 + 硬件预取

数组是连续内存，匹配时从最优档开始向差的价格方向遍历，访问模式是**顺序的、可预测的**。CPU 硬件预取器检测到顺序访问后，自动提前加载后续缓存行。每 64 字节（一个缓存行）只触发一次缓存未命中。

---

## 3. 数据结构设计

### 3.1 PriceLevel（价格档位）

```rust
pub struct PriceLevel {
    pub total_lot: i64,           // 该档总挂单量
    pub ids: VecDeque<u64>,       // FIFO 订单 ID 队列（指向 OrderStore）
}
```

- 同价位订单按时间优先（FIFO），用 `VecDeque` 维护
- 订单实际数据存在 `OrderStore`（复用 hp-engine 的 slab 分配器），档位只存 ID

### 3.2 ArrayBook（订单簿）

```rust
pub struct ArrayBook {
    config: BookConfig,              // tick_lo / tick_hi
    bids: Vec<PriceLevel>,           // 买盘数组（下标 = tick - tick_lo）
    asks: Vec<PriceLevel>,           // 卖盘数组
    bid_bitmap: Vec<u64>,            // 买盘非空档位位图
    ask_bitmap: Vec<u64>,            // 卖盘非空档位位图
    best_bid_idx: Option<usize>,     // 最优买价下标
    best_ask_idx: Option<usize>,     // 最优卖价下标
    store: OrderStore,               // 订单存储（slab + free list）
}
```

### 3.3 BookConfig（价格范围配置）

```rust
pub struct BookConfig {
    pub tick_lo: i64,   // 最低价格 tick（含）
    pub tick_hi: i64,   // 最高价格 tick（含）
}
```

- 数组大小 = `tick_hi - tick_lo + 1`
- 位图大小 = `(数组大小 + 63) / 64` 个 u64
- 超出范围的订单被拒绝（返回 `PriceOutOfRange` 错误）

**内存估算**：以 tick 范围 0..=1,000,000（100 万档）为例：
- 档位数组：1,000,000 × ~32B（PriceLevel 空时）= ~32MB × 2（买卖）= 64MB
- 位图：1,000,000 / 8 = 125KB × 2 = 250KB
- 总计约 65MB，可接受

**实际内存更小**：`VecDeque::new()` 不分配堆，空 `PriceLevel` 只有 `total_lot`（8B）+ `VecDeque` 的 3 个指针（24B）= 32B。活跃档位的 `VecDeque` 才分配堆。

### 3.4 ArrayEngine（引擎层）

封装 `ArrayBook`，提供与 `HpEngine` 对齐的 `HpCommand` 接口：

- `Limit`：先吃对手盘，剩余挂单
- `Cancel`：按订单 ID 撤销
- `Market`：用极端价格吃对手盘，未成交部分自动撤销

维护 `client_to_ids: FxHashMap<u64, Vec<u64>>`，支持同一 client 多在途订单。

---

## 4. 核心操作复杂度

| 操作 | match-core (BTreeSet) | match-core-hp (BTreeMap) | match-core-array |
|------|----------------------|------------------------|-----------------|
| 插入挂单 | O(log n) 全字段比较 | O(log n) 树遍历 + Level 查找 | **O(1) 下标计算 + VecDeque push** |
| 撤销订单 | O(n) find + O(log n) remove | O(log n) 树查找 + Level remove | **O(1) 下标 + O(k) 同档移除** |
| 最优价读取 | O(log n) 树最左/右 | O(1) 缓存指针 | **O(1) 整数读取** |
| 匹配遍历 | O(log n) 迭代器跳转 | O(log n) 迭代器 + Level 连续 | **O(1) 下标 + CTZ 跳档 + 顺序预取** |
| 行情快照 | O(log n + k) | O(log n + k) | **O(k) CTZ 跳档** |

---

## 5. 基准测试结果

### 5.1 测试环境

- Cloud VM Linux，criterion 0.5，sample_size=15，warm-up=2s
- N=50,000 订单/轮
- 三个引擎对比：`core`（BTreeSet）、`hp`（BTreeMap）、`array`（数组+偏移）

### 5.2 结果（时间越低越好）

| 场景 | core | hp | array | array vs hp |
|------|------|-----|-------|-------------|
| rest_only（纯挂单） | 9.35 s | 5.27 ms | **5.09 ms** | **快 3.4%** |
| cross_full（完全成交） | 2.97 s | 3.09 ms | 45.2 ms | 慢 14.6x |
| partial_walk（部分成交） | 6.89 s | 5.61 ms | **5.69 ms** | 慢 1.4% |
| cancel_hot（热点撤单） | — | — | **3.83 ms** | — |
| mixed（混合） | — | — | 24.4 ms | — |

### 5.3 结果分析

**array 优势场景**：
- **rest_only（纯挂单）**：array 5.09ms vs hp 5.27ms，**快 3.4%**。插入是 O(1) 直接寻址，比 BTreeMap 的 O(log n) 树遍历快。
- **partial_walk（部分成交）**：array 5.69ms vs hp 5.61ms，基本持平。

**array 劣势场景**：
- **cross_full（完全成交）**：array 45.2ms vs hp 3.09ms，**慢 14.6 倍**。原因：
  1. 当前 `match_bid`/`match_ask` 实现中，每次 fill 都 `self.store.get().cloned()` 克隆整个 `HpOrder`，再 `self.store.get_mut()` 二次查找
  2. 每次档位清空后调用 `find_first_set` 扫描位图，而 hp-engine 的 BTreeMap 迭代器有缓存的 next 指针
  3. `VecDeque` 的 `pop_front` 有内存移动开销，hp-engine 的 Level 用 `VecDeque` 但有对象池优化

### 5.4 优化方向（后续工作）

1. **消除 match 循环中的订单克隆**：直接在 `OrderStore` 上 `get_mut`，不 clone
2. **内联 CTZ 跳档**：当前档还有订单时不调用 `find_first_set`，只有档清空时才跳
3. **Level 用数组+head/tail 指针替代 VecDeque**：避免 VecDeque 的环形缓冲区开销
4. **预取下一档**：匹配当前档时，`std::intrinsics::prefetch` 下一档地址
5. **事件缓冲区复用**：当前 `events.clear()` 后容量保留，已优化

---

## 6. 价格范围处理策略

### 6.1 固定范围（当前实现）

启动时配置 `tick_lo`/`tick_hi`，数组预分配。超出范围的订单被拒绝。

**适用**：股票、期货等价格范围可控的产品。

**不适用**：加密货币（价格范围 0.00001~1000000，tick 太小时数组过大）。

### 6.2 滑动窗口（后续优化）

维护 `base_tick` + 固定窗口大小（如 65536 档），tick 超出窗口时 rebase（拷贝活跃档位到新窗口）。rebase 是罕见操作（价格波动超过窗口范围才触发）。

### 6.3 价格分段（后续优化）

将价格范围分成多个段，每段用一个数组，段之间用 BTreeMap 组织。兼顾大价格范围和 O(1) 访问。

---

## 7. 容错设计

### 7.1 catch_unwind 集成

`ArrayEngine` 设计为可在 `symbol_worker` 中被 `catch_unwind` 包裹：

```rust
match std::panic::catch_unwind(AssertUnwindSafe(|| {
    engine.on_order(cmd)
})) {
    Ok(Ok(events)) => handle_events(events),
    Ok(Err(e)) => reject_order(e),
    Err(_) => log_engine_panic(),
}
```

### 7.2 panic 后状态

- `ArrayBook` 的数组/位图在 panic 后可能处于半更新状态
- 建议：catch 到 panic 后标记 engine 受损，拒绝后续订单，触发重启+journal 恢复
- 与 hp-engine 相同的容错策略

---

## 8. 与 hp-engine 的代码复用

`match-core-array` 直接依赖 `match-core-hp`，复用以下组件（零成本 re-export）：

- `HpOrder`、`HpEvent`、`HpCommand`：命令/事件类型
- `OrderStore`：slab 分配器 + free list
- `Side`、`SymbolScale`：基础类型

只重写了 `Book`（价格索引）和 `Engine`（命令调度），代码量约 600 行（含测试）。

---

## 9. 文件结构

```
crates/match-core-array/
├── Cargo.toml
├── src/
│   ├── lib.rs          # re-export + 文档
│   ├── types.rs        # BookConfig, ArrayError
│   ├── level.rs        # PriceLevel
│   ├── book.rs         # ArrayBook（核心：数组+位图+CTZ）
│   └── engine.rs       # ArrayEngine（HpCommand 接口）
└── tests/              # 集成测试（待补充）
```

---

## 10. 测试覆盖

- 12 个单元测试全部通过
- 覆盖：插入/最优价、跨档成交、撤销更新最优价、价格越界、行情快照、位图 CTZ/CLZ、多在途订单、市价单

---

## 11. 结论与建议

### 11.1 当前状态

- ✅ 功能完整：Limit/Cancel/Market 命令，多在途订单，行情快照
- ✅ 测试通过：12 个单元测试
- ✅ 基准可运行：5 个场景对比
- ⚠️ cross_full 场景性能不如 hp-engine（match 循环需优化）
- ⚠️ 价格范围固定，不适合加密货币大价格范围

### 11.2 生产建议

1. **当前不建议替换 hp-engine 作为生产默认**：cross_full 场景慢 14x，需要优化 match 循环
2. **rest_only 场景有优势**：纯挂单场景快 3.4%，适合做市商大量挂单的 workload
3. **优先优化 match 循环**：消除 clone、内联 CTZ、预取下一档，预期 cross_full 可追平 hp-engine
4. **滑动窗口支持**：解决加密货币大价格范围问题

### 11.3 与参考文档的对照

参考文档 §2.1 预测"数组方案延迟比红黑树低两个数量级"。在 match-rust 的实测中：
- 对比 `match-core`（BTreeSet）：array 快 **1800 倍**（rest_only：5ms vs 9.35s），符合预期
- 对比 `match-core-hp`（BTreeMap+OrderStore 分离）：array 基本持平或略优，因为 hp-engine 已经做了大量优化（slab 分配、Level 分离、BTreeMap 缓存最优价）

**关键洞察**：数组方案的压倒性优势是相对于"每个节点完整订单+全字段比较"的朴素 BTreeSet 实现。当引擎已经做了订单存储分离（hp-engine），数组方案的边际收益缩小，但在插入/撤销场景仍有 O(1) vs O(log n) 的理论优势。

---

## 附录 A：位图操作汇编级解析

### CTZ（Count Trailing Zeros）

```rust
let bit = first_word.trailing_zeros() as usize;
```

x86_64 编译为：
```asm
tzcnt rax, rdi      # 1 uop, 1 cycle latency, 0.5 throughput
```

或在不支持 BMI1 的 CPU 上回退为：
```asm
bsf rax, rdi        # 1 uop, 3 cycle latency
```

### CLZ（Count Leading Zeros）

```rust
let bit = 63 - word.leading_zeros() as usize;
```

x86_64 编译为：
```asm
lzcnt rax, rdi      # 1 uop, 1 cycle latency
```

或回退为：
```asm
bsr rax, rdi        # 1 uop, 3 cycle latency
```

### 位图 set/clear

```rust
bitmap[idx >> 6] |= 1u64 << (idx & 63);   // bts 指令
bitmap[idx >> 6] &= !(1u64 << (idx & 63)); // btr 指令
```

都是单指令，1 周期。

---

## 附录 B：内存布局图

```
ArrayBook 内存布局（买盘侧，卖盘侧对称）：

bids: Vec<PriceLevel>
┌─────────────────────────────────────────────────────────┐
│ idx 0 (tick=tick_lo)   │ PriceLevel { total_lot, ids } │
│ idx 1                  │ PriceLevel { total_lot, ids } │
│ idx 2                  │ PriceLevel { total_lot, ids } │  ← 连续内存，硬件预取
│ ...                    │ ...                           │
│ idx N (tick=tick_hi)   │ PriceLevel { total_lot, ids } │
└─────────────────────────────────────────────────────────┘

bid_bitmap: Vec<u64>
┌──────────┬──────────┬──────────┐
│ word 0   │ word 1   │ word 2   │  每 bit 对应一个档位
│ 64 bits  │ 64 bits  │ 64 bits  │  1=非空, 0=空
└──────────┴──────────┴──────────┘
     ↑
     CTZ 从这里找第一个 1 bit

best_bid_idx: Option<usize>  → 直接指向最优买价档位
```
