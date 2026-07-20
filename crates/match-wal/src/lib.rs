//! Async batched write-ahead log inspired by `crypto-exchange` persistence buffers.
//!
//! Hot path: `append` into a bounded channel (no disk wait in `Async` mode).
//! Background thread batches records and writes to a file.

use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use thiserror::Error;

/// Durability mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalMode {
    /// Append returns after queue accept; flusher writes in batches.
    Async,
    /// Append blocks until the record is written (and optionally synced).
    Sync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum RecordKind {
    OrderAccepted = 1,
    Fill = 2,
    Cancel = 3,
}

/// Fixed-layout log record (24 bytes + kind).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalRecord {
    pub kind: RecordKind,
    pub id_a: u64,
    pub id_b: u64,
    pub price_tick: i64,
    pub qty_lot: i64,
}

impl WalRecord {
    pub fn encode(&self) -> [u8; 33] {
        let mut buf = [0u8; 33];
        buf[0] = self.kind as u8;
        buf[1..9].copy_from_slice(&self.id_a.to_le_bytes());
        buf[9..17].copy_from_slice(&self.id_b.to_le_bytes());
        buf[17..25].copy_from_slice(&self.price_tick.to_le_bytes());
        buf[25..33].copy_from_slice(&self.qty_lot.to_le_bytes());
        buf
    }
}

#[derive(Debug, Error)]
pub enum WalError {
    #[error("wal queue full (backpressure)")]
    Busy,
    #[error("wal closed")]
    Closed,
    #[error(transparent)]
    Io(#[from] io::Error),
}

enum Msg {
    Rec(WalRecord),
    Flush(mpsc::Sender<io::Result<()>>),
    Shutdown,
}

/// Handle for appending records; owns the background flusher.
pub struct Wal {
    tx: SyncSender<Msg>,
    join: Option<JoinHandle<()>>,
    mode: WalMode,
    /// When true, [`Drop`] skips sending `Shutdown` (tests only).
    skip_shutdown: bool,
}

impl Wal {
    /// Open (create) `path` and start a flusher thread.
    pub fn open(path: impl AsRef<Path>, mode: WalMode, queue_cap: usize) -> Result<Self, WalError> {
        let path = path.as_ref().to_path_buf();
        let (tx, rx) = mpsc::sync_channel(queue_cap.max(1));
        let join = thread::Builder::new()
            .name("match-wal-flush".into())
            .spawn(move || flush_loop(path, rx))
            .map_err(WalError::Io)?;
        Ok(Self {
            tx,
            join: Some(join),
            mode,
            skip_shutdown: false,
        })
    }

    pub fn append(&self, rec: WalRecord) -> Result<(), WalError> {
        match self.mode {
            WalMode::Async => self.tx.try_send(Msg::Rec(rec)).map_err(|e| match e {
                mpsc::TrySendError::Full(_) => WalError::Busy,
                mpsc::TrySendError::Disconnected(_) => WalError::Closed,
            }),
            WalMode::Sync => {
                self.tx.send(Msg::Rec(rec)).map_err(|_| WalError::Closed)?;
                let (ack_tx, ack_rx) = mpsc::channel();
                self.tx
                    .send(Msg::Flush(ack_tx))
                    .map_err(|_| WalError::Closed)?;
                ack_rx.recv().map_err(|_| WalError::Closed)??;
                Ok(())
            }
        }
    }

