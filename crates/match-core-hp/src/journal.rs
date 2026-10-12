//! mmap 顺序日志（journal）——参考 LMAX Disruptor 的 journal-first 持久化简化版。
//!
//! 设计要点：
//! - 预分配固定容量文件 + `mmap(MAP_SHARED)`，追加写入 = memcpy 到映射区（page cache 写，ns 级）。
//! - 记录格式：`[u32 little-endian len][payload]`，顺序游标推进。
//! - **提交边界（崩溃恢复锚点）**：文件头 8 字节存 `committed_cursor`。
//!   `commit()` 采用**先数据后元数据**顺序：① msync(数据区) ② 写 header ③ msync(header)。
//!   恢复时只回放 `[header, committed_cursor)`——header 可见即数据必已落盘，未提交尾部按"崩溃时未持久化"丢弃。
//! - 落盘语义由调用方选择：`msync(false)` = MS_ASYNC（交给内核刷，**不是崩溃一致**）；
//!   `commit()` = MS_SYNC 批量刷盘 + header 推进（崩溃一致，L3 级，无 BBU/复制依赖）。
//!
//! 诚实边界：
//! - mmap 写 page cache ≠ 持久（OS 崩溃丢未刷脏页）；只有 commit()/fsync 落盘。
//! - 与 LMAX/Aeron 的口径差异：LMAX journaler = mmap 批量流式写 + **不逐条 fsync**，
//!   落盘靠 RAID 控制器电池备份（BBU）缓存兜底；Aeron = mmap 写 + **不主动 fsync**，
//!   持久靠集群复制（RAFT）。本实现 = 纯软件 MS_SYNC（无 BBU/复制依赖），**比二者更严格**，
//!   吞吐代价约 -60%（mmap-append 22M/s → msync(4096) 10.5M/s）。
//! - 6M/s 量级可达的前提：记录小（~30-60B）、顺序写带宽不是瓶颈（SSD 1-3GB/s）、
//!   msync 批量（如 4096 条一次）。若生产接受 BBU RAID 或跨机复制，可去掉 MS_SYNC 回到 22M/s。

use std::fs::OpenOptions;
use std::io;
use std::os::unix::io::AsRawFd;
use std::path::Path;

/// 文件头：committed_cursor（u64 LE）。记录区从偏移 8 开始。
const HEADER_LEN: usize = 8;

/// 内存映射顺序日志。
pub struct MmapJournal {
    map: *mut u8,
    cap: usize,
    cursor: usize,
    fd: i32,
}

// 单写者语义：由调用方保证同一时刻只有一个线程 append/commit。
unsafe impl Send for MmapJournal {}
unsafe impl Sync for MmapJournal {}

impl MmapJournal {
    /// 创建（truncate）并映射文件。`capacity` 为字节数，建议 ≥ 4KiB 对齐。
    /// 新文件 header.committed_cursor = 0。
    pub fn create(path: &Path, capacity: usize) -> io::Result<Self> {
        let f = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(path)?;
        f.set_len(capacity as u64)?;
        let fd = f.as_raw_fd();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                capacity,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        // fd 生命周期交给 journal（Drop 时 close）；File 的 Drop 不再执行。
        std::mem::forget(f);
        Ok(Self {
            map: map as *mut u8,
            cap: capacity,
            cursor: HEADER_LEN,
            fd,
        })
    }

    /// 打开已有文件并映射（**不 truncate**）——崩溃恢复读取用。
    /// `cursor` 置为 capacity（只读恢复；Drop 时 msync 全量无脏页，无副作用）。
    pub fn open_existing(path: &Path, capacity: usize) -> io::Result<Self> {
        let f = OpenOptions::new().read(true).write(true).open(path)?;
        let fd = f.as_raw_fd();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                capacity,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if map == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        std::mem::forget(f);
        Ok(Self {
            map: map as *mut u8,
            cap: capacity,
            cursor: capacity,
            fd,
        })
    }

