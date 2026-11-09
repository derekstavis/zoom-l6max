//! Bounded single-producer, single-consumer queue for the diagnostic.
//!
//! The caller owns storage and must ensure one producer, one consumer, and a
//! stable allocation for every operation. This does not establish those
//! properties for the target firmware.
use core::sync::atomic::{AtomicU32, Ordering};

pub const CAPACITY: u32 = 64;
pub const HEADER_WORDS: usize = 4;
pub const WORDS_WITH_GENERATIONS: usize = HEADER_WORDS + 2 * CAPACITY as usize;

// The caller supplies a valid, aligned array of atomic words. Standalone mode
// has no generation array; the combined diagnostic includes one.
pub struct Queue<'a> {
    words: &'a [AtomicU32],
    generations: bool,
}

impl<'a> Queue<'a> {
    pub fn new(words: &'a [AtomicU32], generations: bool) -> Self {
        let required = HEADER_WORDS + CAPACITY as usize * (1 + generations as usize);
        assert!(words.len() >= required);
        Self { words, generations }
    }

    /// Returns the new head on success, or the new drop count when full.
    pub fn push(&self, item: u32, generation: impl FnOnce() -> u32) -> Result<u32, u32> {
        let head = self.words[0].load(Ordering::Relaxed);
        let tail = self.words[1].load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= CAPACITY {
            return Err(self.words[2].fetch_add(1, Ordering::Relaxed).wrapping_add(1));
        }
        let index = (head % CAPACITY) as usize;
        if self.generations {
            self.words[HEADER_WORDS + CAPACITY as usize + index].store(generation(), Ordering::Relaxed);
        }
        self.words[HEADER_WORDS + index].store(item, Ordering::Relaxed);
        let next = head.wrapping_add(1);
        self.words[0].store(next, Ordering::Release);
        Ok(next)
    }

    /// Capture a bounded batch; reject impossible indices without consuming.
    pub fn batch(&self) -> Option<(u32, u32)> {
        let head = self.words[0].load(Ordering::Acquire);
        let tail = self.words[1].load(Ordering::Relaxed);
        (head.wrapping_sub(tail) <= CAPACITY).then_some((head, tail))
    }

    pub fn read(&self, tail: u32) -> (u32, u32) {
        let index = (tail % CAPACITY) as usize;
        let item = self.words[HEADER_WORDS + index].load(Ordering::Relaxed);
        let generation = if self.generations {
            self.words[HEADER_WORDS + CAPACITY as usize + index].load(Ordering::Relaxed)
        } else { 0 };
        (item, generation)
    }

    pub fn commit(&self, tail: u32) {
        self.words[1].store(tail, Ordering::Release);
    }
}
