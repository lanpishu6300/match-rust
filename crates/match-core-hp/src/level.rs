use std::collections::VecDeque;

/// Aggregated qty and FIFO order ids at one price tick.
#[derive(Debug, Default)]
pub(crate) struct Level {
    pub(crate) total_lot: i64,
    pub(crate) ids: VecDeque<u64>,
}

impl Level {
    pub(crate) fn push(&mut self, id: u64, lot: i64) {
        self.ids.push_back(id);
        self.total_lot += lot;
    }

    pub(crate) fn clear(&mut self) {
        self.total_lot = 0;
        self.ids.clear();
    }
}
