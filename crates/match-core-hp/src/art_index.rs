//! Byte-wise radix tree for `i64` price ticks (ART-style), feature `art`.
//!
//! Keys are stored in order-preserving 8-byte form: `(tick as u64) ^ (1 << 63)` big-endian.
//! Inner nodes use ordered child maps (sparse radix); leaves hold [`Level`](crate::level::Level).

use crate::level::Level;
use crate::level_index::LevelIndex;
use std::collections::BTreeMap;

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
        for (depth, &b) in bytes.iter().enumerate() {
            match cur {
                Node::Leaf { key, value } => {
                    return if *key == tick { Some(value) } else { None };
                }
                Node::Inner { children } => {
                    cur = children.get(&b).map(|c| c.as_ref())?;
                    if depth == 7 {
                        return match cur {
                            Node::Leaf { key, value } if *key == tick => Some(value),
                            _ => None,
                        };
                    }
                }
            }
        }
        None
    }

    fn get_mut(&mut self, tick: i64) -> Option<&mut Level> {
        let bytes = key_bytes(tick);
        let mut cur = self.root.as_mut()?.as_mut();
        for (depth, &b) in bytes.iter().enumerate() {
            match cur {
                Node::Leaf { key, value } => {
                    return if *key == tick { Some(value) } else { None };
                }
                Node::Inner { children } => {
                    let child = children.get_mut(&b)?;
                    if depth == 7 {
                        return match child.as_mut() {
                            Node::Leaf { key, value } if *key == tick => Some(value),
                            _ => None,
                        };
                    }
                    cur = child.as_mut();
                }
            }
        }
        None
    }

    fn contains(&self, tick: i64) -> bool {
        self.get(tick).is_some()
    }

    fn insert(&mut self, tick: i64, level: Level) {
        let bytes = key_bytes(tick);
        if self.root.is_none() {
            self.root = Some(Box::new(Node::Leaf { key: tick, value: level }));
            return;
        }
        Self::insert_at(self.root.as_mut().unwrap(), &bytes, 0, tick, level);
    }

    fn insert_at(node: &mut Box<Node>, bytes: &[u8; 8], depth: usize, tick: i64, level: Level) {
        match node.as_mut() {
            Node::Leaf {
                key: existing_key,
                value,
            } => {
                if *existing_key == tick {
                    *value = level;
                    return;
                }
                let old_key = *existing_key;
                let old_level = std::mem::take(value);
                let old_bytes = key_bytes(old_key);
                *node = Box::new(Node::Inner {
                    children: BTreeMap::new(),
                });
                Self::insert_at(node, &old_bytes, depth, old_key, old_level);
                Self::insert_at(node, bytes, depth, tick, level);
            }
            Node::Inner { children } => {
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
                // If child is a leaf for a different key that shares this prefix, split below.
                if let Node::Leaf {
                    key: ek,
                    value: ev,
                } = child.as_mut()
                {
                    if *ek != tick {
                        let old_key = *ek;
                        let old_level = std::mem::take(ev);
                        let old_bytes = key_bytes(old_key);
                        *child = Box::new(Node::Inner {
                            children: BTreeMap::new(),
                        });
                        Self::insert_at(child, &old_bytes, depth + 1, old_key, old_level);
                        Self::insert_at(child, bytes, depth + 1, tick, level);
                        return;
                    }
                    *ev = level;
                    return;
                }
                Self::insert_at(child, bytes, depth + 1, tick, level);
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
            Node::Leaf { key, value } => {
                if *key != tick {
                    return None;
                }
                let v = std::mem::take(value);
                Some((v, true))
            }
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
}
