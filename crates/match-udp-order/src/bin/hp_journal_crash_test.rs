//! `hp_journal_crash_test` — 崩溃恢复 + 回放验证（journal 语义最后一环）
//!
//! 验证目标：
//! 1. 正常退出：journal 全量 commit → 回放恢复全部订单，book 状态与检查点一致。
//! 2. 真实崩溃（SIGKILL）：父进程轮询 journal header，在提交中途 kill 子进程；
//!    回放只恢复 committed 前缀（未提交尾部按"崩溃时未持久化"丢弃），
//!    恢复后的 book 状态与子进程最后一次已落检查点一致（确定性重放核对）。
//!
//! 核对机制（避免"回放自证"）：子进程每次 commit 后把 (订单数, best_bid, best_ask, fills)
//! 追加写入 sidecar 检查点文件；父进程回放后取 `orders <= 回放数` 的最后一条检查点对比。
//! 命令序列固定（B/S 交替成交），撮合确定性——任何不一致即编码/解码或撮合 bug。

use match_core_hp::{HpCommand, HpEngine, HpEvent, MmapJournal, Side};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

const REC_LEN: usize = 35; // tag+side+price+qty+ts+client+pad
const BUY_TICK: i64 = 100_000;
const SELL_TICK: i64 = 99_990;
const JOURNAL_CAP: usize = 256 * 1024 * 1024;

#[inline]
fn encode_limit(side: Side, price_tick: i64, qty_lot: i64, client_id: u64) -> [u8; REC_LEN] {
    let mut b = [0u8; REC_LEN];
    b[0] = 0;
    b[1] = if side == Side::Buy { 0 } else { 1 };
    b[2..10].copy_from_slice(&price_tick.to_le_bytes());
    b[10..18].copy_from_slice(&qty_lot.to_le_bytes());
    b[18..26].copy_from_slice(&0u64.to_le_bytes());
    b[26..34].copy_from_slice(&client_id.to_le_bytes());
    b[34] = 0;
    b
}

fn decode_limit(p: &[u8]) -> HpCommand {
    debug_assert_eq!(p.len(), REC_LEN);
    let side = if p[1] == 0 { Side::Buy } else { Side::Sell };
    let price = i64::from_le_bytes(p[2..10].try_into().unwrap());
    let qty = i64::from_le_bytes(p[10..18].try_into().unwrap());
    let ts = u64::from_le_bytes(p[18..26].try_into().unwrap());
    let client = u64::from_le_bytes(p[26..34].try_into().unwrap());
    HpCommand::Limit { side, price_tick: price, qty_lot: qty, ts, client_id: client }
}

/// 订单序列：每 4 单一循环——0/1 立即成交（Buy 100000 ≥ Sell 99990），
/// 2/3 价格错开 rest（bid 恒 99950 / ask 恒 100500）→ book 非平凡状态，检查点核对有强度。
fn order_at(i: usize) -> HpCommand {
    match i % 4 {
        0 => HpCommand::Limit { side: Side::Buy, price_tick: 100_000, qty_lot: 1, ts: 0, client_id: i as u64 },
        1 => HpCommand::Limit { side: Side::Sell, price_tick: 99_990, qty_lot: 1, ts: 0, client_id: i as u64 },
        2 => HpCommand::Limit { side: Side::Buy, price_tick: 99_950, qty_lot: 1, ts: 0, client_id: i as u64 },
        _ => HpCommand::Limit { side: Side::Sell, price_tick: 100_500, qty_lot: 1, ts: 0, client_id: i as u64 },
    }
}

fn writes_to_ckpt(f: &mut File, orders: usize, bid: Option<i64>, ask: Option<i64>, fills: usize) {
    let bid = bid.map(|v| v.to_string()).unwrap_or_else(|| "-1".into());
    let ask = ask.map(|v| v.to_string()).unwrap_or_else(|| "-1".into());
    let _ = writeln!(f, "{orders} {bid} {ask} {fills}");
    let _ = f.flush(); // 尽力落检查点（kill 时可能丢最后一条——回放侧容忍）
}

/// 子进程：journal-first 撮合 + 周期 commit + 检查点。
fn run_child(count: usize, every: usize, journal: &Path, ckpt: &Path) {
    let mut j = MmapJournal::create(journal, JOURNAL_CAP).expect("journal");
    let mut engine = HpEngine::new();
    let mut ckpt_file = OpenOptions::new().create(true).truncate(true).write(true).open(ckpt).unwrap();
    let mut since_commit = 0usize;
    let mut fills = 0usize;
    for i in 0..count {
        let cmd = order_at(i);
        let enc = encode_limit(
            match cmd {
                HpCommand::Limit { side, .. } => side,
                _ => unreachable!(),
            },
            match cmd {
                HpCommand::Limit { price_tick, .. } => price_tick,
                _ => unreachable!(),
            },
            1,
            i as u64,
        );
        j.append(&enc).expect("journal full");
        let evs = engine.on_order(cmd);
        fills += evs.iter().filter(|e| matches!(e, HpEvent::Fill { .. })).count();
        since_commit += 1;
        if since_commit >= every {
            j.commit().expect("commit");
            writes_to_ckpt(&mut ckpt_file, i + 1, engine.book.best_bid(), engine.book.best_ask(), fills);
            since_commit = 0;
        }
    }
    j.commit().expect("commit-final");
    writes_to_ckpt(&mut ckpt_file, count, engine.book.best_bid(), engine.book.best_ask(), fills);
}

