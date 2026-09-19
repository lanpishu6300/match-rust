//! Fixed-capacity ring cache of published messages, shared between the
//! publisher (writer) and the retransmit server (reader) so NAKs can be served
//! from recent history without touching the network stack.
//!
//! The cache is indexed modulo `capacity` by sequence number. A slot is only
//! considered a hit when the stored message's seq matches the queried seq
//! (overwrite detection is implicit: older messages evicted by newer ones).

use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedMessage {
    pub seq: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct MessageRingBuf {
    slots: Vec<Option<CachedMessage>>,
    capacity: usize,
    write_pos: usize,
    /// Sequence of the first message ever pushed; fixes the seq→slot mapping
    /// (`(seq - base) % capacity`), which is stable even across evictions.
    base_seq: Option<u64>,
    /// Lowest sequence still present in the cache (best effort; grows as the
    /// ring wraps). `None` while empty.
    pub first_seq: Option<u64>,
}

impl MessageRingBuf {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "ring capacity must be > 0");
        Self {
            slots: vec![None; capacity],
            capacity,
            write_pos: 0,
            base_seq: None,
            first_seq: None,
        }
    }

    #[inline]
    fn index_of(&self, seq: u64) -> usize {
        let base = self.base_seq.expect("index_of on empty ring");
        (seq.wrapping_sub(base) % self.capacity as u64) as usize
    }

    /// Store a message, evicting the oldest on wrap. Sequence numbers are
    /// expected to arrive in strictly increasing order.
    pub fn push(&mut self, seq: u64, payload: Vec<u8>) {
        if self.slots[self.write_pos].is_some() {
            // Slot about to be overwritten: advance the low watermark.
            let evicted = self.write_pos;
            let evicted_seq = self
                .slots[evicted]
                .as_ref()
                .map(|m| m.seq)
                .unwrap_or(self.first_seq.unwrap_or(0));
            self.first_seq = Some(
                self.first_seq
                    .map(|f| f.max(evicted_seq + 1))
                    .unwrap_or(evicted_seq + 1),
            );
        }
        self.slots[self.write_pos] = Some(CachedMessage { seq, payload });
        self.write_pos = (self.write_pos + 1) % self.capacity;
        self.base_seq = self.base_seq.or(Some(seq));
        self.first_seq = self.first_seq.or(Some(seq));
    }

    pub fn get(&self, seq: u64) -> Option<&CachedMessage> {
        match &self.slots[self.index_of(seq)] {
            Some(m) if m.seq == seq => Some(m),
            _ => None,
        }
    }

    /// Fetch a consecutive range `[start, start+count)`; stops at the first
    /// missing sequence so callers never receive a sparse reply. Returns the
    /// messages that are actually available.
    pub fn get_range(&self, start: u64, count: u64) -> Vec<CachedMessage> {
        let mut out = Vec::new();
        for seq in start..start.saturating_add(count) {
            match self.get(seq) {
                Some(m) => out.push(m.clone()),
                None => break,
            }
        }
        out
    }

    pub fn contains(&self, seq: u64) -> bool {
        self.get(seq).is_some()
    }

    pub fn len(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Lowest sequence guaranteed absent (older messages were evicted).
    pub fn eviction_floor(&self) -> Option<u64> {
        self.first_seq
    }
}

/// Convenience alias used by publisher + retransmit server sharing.
pub type SharedRingBuf = Arc<Mutex<MessageRingBuf>>;

pub fn new_shared(capacity: usize) -> SharedRingBuf {
    Arc::new(Mutex::new(MessageRingBuf::new(capacity)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_get_and_eviction() {
        let mut ring = MessageRingBuf::new(4);
        for i in 1..=6u64 {
            ring.push(i, vec![i as u8]);
        }
        // Capacity 4 => only 3..=6 survive.
        assert!(!ring.contains(1));
        assert!(!ring.contains(2));
        assert!(ring.contains(3));
        assert!(ring.contains(6));
        assert_eq!(ring.len(), 4);
        assert_eq!(ring.eviction_floor(), Some(3));
    }

    #[test]
    fn get_range_contiguous_only() {
        let mut ring = MessageRingBuf::new(16);
        for i in 1..=10u64 {
            ring.push(i, vec![i as u8]);
        }
        assert_eq!(ring.get_range(4, 3).len(), 3); // 4,5,6
        assert_eq!(ring.get_range(9, 10).len(), 2); // 9,10 then stop
        assert_eq!(ring.get_range(11, 3).len(), 0); // missing
    }

    #[test]
    fn first_seq_tracking() {
        let mut ring = MessageRingBuf::new(2);
        assert_eq!(ring.first_seq, None);
        ring.push(10, vec![]);
        ring.push(11, vec![]);
        ring.push(12, vec![]); // evicts 10
        assert_eq!(ring.eviction_floor(), Some(11));
    }
}