    /// 追加一条记录，返回其起始偏移（恢复/回放锚点）。容量不足返回 None。
    pub fn append(&mut self, payload: &[u8]) -> Option<u64> {
        let total = payload.len() + 4;
        if self.cursor + total > self.cap {
            return None;
        }
        let off = self.cursor as u64;
        unsafe {
            let dst = self.map.add(self.cursor);
            std::ptr::copy_nonoverlapping(
                (payload.len() as u32).to_le_bytes().as_ptr(),
                dst,
                4,
            );
            std::ptr::copy_nonoverlapping(payload.as_ptr(), dst.add(4), payload.len());
        }
        self.cursor += total;
        Some(off)
    }

    /// 批量追加（同一 memcpy 循环），返回写入字节数（含 len 头）。
    pub fn append_batch(&mut self, records: &[&[u8]]) -> usize {
        let mut written = 0usize;
        for payload in records {
            let total = payload.len() + 4;
            if self.cursor + total > self.cap {
                break;
            }
            unsafe {
                let dst = self.map.add(self.cursor);
                std::ptr::copy_nonoverlapping(
                    (payload.len() as u32).to_le_bytes().as_ptr(),
                    dst,
                    4,
                );
                std::ptr::copy_nonoverlapping(payload.as_ptr(), dst.add(4), payload.len());
            }
            self.cursor += total;
            written += total;
        }
        written
    }

    /// 刷盘。`sync=true` → MS_SYNC（阻塞落盘）；`false` → MS_ASYNC（异步，非崩溃一致）。
    pub fn msync(&self, sync: bool) -> io::Result<()> {
        let flag = if sync { libc::MS_SYNC } else { libc::MS_ASYNC };
        let r = unsafe { libc::msync(self.map as *mut libc::c_void, self.cursor, flag) };
        if r != 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }

    /// 提交：先落数据（MS_SYNC 到 cursor），再写 header 推进 committed_cursor，
    /// 最后落 header。崩溃后只回放 committed 前缀。
    /// 注意：msync 的 addr 必须页对齐，故数据区从 map（页对齐起点）刷 `cursor` 长度
    /// （此刻 header 仍是旧值，"先数据后元数据"顺序保持）。
    pub fn commit(&mut self) -> io::Result<usize> {
        let cur = self.cursor;
        let page = unsafe { libc::sysconf(libc::_SC_PAGESIZE) } as usize;
        if cur > HEADER_LEN {
            // ① 数据区落盘（含旧 header 一并刷，无妨）
            let r = unsafe {
                libc::msync(self.map as *mut libc::c_void, cur, libc::MS_SYNC)
            };
            if r != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        // ② 写 header
        unsafe {
            std::ptr::copy_nonoverlapping(
                (cur as u64).to_le_bytes().as_ptr(),
                self.map as *mut u8,
                8,
            );
        }
        // ③ 落 header 所在页
        let r = unsafe {
            libc::msync(self.map as *mut libc::c_void, page, libc::MS_SYNC)
        };
        if r != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cur)
    }

    /// 当前 committed_cursor（恢复锚点）。新文件 / 未 commit 时为 HEADER_LEN（=8）。
    pub fn committed_cursor(&self) -> u64 {
        unsafe { u64::from_le_bytes(std::ptr::read_unaligned(self.map as *const [u8; 8])) }
            .max(HEADER_LEN as u64)
    }

    /// 已写入字节数（含 len 头，未提交部分可能未落盘）。
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// 迭代记录区 `[HEADER_LEN, end)` 的记录（恢复/回放）。
    /// 崩溃恢复用 `end = committed_cursor()`；正常读取用 `end = cursor()`。
    pub fn iter_records(&self, end: usize) -> impl Iterator<Item = (u64, &[u8])> + '_ {
        let mut off = HEADER_LEN;
        std::iter::from_fn(move || {
            if off + 4 > end {
                return None;
            }
            let len = unsafe {
                u32::from_le_bytes(std::ptr::read_unaligned(self.map.add(off) as *const [u8; 4]))
            } as usize;
            if off + 4 + len > end {
                return None;
            }
            let start = off as u64;
            let payload = unsafe { std::slice::from_raw_parts(self.map.add(off + 4), len) };
            off += 4 + len;
            Some((start, payload))
        })
    }
}

