//! `gc-stress`: a full collection at every checkpoint, or, for a program
//! too slow for that, at every `n`th one.

use super::*;

/// Which checkpoints collect: the `every`th from the `offset`th on
/// (counted from 0), each one's number kept when asked for.
#[derive(Debug)]
pub(crate) struct Stress {
    every: u64,
    /// The checkpoint the next collection is at.
    next: u64,
    /// Checkpoints passed with the collector running.
    seen: u64,
    log: Option<Vec<u64>>,
}

impl Default for Stress {
    fn default() -> Self {
        Stress {
            every: 1,
            next: 0,
            seen: 0,
            log: None,
        }
    }
}

impl Vm {
    /// From the next checkpoint on, collect at checkpoints `offset`,
    /// `offset + every`, `offset + 2 * every`, … (counted from 0 there)
    /// instead of at each one: the `every` offsets `0..every` together
    /// collect at every checkpoint once.
    pub(crate) fn gc_stress_interval(&mut self, every: u64, offset: u64) {
        assert!(offset < every, "an offset within the interval");
        self.stress.every = every;
        self.stress.seen = 0;
        self.stress.next = offset;
    }

    /// Start keeping the numbers of the checkpoints that collect, counted
    /// as [`Vm::gc_stress_interval`] counts them.
    pub(crate) fn gc_stress_log(&mut self) {
        self.stress.log = Some(Vec::new());
    }

    /// The numbers [`Vm::gc_stress_log`] kept.
    pub(crate) fn gc_stress_collected(&self) -> &[u64] {
        self.stress.log.as_deref().unwrap_or_default()
    }

    /// Changes when a checkpoint collects.
    pub(in super::super) fn stress_mark(&self) -> u64 {
        self.stress.next
    }

    /// The checkpoint at `live_top` with the collector running.
    pub(in super::super) fn stress_checkpoint(&mut self, live_top: u32) {
        self.heap.safe_point();
        let n = self.stress.seen;
        self.stress.seen += 1;
        if n == self.stress.next {
            self.stress.next += self.stress.every;
            if let Some(log) = &mut self.stress.log {
                log.push(n);
            }
            self.gc_top = live_top;
            self.collect_garbage();
        }
    }
}
