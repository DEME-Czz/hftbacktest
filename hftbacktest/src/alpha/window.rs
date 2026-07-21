use std::collections::VecDeque;

use super::LobSnapshot;

pub const WINDOW_SIZE: usize = 100;

#[derive(Debug, Default)]
pub struct LobWindow {
    snapshots: VecDeque<LobSnapshot>,
}

impl LobWindow {
    pub fn new() -> Self {
        Self {
            snapshots: VecDeque::with_capacity(WINDOW_SIZE),
        }
    }

    /// Adds a changed order-book state. Returns false for a duplicate of the latest state.
    pub fn push(&mut self, snapshot: LobSnapshot) -> bool {
        if self.snapshots.back() == Some(&snapshot) {
            return false;
        }
        if self.snapshots.len() == WINDOW_SIZE {
            self.snapshots.pop_front();
        }
        self.snapshots.push_back(snapshot);
        true
    }

    pub fn clear(&mut self) {
        self.snapshots.clear();
    }

    pub fn len(&self) -> usize {
        self.snapshots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
    }

    pub fn is_ready(&self) -> bool {
        self.snapshots.len() == WINDOW_SIZE
    }

    pub fn latest(&self) -> Option<&LobSnapshot> {
        self.snapshots.back()
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = &LobSnapshot> {
        self.snapshots.iter()
    }
}