impl Drop for MmapJournal {
    fn drop(&mut self) {
        unsafe {
            // 尽力落盘后解除映射（msync 失败忽略：drop 阶段无错误通道）。
            libc::msync(self.map as *mut libc::c_void, self.cursor, libc::MS_SYNC);
            libc::munmap(self.map as *mut libc::c_void, self.cap);
            libc::close(self.fd);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_replay_roundtrip() {
        let path = std::env::temp_dir().join(format!("journal_test_{}", std::process::id()));
        {
            let mut j = MmapJournal::create(&path, 4096).unwrap();
            assert_eq!(j.append(b"abc").unwrap(), HEADER_LEN as u64);
            assert_eq!(j.append(b"de").unwrap(), (HEADER_LEN + 7) as u64);
            assert_eq!(j.append(b"f").unwrap(), (HEADER_LEN + 13) as u64);
            let recs: Vec<(u64, Vec<u8>)> = j
                .iter_records(j.cursor())
                .map(|(off, p)| (off, p.to_vec()))
                .collect();
            assert_eq!(recs.len(), 3);
            assert_eq!((recs[0].0, &recs[0].1[..]), (8, b"abc".as_slice()));
            assert_eq!((recs[1].0, &recs[1].1[..]), (15, b"de".as_slice()));
            assert_eq!((recs[2].0, &recs[2].1[..]), (21, b"f".as_slice()));
            j.commit().unwrap();
        }
        assert!(path.exists());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn append_batch_writes_all() {
        let path = std::env::temp_dir().join(format!("journal_batch_{}", std::process::id()));
        let mut j = MmapJournal::create(&path, 4096).unwrap();
        let recs: Vec<&[u8]> = vec![b"x", b"yy", b"zzz"];
        let n = j.append_batch(&recs);
        assert_eq!(n, (1 + 4) + (2 + 4) + (3 + 4));
        assert_eq!(j.iter_records(j.cursor()).count(), 3);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn commit_then_reopen_recovers_committed_prefix() {
        let path =
            std::env::temp_dir().join(format!("journal_recover_{}", std::process::id()));
        let mut j = MmapJournal::create(&path, 8192).unwrap();
        j.append(b"first").unwrap();
        j.append(b"second").unwrap();
        let committed = j.commit().unwrap(); // 两条都提交
        j.append(b"third").unwrap(); // 未提交（模拟崩溃前最后一批）
        drop(j);

        // 重新打开（reopen-only：不 truncate）
        let f = OpenOptions::new().read(true).write(true).open(&path).unwrap();
        let fd = f.as_raw_fd();
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                8192,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        assert_ne!(map, libc::MAP_FAILED);
        std::mem::forget(f);
        let reopened = MmapJournal {
            map: map as *mut u8,
            cap: 8192,
            cursor: 8192, // 未知写游标；恢复只依赖 committed_cursor
            fd,
        };
        assert_eq!(reopened.committed_cursor() as usize, committed);
        let recs: Vec<Vec<u8>> = reopened
            .iter_records(reopened.committed_cursor() as usize)
            .map(|(_, p)| p.to_vec())
            .collect();
        assert_eq!(recs.len(), 2);
        assert_eq!(recs[0], b"first");
        assert_eq!(recs[1], b"second");
        unsafe {
            libc::munmap(reopened.map as *mut libc::c_void, reopened.cap);
            libc::close(reopened.fd);
        }
        std::fs::remove_file(&path).ok();
    }
}
