//! MatchFsm —— 把 match-core-hp 撮合引擎接入 multiraft 的可插拔状态机（multiraft 二期）。
//!
//! 设计：
//! - **日志条目 = 序列化撮合命令**（Limit/Cancel/Market 定长编码，含客户端幂等键 idem）。
//! - **apply = 确定性重放**：`engine.on_order(cmd)`；同一 (group, idem) 只执行一次（幂等集合），
//!   防止 propose 超时重试/leader 重放造成重复撮合。
//! - **ApplyOut.effects = 序列化撮合事件**（Fill/Rest/Revoke），propose 调用方解码即得成交回报。
//! - **快照（一期简化）**：累积命令字节 + 幂等集合（bincode）——restore 时新建引擎全量重放。
//!   诚实边界：空间 ∝ 日志量；生产需真状态快照（book/order-store 导出），见 docs/raft-replication.md。
//!
//! 一致性模型：multiraft `propose` Ok = quorum 提交 + 本节点 apply（linearizable 写）；
//! 多节点对同一命令序列确定性重放 → 各节点撮合簿状态逐命令一致（demo 验证）。

use match_core_hp::{HpCommand, HpEngine, HpEvent, Side};
use multiraft_fsm::{ApplyOut, GroupId, StateMachine};
use rustc_hash::FxHashMap;
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MatchFsmError {
    #[error("decode command: {0}")]
    Decode(String),
    #[error("bincode: {0}")]
    Bincode(String),
}

/// 每 group 的撮合状态。
#[derive(Default)]
struct MatchState {
    engine: HpEngine,
    /// 累积命令字节（一期快照：全量重放重建）。
    cmds: Vec<u8>,
    /// 已处理幂等键。
    idem: HashSet<u64>,
    /// 统计（一致性核对用）。
    fills: u64,
    revokes: u64,
}

impl MatchState {
    fn replay_all(&mut self) {
        let mut off = 0usize;
        let cmds = std::mem::take(&mut self.cmds);
        // 每条命令定长/变长？——统一变长：len u32 前缀 + payload（命令编码自身是变长）
        while off + 4 <= cmds.len() {
            let len = u32::from_le_bytes(cmds[off..off + 4].try_into().unwrap()) as usize;
            if off + 4 + len > cmds.len() {
                break;
            }
            let (idem, cmd) = decode_command(&cmds[off + 4..off + 4 + len]);
            self.idem.insert(idem);
            let evs = self.engine.on_order(cmd);
            self.fills += evs.iter().filter(|e| matches!(e, HpEvent::Fill { .. })).count() as u64;
            self.revokes += evs.iter().filter(|e| matches!(e, HpEvent::Revoke { .. })).count() as u64;
            off += 4 + len;
        }
        self.cmds = cmds;
    }
}

/// multiraft 可插拔撮合状态机。
pub struct MatchFsm {
    groups: FxHashMap<GroupId, MatchState>,
}

impl Default for MatchFsm {
    fn default() -> Self {
        Self::new()
    }
}

impl MatchFsm {
    pub fn new() -> Self {
        Self { groups: FxHashMap::default() }
    }

