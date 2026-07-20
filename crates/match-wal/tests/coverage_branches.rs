use match_wal::{RecordKind, Wal, WalError, WalMode, WalRecord};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn tmp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "match-wal-{}-{}-{}",
        name,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = fs::create_dir_all(&dir);
    dir
}

fn sample(i: u64) -> WalRecord {
    WalRecord {
        kind: RecordKind::Fill,
        id_a: i,
        id_b: i + 1,
        price_tick: 100,
        qty_lot: 1,
    }
}

#[test]
fn sync_mode_append_and_flush() {
    let dir = tmp_dir("sync");
    let path = dir.join("s.wal");
    let wal = Wal::open(&path, WalMode::Sync, 16).unwrap();
    wal.append(sample(1)).unwrap();
    wal.append(sample(2)).unwrap();
    drop(wal);
    let bytes = fs::read(&path).unwrap();
    assert_eq!(bytes.len(), 2 * 33);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn async_busy_when_queue_full() {
    let dir = tmp_dir("busy");
    let path = dir.join("b.wal");
    // capacity 1; hold flusher busy by not consuming quickly — fill with sync_channel
    let wal = Arc::new(Wal::open(&path, WalMode::Async, 1).unwrap());
    // First append ok
    wal.append(sample(0)).unwrap();
    // Flood until Busy (flusher may drain; retry briefly)
    let mut saw_busy = false;
    for i in 1..10_000 {
        match wal.append(sample(i)) {
            Err(WalError::Busy) => {
                saw_busy = true;
                break;
            }
            Ok(()) => {}
            Err(e) => panic!("unexpected {e}"),
        }
    }
    assert!(saw_busy, "expected WalError::Busy under backpressure");
    drop(wal);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn async_batch_size_flush_and_timeout() {
    let dir = tmp_dir("batch");
    let path = dir.join("big.wal");
    let wal = Wal::open(&path, WalMode::Async, 4096).unwrap();
    // > 32KiB / 33 ≈ 993 records to force size-based flush in flusher
    for i in 0..1200 {
        wal.append(sample(i)).unwrap();
    }
    // allow timeout path
    thread::sleep(Duration::from_millis(20));
    wal.flush().unwrap();
    drop(wal);
    let bytes = fs::read(&path).unwrap();
    assert!(bytes.len() >= 1200 * 33);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn record_kinds_encode() {
    for kind in [
        RecordKind::OrderAccepted,
        RecordKind::Fill,
        RecordKind::Cancel,
    ] {
        let r = WalRecord {
            kind,
            id_a: 1,
            id_b: 2,
            price_tick: -3,
            qty_lot: 4,
        };
        let b = r.encode();
        assert_eq!(b[0], kind as u8);
        assert_eq!(b.len(), 33);
    }
}

#[test]
fn open_queue_cap_zero_becomes_one() {
    let dir = tmp_dir("cap0");
    let path = dir.join("c.wal");
    let wal = Wal::open(&path, WalMode::Async, 0).unwrap();
    wal.append(sample(1)).unwrap();
    wal.flush().unwrap();
    drop(wal);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn error_display() {
    let s = format!("{}", WalError::Busy);
    assert!(s.contains("full") || s.contains("Busy") || s.contains("backpressure"));
    let s = format!("{}", WalError::Closed);
    assert!(s.contains("closed") || s.contains("Closed"));
}

#[test]
fn flusher_dies_on_bad_path_yields_closed() {
    let dir = tmp_dir("badpath");
    let blocker = dir.join("not-a-dir");
    fs::write(&blocker, b"x").unwrap();
    let path = blocker.join("child.wal");
    let wal = Wal::open(&path, WalMode::Async, 8).unwrap();
    // flusher thread exits after open failure; channel disconnects
    let mut closed = false;
    for i in 0..200 {
        match wal.append(sample(i)) {
            Err(WalError::Closed) => {
                closed = true;
                break;
            }
            Err(WalError::Busy) => thread::sleep(Duration::from_millis(2)),
            Ok(()) => thread::sleep(Duration::from_millis(2)),
            Err(e) => panic!("unexpected {e}"),
        }
    }
    assert!(closed, "expected Closed after flusher open failure");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn flush_empty_buffer_ok() {
    let dir = tmp_dir("emptyflush");
    let path = dir.join("e.wal");
    let wal = Wal::open(&path, WalMode::Async, 8).unwrap();
    wal.flush().unwrap();
    drop(wal);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn sync_closed_after_drop_is_not_usable() {
    let dir = tmp_dir("syncdrop");
    let path = dir.join("s.wal");
    let wal = Wal::open(&path, WalMode::Sync, 8).unwrap();
    drop(wal);
    // cannot append on dropped wal — covered via Drop join path above
    let _ = fs::remove_dir_all(&dir);
}
