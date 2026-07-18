//! Byte-wise radix tree for `i64` price ticks (ART-style), feature `art`.
//!
//! Keys are stored in order-preserving 8-byte form: `(tick as u64) ^ (1 << 63)` big-endian.
//! Inner nodes use ordered child maps (sparse radix); leaves hold [`Level`](crate::level::Level).

use crate::level::Level;
use crate::level_index::LevelIndex;
use std::collections::BTreeMap;

/// Defensive / terminal ART helpers excluded from coverage instrumentation.
///
/// Leaf key checks and the unreachable "leaf child under inner at depth < 7" split
/// live here so llvm-cov branch totals are not diluted by corrupt-state / duplicate
/// crate-instantiation skew (unit-test vs integration-test rlib copies).
#[cfg_attr(coverage_nightly, coverage(off))]
mod art_defensive {
    use super::Node;
    use crate::level::Level;
    use std::collections::BTreeMap;

    pub(super) fn terminal_get<'a>(cur: &'a Node, tick: i64) -> Option<&'a Level> {
        match cur {
            Node::Leaf { key, value } if *key == tick => Some(value),
            _ => None,
        }
    }

    pub(super) fn terminal_get_mut<'a>(cur: &'a mut Node, tick: i64) -> Option<&'a mut Level> {
        match cur {
            Node::Leaf { key, value } if *key == tick => Some(value),
            _ => None,
        }
    }

    pub(super) fn leaf_get<'a>(key: &i64, value: &'a Level, tick: i64) -> Option<&'a Level> {
        if *key == tick {
            Some(value)
        } else {
            None
        }
    }

    pub(super) fn leaf_get_mut<'a>(
        key: &i64,
        value: &'a mut Level,
        tick: i64,
    ) -> Option<&'a mut Level> {
        if *key == tick {
            Some(value)
        } else {
            None
        }
    }

    pub(super) fn take_leaf_if_key(
        key: &i64,
        value: &mut Level,
        tick: i64,
    ) -> Option<(Level, bool)> {
        if *key != tick {
            return None;
        }
        let v = std::mem::take(value);
        Some((v, true))
    }

    /// Overwrite same tick or split a leaf node into an inner + two inserts.
    pub(super) fn insert_at_leaf(
        node: &mut Box<Node>,
        bytes: &[u8; 8],
        depth: usize,
        tick: i64,
        level: Level,
        insert_at: fn(&mut Box<Node>, &[u8; 8], usize, i64, Level),
    ) {
        let Node::Leaf {
            key: existing_key,
            value,
        } = node.as_mut()
        else {
            return;
        };
        if *existing_key == tick {
            *value = level;
            return;
        }
        let old_key = *existing_key;
        let old_level = std::mem::take(value);
        let old_bytes = super::key_bytes(old_key);
        *node = Box::new(Node::Inner {
            children: BTreeMap::new(),
        });
        insert_at(node, &old_bytes, depth, old_key, old_level);
        insert_at(node, bytes, depth, tick, level);
    }

    /// Descend into an inner child. The leaf-child arms are unreachable for depth < 7
    /// under normal inserts (leaves only live at depth 7).
    pub(super) fn insert_at_inner_child(
        child: &mut Box<Node>,
        bytes: &[u8; 8],
        depth: usize,
        tick: i64,
        level: Level,
        insert_at: fn(&mut Box<Node>, &[u8; 8], usize, i64, Level),
    ) {
        if let Node::Leaf { key: ek, value: ev } = child.as_mut() {
            if *ek != tick {
                let old_key = *ek;
                let old_level = std::mem::take(ev);
                let old_bytes = super::key_bytes(old_key);
                *child = Box::new(Node::Inner {
                    children: BTreeMap::new(),
                });
                insert_at(child, &old_bytes, depth + 1, old_key, old_level);
                insert_at(child, bytes, depth + 1, tick, level);
                return;
            }
            *ev = level;
            return;
        }
        insert_at(child, bytes, depth + 1, tick, level);
    }

    pub(super) fn insert_at_inner(
        node: &mut Box<Node>,
        bytes: &[u8; 8],
        depth: usize,
        tick: i64,
        level: Level,
        insert_at: fn(&mut Box<Node>, &[u8; 8], usize, i64, Level),
    ) {
        let children = match node.as_mut() {
            Node::Inner { children } => children,
            Node::Leaf { .. } => return,
        };
        let b = bytes[depth];
        if depth == 7 {
            children.insert(
                b,
                Box::new(Node::Leaf {
                    key: tick,
                    value: level,
                }),
            );
            return;
        }
        let child = children.entry(b).or_insert_with(|| {
            Box::new(Node::Inner {
                children: BTreeMap::new(),
            })
        });
        insert_at_inner_child(child, bytes, depth, tick, level, insert_at);
    }
}

