//! Frames waiting for an emulated network device: the network thread puts
//! them in, and the emulator takes them out between instructions.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Frames a device may have waiting; more are dropped, oldest first, as a
/// card with a full buffer drops them.
pub const LIMIT: usize = 256;

#[derive(Default)]
pub struct PortQueue {
    frames: Mutex<VecDeque<Vec<u8>>>,
    /// Whether frames are waiting, to look at without the lock.
    pending: AtomicBool,
}

impl PortQueue {
    pub fn push(&self, frame: Vec<u8>) {
        let Ok(mut frames) = self.frames.lock() else { return };
        if frames.len() >= LIMIT {
            frames.pop_front();
        }
        frames.push_back(frame);
        self.pending.store(true, Ordering::Release);
    }

    pub fn pop(&self) -> Option<Vec<u8>> {
        if !self.is_pending() {
            return None;
        }
        let mut frames = self.frames.lock().ok()?;
        let frame = frames.pop_front();
        if frames.is_empty() {
            self.pending.store(false, Ordering::Release);
        }
        frame
    }

    pub fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    pub fn clear(&self) {
        if let Ok(mut frames) = self.frames.lock() {
            frames.clear();
        }
        self.pending.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_order_and_drops_the_oldest_when_full() {
        let q = PortQueue::default();
        assert!(!q.is_pending());
        assert_eq!(q.pop(), None);
        for i in 0..LIMIT + 2 {
            q.push(vec![i as u8]);
        }
        assert!(q.is_pending());
        assert_eq!(q.pop(), Some(vec![2]));
        q.clear();
        assert!(!q.is_pending());
        q.push(vec![7]);
        assert_eq!(q.pop(), Some(vec![7]));
        assert!(!q.is_pending());
    }
}
