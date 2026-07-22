use crate::types::HpOrder;

/// Preallocated order slots with a free list for O(1) alloc/free.
#[derive(Debug, Default)]
pub struct OrderStore {
    slots: Vec<Option<HpOrder>>,
    free: Vec<u64>,
    live: usize,
}

impl OrderStore {
    pub fn new() -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            live: 0,
        }
    }

    pub fn with_capacity(cap: usize) -> Self {
        Self {
            slots: Vec::with_capacity(cap),
            free: Vec::with_capacity(cap / 2),
            live: 0,
        }
    }

    /// Number of occupied slots (not free-list size).
    pub fn live_len(&self) -> usize {
        self.live
    }

    /// Insert order; assigns and returns `id` (1-based slot index).
    pub fn insert(&mut self, mut order: HpOrder) -> u64 {
        let id = if let Some(id) = self.free.pop() {
            id
        } else {
            self.slots.push(None);
            self.slots.len() as u64
        };
        order.id = id;
        let idx = (id - 1) as usize;
        self.slots[idx] = Some(order);
        self.live += 1;
        id
    }

    pub fn get(&self, id: u64) -> Option<&HpOrder> {
        let idx = id.checked_sub(1)? as usize;
        self.slots.get(idx)?.as_ref()
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut HpOrder> {
        let idx = id.checked_sub(1)? as usize;
        self.slots.get_mut(idx)?.as_mut()
    }

    pub fn remove(&mut self, id: u64) -> Option<HpOrder> {
        let idx = id.checked_sub(1)? as usize;
        let order = self.slots.get_mut(idx)?.take()?;
        self.free.push(id);
        self.live -= 1;
        Some(order)
    }

    pub fn contains(&self, id: u64) -> bool {
        self.get(id).is_some()
    }
}