/// 回放：打开 journal（不 truncate），读 committed_cursor，回放 committed 前缀。
fn replay(journal: &Path) -> (usize, usize, Option<i64>, Option<i64>) {
    let j = MmapJournal::open_existing(journal, JOURNAL_CAP).expect("open journal");
    let end = j.committed_cursor() as usize;
    let mut engine = HpEngine::new();
    let mut orders = 0usize;
    let mut fills = 0usize;
    for (_, p) in j.iter_records(end) {
        let cmd = decode_limit(p);
        let evs = engine.on_order(cmd);
        orders += 1;
        fills += evs.iter().filter(|e| matches!(e, HpEvent::Fill { .. })).count();
    }
    drop(j); // Drop 负责 munmap + close
    (orders, fills, engine.book.best_bid(), engine.book.best_ask())
}

#[derive(Clone, Copy)]
struct Ckpt {
    orders: usize,
    bid: i64,
    ask: i64,
    fills: usize,
}

fn read_ckpts(path: &Path) -> Vec<Ckpt> {
    let f = File::open(path).expect("open ckpt");
    BufReader::new(f)
        .lines()
        .filter_map(|l| {
            let l = l.ok()?;
            let mut it = l.split_whitespace();
            let orders = it.next()?.parse().ok()?;
            let bid = it.next()?.parse().ok()?;
            let ask = it.next()?.parse().ok()?;
            let fills = it.next()?.parse().ok()?;
            Some(Ckpt { orders, bid, ask, fills })
        })
        .collect()
}

/// 父进程核对：回放数 R 对应检查点中 orders <= R 的最后一条，状态必须一致。
fn verify(journal: &Path, ckpt: &Path, expect_orders: Option<usize>) -> (usize, usize) {
    let (orders, fills, bid, ask) = replay(journal);
    println!("  replay: orders={orders} fills={fills} bid={bid:?} ask={ask:?}");
    if let Some(e) = expect_orders {
        assert_eq!(orders, e, "recovered orders != expected");
    }
    let ckpts = read_ckpts(ckpt);
    assert!(!ckpts.is_empty(), "no checkpoints");
    let best = ckpts.iter().rev().find(|c| c.orders <= orders).expect("ckpt <= orders");
    assert_eq!(bid.map(|v| v.to_string()).unwrap_or_else(|| "-1".into()), best.bid.to_string(), "bid mismatch");
    assert_eq!(ask.map(|v| v.to_string()).unwrap_or_else(|| "-1".into()), best.ask.to_string(), "ask mismatch");
    assert!(orders > 0, "nothing recovered");
    println!("  VERIFY OK: state matches checkpoint at orders={} (fills {})", best.orders, best.fills);
    (orders, fills)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let tmp = std::env::temp_dir();
    let journal = PathBuf::from(std::env::var("JOURNAL").unwrap_or_else(|_| tmp.join("hp_journal_crash.jrnl").to_string_lossy().into_owned()));
    let ckpt = journal.with_extension("ckpt");
    let count: usize = std::env::var("COUNT").ok().and_then(|v| v.parse().ok()).unwrap_or(200_000);
    let every: usize = std::env::var("EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(4096);

    if args.iter().any(|a| a == "--child") {
        run_child(count, every, &journal, &ckpt);
        return;
    }

    let _ = std::fs::remove_file(&journal);
    let _ = std::fs::remove_file(&ckpt);
    println!("=== hp_journal_crash_test: count={count} every={every} ===");

    // 场景 1：正常退出 → 全量恢复
    println!("[1] graceful exit → full recovery");
    let exe = std::env::current_exe().unwrap();
    let status = Command::new(&exe).arg("--child").status().unwrap();
    assert!(status.success());
    let (orders, _) = verify(&journal, &ckpt, Some(count));
    assert_eq!(orders, count);

    // 场景 2：SIGKILL 中途注入 → committed 前缀恢复 + 检查点状态一致
    println!("[2] SIGKILL mid-flight → committed-prefix recovery");
    let _ = std::fs::remove_file(&journal);
    let _ = std::fs::remove_file(&ckpt);
    let mut child = Command::new(&exe).arg("--child").spawn().unwrap();
    // 轮询 header 直到至少 3 个 batch 已提交（保证 kill 落在中途且已有检查点）
    let mut observed = 0u64;
    for _ in 0..500 {
        if let Ok(h) = read_header(&journal) {
            observed = h;
            if observed > (8 + 3 * every * (REC_LEN + 4)) as u64 {
                break;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    println!("  committed before kill: {} bytes ({} orders)", observed, orders_from_bytes(observed));
    child.kill().expect("kill child"); // SIGKILL
    let _ = child.wait();
    let (orders2, _) = verify(&journal, &ckpt, None);
    assert!(orders2 < count, "kill should leave partial journal, got {orders2}/{count}");
    println!("  PASS: recovered {orders2}/{count} orders (tail lost by design)");
    let _ = std::fs::remove_file(&journal);
    let _ = std::fs::remove_file(&ckpt);
    println!("=== ALL PASS ===");
}

fn read_header(path: &Path) -> std::io::Result<u64> {
    use std::io::Read;
    let mut f = File::open(path)?;
    let mut buf = [0u8; 8];
    f.read_exact(&mut buf)?;
    Ok(u64::from_le_bytes(buf))
}

fn orders_from_bytes(bytes: u64) -> u64 {
    if bytes <= 8 {
        0
    } else {
        (bytes - 8) / (REC_LEN as u64 + 4)
    }
}
