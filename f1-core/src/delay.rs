//! Holds items back until their release time: the lamp follows the TV, which
//! runs seconds behind the live feed.
//!
//! Earliest release first, whatever order items were pushed in (the start
//! lights will push items that are due before ones already queued). Items
//! with the same release time come out in the order they went in.

use std::{
    cmp::{Ordering, Reverse},
    collections::BinaryHeap,
    time::Instant,
};

/// One queued item. Ordered by release time, then by push order, never by
/// the item itself, so `T` needs no `Ord`.
#[derive(Clone, Debug)]
struct Entry<T> {
    release_at: Instant,
    seq: u64,
    item: T,
}

impl<T> Entry<T> {
    fn key(&self) -> (Instant, u64) {
        (self.release_at, self.seq)
    }
}

impl<T> PartialEq for Entry<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key() == other.key()
    }
}

impl<T> Eq for Entry<T> {}

impl<T> PartialOrd for Entry<T> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<T> Ord for Entry<T> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key().cmp(&other.key())
    }
}

#[derive(Clone, Debug)]
pub struct DelayQueue<T> {
    /// `BinaryHeap` gives the largest first; `Reverse` makes that the earliest.
    heap: BinaryHeap<Reverse<Entry<T>>>,
    next_seq: u64,
}

impl<T> Default for DelayQueue<T> {
    fn default() -> Self {
        DelayQueue {
            heap: BinaryHeap::new(),
            next_seq: 0,
        }
    }
}

impl<T> DelayQueue<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, item: T, release_at: Instant) {
        self.heap.push(Reverse(Entry {
            release_at,
            seq: self.next_seq,
            item,
        }));
        self.next_seq += 1;
    }

    /// The earliest item due at `now`, if any. Call in a `while let` loop to
    /// take everything that's due.
    pub fn pop_ready(&mut self, now: Instant) -> Option<T> {
        if self.heap.peek()?.0.release_at > now {
            return None;
        }
        self.heap.pop().map(|Reverse(entry)| entry.item)
    }

    /// When the next item is due.
    pub fn next_release(&self) -> Option<Instant> {
        self.heap.peek().map(|Reverse(entry)| entry.release_at)
    }

    pub fn len(&self) -> usize {
        self.heap.len()
    }

    pub fn is_empty(&self) -> bool {
        self.heap.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Not `Ord`, not even `PartialEq`: the queue mustn't need either.
    #[derive(Debug)]
    struct Plain(&'static str);

    fn drain<T>(q: &mut DelayQueue<T>, now: Instant) -> Vec<T> {
        let mut out = Vec::new();
        while let Some(item) = q.pop_ready(now) {
            out.push(item);
        }
        out
    }

    #[test]
    fn nothing_is_released_early() {
        let t0 = Instant::now();
        let mut q = DelayQueue::new();
        q.push("a", t0 + ms(100));
        assert!(q.pop_ready(t0 + ms(99)).is_none());
        assert_eq!(q.pop_ready(t0 + ms(100)), Some("a"));
    }

    #[test]
    fn zero_delay_is_released_at_once() {
        let t0 = Instant::now();
        let mut q = DelayQueue::new();
        q.push("now", t0);
        assert_eq!(q.pop_ready(t0), Some("now"));
        assert!(q.is_empty());
    }

    #[test]
    fn same_release_time_keeps_push_order() {
        let t0 = Instant::now();
        let mut q = DelayQueue::new();
        for item in ["first", "second", "third", "fourth"] {
            q.push(item, t0 + ms(50));
        }
        assert_eq!(
            drain(&mut q, t0 + ms(50)),
            ["first", "second", "third", "fourth"]
        );
    }

    #[test]
    fn earlier_release_pushed_later_comes_out_first() {
        let t0 = Instant::now();
        let mut q = DelayQueue::new();
        q.push("late", t0 + ms(300));
        q.push("early", t0 + ms(100));
        q.push("middle", t0 + ms(200));
        assert_eq!(drain(&mut q, t0 + ms(1000)), ["early", "middle", "late"]);
    }

    #[test]
    fn only_due_items_come_out() {
        let t0 = Instant::now();
        let mut q = DelayQueue::new();
        q.push(1, t0 + ms(10));
        q.push(2, t0 + ms(20));
        q.push(3, t0 + ms(30));
        assert_eq!(drain(&mut q, t0 + ms(20)), [1, 2]);
        assert_eq!(q.len(), 1);
        assert_eq!(q.next_release(), Some(t0 + ms(30)));
    }

    #[test]
    fn empty_queue_has_nothing_and_no_next_release() {
        let mut q: DelayQueue<u8> = DelayQueue::new();
        assert!(q.pop_ready(Instant::now()).is_none());
        assert_eq!(q.next_release(), None);
    }

    #[test]
    fn items_need_no_ordering_of_their_own() {
        let t0 = Instant::now();
        let mut q = DelayQueue::new();
        q.push(Plain("b"), t0 + ms(2));
        q.push(Plain("a"), t0 + ms(1));
        let names: Vec<&str> = drain(&mut q, t0 + ms(2)).into_iter().map(|p| p.0).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn many_items_with_mixed_times_come_out_sorted_and_stable() {
        let t0 = Instant::now();
        let mut q = DelayQueue::new();
        // (release in ms, label): pushed out of order, with ties.
        let pushed = [
            (30, 'a'),
            (10, 'b'),
            (30, 'c'),
            (20, 'd'),
            (10, 'e'),
            (20, 'f'),
        ];
        for (at, label) in pushed {
            q.push(label, t0 + ms(at));
        }
        let out: String = drain(&mut q, t0 + ms(30)).into_iter().collect();
        assert_eq!(out, "bedfac");
    }
}