    /// Request a flush of buffered records (Async mode).
    pub fn flush(&self) -> Result<(), WalError> {
        let (ack_tx, ack_rx) = mpsc::channel();
        self.tx
            .send(Msg::Flush(ack_tx))
            .map_err(|_| WalError::Closed)?;
        ack_rx.recv().map_err(|_| WalError::Closed)??;
        Ok(())
    }
}

impl Drop for Wal {
    fn drop(&mut self) {
        if !self.skip_shutdown {
            let _ = self.tx.send(Msg::Shutdown);
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

#[cfg(test)]
impl Wal {
    /// Drop the sender without `Shutdown` (coverage: flusher `Disconnected` path).
    fn drop_sender_without_shutdown(mut self) {
        self.skip_shutdown = true;
        let join = self.join.take();
        let (dead_tx, dead_rx) = mpsc::sync_channel(1);
        drop(dead_rx);
        // Replace sender; old `tx` drops → flusher sees Disconnected.
        self.tx = dead_tx;
        drop(self);
        if let Some(j) = join {
            let _ = j.join();
        }
    }
}

fn flush_loop(path: PathBuf, rx: Receiver<Msg>) {
    let mut file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("match-wal: open failed: {e}");
            return;
        }
    };
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let batch_deadline = Duration::from_millis(5);
    let mut last_write = Instant::now();

    loop {
        let msg = if buf.is_empty() {
            rx.recv().ok()
        } else {
            match rx.recv_timeout(batch_deadline.saturating_sub(last_write.elapsed())) {
                Ok(m) => Some(m),
                Err(RecvTimeoutError::Timeout) => {
                    if let Err(e) = write_buf(&mut file, &mut buf) {
                        eprintln!("match-wal: flush on timeout failed: {e}");
                    }
                    last_write = Instant::now();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => None,
            }
        };

        let Some(msg) = msg else {
            if let Err(e) = write_buf(&mut file, &mut buf) {
                eprintln!("match-wal: final flush failed: {e}");
            }
            break;
        };

        match msg {
            Msg::Rec(r) => {
                buf.extend_from_slice(&r.encode());
                if buf.len() >= 32 * 1024 {
                    if let Err(e) = write_buf(&mut file, &mut buf) {
                        eprintln!("match-wal: batch write failed: {e}");
                    }
                    last_write = Instant::now();
                }
            }
            Msg::Flush(ack) => {
                let res = write_buf(&mut file, &mut buf);
                last_write = Instant::now();
                let _ = ack.send(res);
            }
            Msg::Shutdown => {
                if let Err(e) = write_buf(&mut file, &mut buf) {
                    eprintln!("match-wal: shutdown flush failed: {e}");
                }
                break;
            }
        }
    }
}

fn write_buf(file: &mut std::fs::File, buf: &mut Vec<u8>) -> io::Result<()> {
    if buf.is_empty() {
        return Ok(());
    }
    file.write_all(buf)?;
    file.flush()?;
    // Crash-durable for Sync mode and Flush acks.
    file.sync_all()?;
    buf.clear();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn async_append_and_flush() {
        let dir = std::env::temp_dir().join(format!("match-wal-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("test.wal");
        let _ = fs::remove_file(&path);

        let wal = Wal::open(&path, WalMode::Async, 1024).unwrap();
        for i in 0..100 {
            wal.append(WalRecord {
                kind: RecordKind::Fill,
                id_a: i,
                id_b: i + 1,
                price_tick: 10_000,
                qty_lot: 1,
            })
            .unwrap();
        }
        wal.flush().unwrap();
        drop(wal);

        let bytes = fs::read(&path).unwrap();
        assert_eq!(bytes.len(), 100 * 33);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn disconnect_while_buffered_hits_disconnected_arm() {
        let dir = std::env::temp_dir().join(format!("match-wal-disc-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let path = dir.join("d.wal");
        let _ = fs::remove_file(&path);

        let wal = Wal::open(&path, WalMode::Async, 4096).unwrap();
        for i in 0..80 {
            wal.append(WalRecord {
                kind: RecordKind::Fill,
                id_a: i,
                id_b: i + 1,
                price_tick: 1,
                qty_lot: 1,
            })
            .unwrap();
        }
        // Give flusher time to buffer without Shutdown, then disconnect sender.
        thread::sleep(Duration::from_millis(5));
        wal.drop_sender_without_shutdown();
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn sync_append_closed_when_flusher_dead() {
        let dir = std::env::temp_dir().join(format!("match-wal-sync-dead-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let blocker = dir.join("file");
        fs::write(&blocker, b"x").unwrap();
        let path = blocker.join("x.wal");
        let wal = Wal::open(&path, WalMode::Sync, 8).unwrap();
        thread::sleep(Duration::from_millis(20));
        let err = wal.append(WalRecord {
            kind: RecordKind::Cancel,
            id_a: 1,
            id_b: 0,
            price_tick: 0,
            qty_lot: 0,
        });
        assert!(matches!(err, Err(WalError::Closed)));
        // Drop may also hit Closed on flush ack path.
        drop(wal);
        let _ = fs::remove_dir_all(&dir);
    }
}