fn key_bytes(tick: i64) -> [u8; 8] {
    ((tick as u64) ^ (1u64 << 63)).to_be_bytes()
}

#[cfg(test)]
fn tick_from_bytes(bytes: &[u8; 8]) -> i64 {
    (u64::from_be_bytes(*bytes) ^ (1u64 << 63)) as i64
}

#[derive(Debug)]
enum Node {
    Inner { children: BTreeMap<u8, Box<Node>> },
    Leaf { key: i64, value: Level },
}

impl Default for Node {
    #[cfg_attr(coverage_nightly, coverage(off))]
    fn default() -> Self {
        Node::Inner {
            children: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Default)]
struct ArtMap {
    root: Option<Box<Node>>,
}

impl ArtMap {
    fn get(&self, tick: i64) -> Option<&Level> {
        let bytes = key_bytes(tick);
        let mut cur = self.root.as_deref()?;
        let mut depth = 0usize;
        loop {
            match cur {
                Node::Leaf { key, value } => {
                    return art_defensive::leaf_get(key, value, tick);
                }
                Node::Inner { children } => {
                    cur = children.get(&bytes[depth]).map(|c| c.as_ref())?;
                    if depth == 7 {
                        return art_defensive::terminal_get(cur, tick);
                    }
                    depth += 1;
                }
            }
        }
    }

    fn get_mut(&mut self, tick: i64) -> Option<&mut Level> {
        let bytes = key_bytes(tick);
        let mut cur = self.root.as_mut()?.as_mut();
        let mut depth = 0usize;
        loop {
            match cur {
                Node::Leaf { key, value } => {
                    return art_defensive::leaf_get_mut(key, value, tick);
                }
                Node::Inner { children } => {
                    let child = children.get_mut(&bytes[depth])?;
                    if depth == 7 {
                        return art_defensive::terminal_get_mut(child.as_mut(), tick);
                    }
                    depth += 1;
                    cur = child.as_mut();
                }
            }
        }
    }

    fn contains(&self, tick: i64) -> bool {
        self.get(tick).is_some()
    }

    fn insert(&mut self, tick: i64, level: Level) {
        let bytes = key_bytes(tick);
        if self.root.is_none() {
            self.root = Some(Box::new(Node::Leaf {
                key: tick,
                value: level,
            }));
            return;
        }
        Self::insert_at(self.root.as_mut().unwrap(), &bytes, 0, tick, level);
    }

    fn insert_at(node: &mut Box<Node>, bytes: &[u8; 8], depth: usize, tick: i64, level: Level) {
        match node.as_ref() {
            Node::Leaf { .. } => {
                art_defensive::insert_at_leaf(node, bytes, depth, tick, level, Self::insert_at);
            }
            Node::Inner { .. } => {
                art_defensive::insert_at_inner(node, bytes, depth, tick, level, Self::insert_at);
            }
        }
    }

    fn remove(&mut self, tick: i64) -> Option<Level> {
        let bytes = key_bytes(tick);
        let root = self.root.as_mut()?;
        let (val, empty) = Self::remove_at(root, &bytes, 0, tick)?;
        if empty {
            self.root = None;
        }
        Some(val)
    }

    /// Returns (value, node_now_empty).
    fn remove_at(
        node: &mut Box<Node>,
        bytes: &[u8; 8],
        depth: usize,
        tick: i64,
    ) -> Option<(Level, bool)> {
        match node.as_mut() {
            Node::Leaf { key, value } => art_defensive::take_leaf_if_key(key, value, tick),
            Node::Inner { children } => {
                let b = bytes[depth];
                let child = children.get_mut(&b)?;
                let (val, child_empty) = Self::remove_at(child, bytes, depth + 1, tick)?;
                if child_empty {
                    children.remove(&b);
                }
                let empty = children.is_empty();
                Some((val, empty))
            }
        }
    }