    /// 一致性核对摘要。
    pub fn summary(&self, group: GroupId) -> Option<MatchSummary> {
        let s = self.groups.get(&group)?;
        Some(MatchSummary {
            applied_orders: s.idem.len() as u64,
            fills: s.fills,
            revokes: s.revokes,
            best_bid: s.engine.book.best_bid(),
            best_ask: s.engine.book.best_ask(),
            live_orders: s.engine.book.store().live_len() as u64,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchSummary {
    pub applied_orders: u64,
    pub fills: u64,
    pub revokes: u64,
    pub best_bid: Option<i64>,
    pub best_ask: Option<i64>,
    pub live_orders: u64,
}

/// 命令编码（定长，含幂等键 idem）。
/// Limit:  [0][side u8][price i64][qty i64][ts u64][client u64][idem u64] = 42B
/// Cancel: [1][id u64][client u64][idem u64] = 25B
/// Market: [2][side u8][qty i64][ts u64][client u64][idem u64] = 34B
pub fn encode_command(cmd: &HpCommand, idem: u64) -> Vec<u8> {
    match *cmd {
        HpCommand::Limit { side, price_tick, qty_lot, ts, client_id } => {
            let mut b = Vec::with_capacity(42);
            b.push(0);
            b.push(if side == Side::Buy { 0 } else { 1 });
            b.extend_from_slice(&price_tick.to_le_bytes());
            b.extend_from_slice(&qty_lot.to_le_bytes());
            b.extend_from_slice(&ts.to_le_bytes());
            b.extend_from_slice(&client_id.to_le_bytes());
            b.extend_from_slice(&idem.to_le_bytes());
            b
        }
        HpCommand::Cancel { id } => {
            let mut b = Vec::with_capacity(25);
            b.push(1);
            b.extend_from_slice(&id.to_le_bytes());
            b.extend_from_slice(&idem.to_le_bytes());
            b
        }
        HpCommand::Market { side, qty_lot, ts, max_fills, client_id } => {
            let mut b = Vec::with_capacity(34);
            b.push(2);
            b.push(if side == Side::Buy { 0 } else { 1 });
            b.extend_from_slice(&qty_lot.to_le_bytes());
            b.extend_from_slice(&ts.to_le_bytes());
            b.extend_from_slice(&max_fills.unwrap_or(0).to_le_bytes());
            b.extend_from_slice(&client_id.to_le_bytes());
            b.extend_from_slice(&idem.to_le_bytes());
            b
        }
    }
}

pub fn decode_command(p: &[u8]) -> (u64, HpCommand) {
    debug_assert!(!p.is_empty());
    let tag = p[0];
    match tag {
        0 => {
            let side = if p[1] == 0 { Side::Buy } else { Side::Sell };
            let price = i64::from_le_bytes(p[2..10].try_into().unwrap());
            let qty = i64::from_le_bytes(p[10..18].try_into().unwrap());
            let ts = u64::from_le_bytes(p[18..26].try_into().unwrap());
            let client = u64::from_le_bytes(p[26..34].try_into().unwrap());
            let idem = u64::from_le_bytes(p[34..42].try_into().unwrap());
            (idem, HpCommand::Limit { side, price_tick: price, qty_lot: qty, ts, client_id: client })
        }
        1 => {
            let id = u64::from_le_bytes(p[1..9].try_into().unwrap());
            let idem = u64::from_le_bytes(p[9..17].try_into().unwrap());
            (idem, HpCommand::Cancel { id })
        }
        _ => {
            let side = if p[1] == 0 { Side::Buy } else { Side::Sell };
            let qty = i64::from_le_bytes(p[2..10].try_into().unwrap());
            let ts = u64::from_le_bytes(p[10..18].try_into().unwrap());
            let max_fills = u32::from_le_bytes(p[18..22].try_into().unwrap());
            let client = u64::from_le_bytes(p[22..30].try_into().unwrap());
            let idem = u64::from_le_bytes(p[30..38].try_into().unwrap());
            let max_fills = if max_fills == 0 { None } else { Some(max_fills) };
            (idem, HpCommand::Market { side, qty_lot: qty, ts, max_fills, client_id: client })
        }
    }
}

/// 事件编码：Fill/Rest/Revoke（定长拼接，propose 回报用）。
/// Fill:   [0][maker_id u64][taker_id u64][maker_client u64][taker_client u64][price i64][qty i64][maker_open i64][taker_open i64]
/// Rest:   [1][id u64][client u64][side u8][price i64][qty i64]
/// Revoke: [2][id u64][client u64][reason u32]
pub fn encode_events(evs: &[HpEvent]) -> Vec<u8> {
    let mut out = Vec::with_capacity(evs.len() * 65);
    for e in evs {
        match *e {
            HpEvent::Fill { maker_id, taker_id, maker_client_id, taker_client_id, price_tick, qty_lot, maker_open_lot, taker_open_lot } => {
                out.push(0);
                out.extend_from_slice(&maker_id.to_le_bytes());
                out.extend_from_slice(&taker_id.to_le_bytes());
                out.extend_from_slice(&maker_client_id.to_le_bytes());
                out.extend_from_slice(&taker_client_id.to_le_bytes());
                out.extend_from_slice(&price_tick.to_le_bytes());
                out.extend_from_slice(&qty_lot.to_le_bytes());
                out.extend_from_slice(&maker_open_lot.to_le_bytes());
                out.extend_from_slice(&taker_open_lot.to_le_bytes());
            }
            HpEvent::Rest { id, client_id, side, price_tick, qty_lot } => {
                out.push(1);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&client_id.to_le_bytes());
                out.push(if side == Side::Buy { 0 } else { 1 });
                out.extend_from_slice(&price_tick.to_le_bytes());
                out.extend_from_slice(&qty_lot.to_le_bytes());
            }
            HpEvent::Revoke { id, client_id, reason } => {
                out.push(2);
                out.extend_from_slice(&id.to_le_bytes());
                out.extend_from_slice(&client_id.to_le_bytes());
                out.extend_from_slice(&reason.to_le_bytes());
            }
        }
    }
    out
}

impl StateMachine for MatchFsm {
    type Error = MatchFsmError;

    fn apply(&mut self, group: GroupId, _index: u64, data: &[u8]) -> Result<ApplyOut, Self::Error> {
        let st = self.groups.entry(group).or_default();
        let (idem, cmd) = decode_command(data);
        if !st.idem.insert(idem) {
            return Ok(ApplyOut::default()); // 幂等命中（重试/重放）
        }
        // 累积命令（快照重放数据）——len u32 前缀
        st.cmds.extend_from_slice(&(data.len() as u32).to_le_bytes());
        st.cmds.extend_from_slice(data);
        let evs = st.engine.on_order(cmd);
        st.fills += evs.iter().filter(|e| matches!(e, HpEvent::Fill { .. })).count() as u64;
        st.revokes += evs.iter().filter(|e| matches!(e, HpEvent::Revoke { .. })).count() as u64;
        Ok(ApplyOut { effects: encode_events(evs) })
    }

    fn snapshot(&self, group: GroupId) -> Result<Vec<u8>, Self::Error> {
        let st = self.groups.get(&group).ok_or_else(|| {
            MatchFsmError::Decode(format!("no state for group {group}"))
        })?;
        bincode::serialize(&(st.cmds.clone(), st.idem.clone()))
            .map_err(|e| MatchFsmError::Bincode(e.to_string()))
    }

    fn restore(&mut self, group: GroupId, snapshot: &[u8]) -> Result<(), Self::Error> {
        let (cmds, idem): (Vec<u8>, HashSet<u64>) =
            bincode::deserialize(snapshot).map_err(|e| MatchFsmError::Bincode(e.to_string()))?;
        let mut st = MatchState {
            engine: HpEngine::new(),
            cmds,
            idem,
            fills: 0,
            revokes: 0,
        };
        st.replay_all();
        self.groups.insert(group, st);
        Ok(())
    }
}