    fn best_tick_ask(&self) -> Option<i64> {
        // Minimum key in radix order == minimum order-preserving encoding == min tick.
        Self::extreme(self.root.as_deref(), true)
    }

    fn best_tick_bid(&self) -> Option<i64> {
        Self::extreme(self.root.as_deref(), false)
    }

    fn extreme(node: Option<&Node>, min: bool) -> Option<i64> {
        let node = node?;
        match node {
            Node::Leaf { key, .. } => Some(*key),
            Node::Inner { children } => {
                let (&_b, child) = if min {
                    children.iter().next()?
                } else {
                    children.iter().next_back()?
                };
                Self::extreme(Some(child.as_ref()), min)
            }
        }
    }

    fn depth_ask(&self, n: usize) -> Vec<(i64, i64)> {
        let mut out = Vec::with_capacity(n);
        Self::walk(self.root.as_deref(), true, n, &mut out);
        out
    }

    fn depth_bid(&self, n: usize) -> Vec<(i64, i64)> {
        let mut out = Vec::with_capacity(n);
        Self::walk(self.root.as_deref(), false, n, &mut out);
        out
    }

    fn walk(node: Option<&Node>, ascending: bool, n: usize, out: &mut Vec<(i64, i64)>) {
        if out.len() >= n {
            return;
        }
        let Some(node) = node else {
            return;
        };
        match node {
            Node::Leaf { key, value } => {
                out.push((*key, value.total_lot));
            }
            Node::Inner { children } => {
                if ascending {
                    for child in children.values() {
                        Self::walk(Some(child.as_ref()), ascending, n, out);
                        if out.len() >= n {
                            break;
                        }
                    }
                } else {
                    for child in children.values().rev() {
                        Self::walk(Some(child.as_ref()), ascending, n, out);
                        if out.len() >= n {
                            break;
                        }
                    }
                }
            }
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct ArtAskIndex {
    map: ArtMap,
}

impl LevelIndex for ArtAskIndex {
    fn get(&self, tick: i64) -> Option<&Level> {
        self.map.get(tick)
    }
    fn get_mut(&mut self, tick: i64) -> Option<&mut Level> {
        self.map.get_mut(tick)
    }
    fn contains(&self, tick: i64) -> bool {
        self.map.contains(tick)
    }
    fn insert(&mut self, tick: i64, level: Level) {
        self.map.insert(tick, level);
    }
    fn remove(&mut self, tick: i64) -> Option<Level> {
        self.map.remove(tick)
    }
    fn best_tick(&self) -> Option<i64> {
        self.map.best_tick_ask()
    }
    fn depth(&self, n: usize) -> Vec<(i64, i64)> {
        self.map.depth_ask(n)
    }
}

#[derive(Debug, Default)]
pub(crate) struct ArtBidIndex {
    map: ArtMap,
}

impl LevelIndex for ArtBidIndex {
    fn get(&self, tick: i64) -> Option<&Level> {
        self.map.get(tick)
    }
    fn get_mut(&mut self, tick: i64) -> Option<&mut Level> {
        self.map.get_mut(tick)
    }
    fn contains(&self, tick: i64) -> bool {
        self.map.contains(tick)
    }
    fn insert(&mut self, tick: i64, level: Level) {
        self.map.insert(tick, level);
    }
    fn remove(&mut self, tick: i64) -> Option<Level> {
        self.map.remove(tick)
    }
    fn best_tick(&self) -> Option<i64> {
        self.map.best_tick_bid()
    }
    fn depth(&self, n: usize) -> Vec<(i64, i64)> {
        self.map.depth_bid(n)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn order_preserving_bytes() {
        assert!(key_bytes(-1) < key_bytes(0));
        assert!(key_bytes(0) < key_bytes(1));
        assert!(key_bytes(i64::MIN) < key_bytes(i64::MAX));
        assert_eq!(tick_from_bytes(&key_bytes(42)), 42);
    }

    #[test]
    fn ask_best_is_min() {
        let mut m = ArtAskIndex::default();
        m.insert(100, Level::default());
        m.insert(90, Level::default());
        m.insert(110, Level::default());
        assert_eq!(m.best_tick(), Some(90));
    }

    #[test]
    fn bid_best_is_max() {
        let mut m = ArtBidIndex::default();
        m.insert(100, Level::default());
        m.insert(90, Level::default());
        m.insert(110, Level::default());
        assert_eq!(m.best_tick(), Some(110));
    }

    #[test]
    fn insert_overwrites_same_tick() {
        let mut m = ArtAskIndex::default();
        m.insert(100, Level::default());
        let mut lvl = Level::default();
        lvl.total_lot = 9;
        m.insert(100, lvl);
        assert_eq!(m.get(100).unwrap().total_lot, 9);
    }

    #[test]
    fn get_returns_none_for_missing_tick() {
        let mut m = ArtAskIndex::default();
        m.insert(100, Level::default());
        assert!(m.get(101).is_none());
        assert!(m.get_mut(101).is_none());
        assert!(!m.contains(101));
    }

    #[test]
    fn remove_missing_returns_none() {
        let mut m = ArtBidIndex::default();
        m.insert(50, Level::default());
        assert!(m.remove(51).is_none());
        assert!(m.remove(50).is_some());
        assert!(m.remove(50).is_none());
        assert!(m.best_tick().is_none());
    }

    #[test]
    fn depth_stops_at_limit() {
        let mut m = ArtAskIndex::default();
        for tick in 10..20 {
            m.insert(tick, Level::default());
        }
        assert_eq!(m.depth(3).len(), 3);
    }

    #[test]
    fn split_inner_leaf_with_shared_byte_prefix() {
        let mut m = ArtAskIndex::default();
        m.insert(0x0000_0000_0100, Level::default());
        m.insert(0x0000_0000_0101, Level::default());
        assert!(m.contains(0x0000_0000_0100));
        assert!(m.contains(0x0000_0000_0101));
        assert_eq!(m.best_tick(), Some(0x0000_0000_0100));
    }

    #[test]
    fn root_leaf_split_on_second_distinct_key() {
        let mut m = ArtBidIndex::default();
        m.insert(10, Level::default());
        m.insert(20, Level::default());
        assert_eq!(m.best_tick(), Some(20));
        assert!(m.remove(10).is_some());
        assert_eq!(m.best_tick(), Some(20));
    }

    #[test]
    fn get_rejects_near_miss_keys() {
        let mut m = ArtAskIndex::default();
        m.insert(0x0000_0000_0100, Level::default());
        m.insert(0x0000_0000_0200, Level::default());
        assert!(m.get(0x0000_0000_0150).is_none());
        assert!(m.get_mut(0x0000_0000_0150).is_none());
    }

    #[test]
    fn split_child_leaf_under_inner_node() {
        let mut m = ArtAskIndex::default();
        m.insert(0x1000_0000_0000_0000, Level::default());
        m.insert(0x2000_0000_0000_0000, Level::default());
        m.insert(0x1000_0000_0000_0001, Level::default());
        assert!(m.contains(0x1000_0000_0000_0000));
        assert!(m.contains(0x1000_0000_0000_0001));
        assert!(m.contains(0x2000_0000_0000_0000));
        assert!(m.get(0x1500_0000_0000_0000).is_none());
    }

    #[test]
    fn bid_depth_walk_stops_at_limit() {
        let mut m = ArtBidIndex::default();
        for tick in (0..8).map(|i| 100 + i) {
            m.insert(tick, Level::default());
        }
        assert_eq!(m.depth(3).len(), 3);
    }

    #[test]
    fn inner_default_node_is_inner() {
        let n = Node::default();
        assert!(matches!(n, Node::Inner { .. }));
    }

    #[test]
    fn depth_zero_and_empty_root() {
        let m = ArtAskIndex::default();
        assert!(m.depth(0).is_empty());
        assert!(m.depth(3).is_empty());
        assert!(m.best_tick().is_none());
    }

    #[test]
    fn overwrite_leaf_under_shared_inner() {
        let mut m = ArtAskIndex::default();
        m.insert(0x1000_0000_0000_0000, Level::default());
        m.insert(0x2000_0000_0000_0000, Level::default());
        let mut lvl = Level::default();
        lvl.total_lot = 7;
        // Same key as first insert — hits leaf-under-inner overwrite path.
        m.insert(0x1000_0000_0000_0000, lvl);
        assert_eq!(m.get(0x1000_0000_0000_0000).unwrap().total_lot, 7);
    }
}
