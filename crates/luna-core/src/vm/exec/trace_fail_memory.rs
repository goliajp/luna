//! Trace heads whose recordings failed to compile, remembered by the
//! content of their function: a chunk loaded again gets new prototypes,
//! whose own failure counts start at zero, and would otherwise record and
//! fail at the same heads as every earlier load did.

use super::*;

/// Heads remembered, at most; the oldest is forgotten first.
const HEADS: usize = 64;

/// Fresh prototypes that skip a given-up head before one records it
/// again: a later load can enter the loop with other values or take
/// another path through it, which can compile.
pub(super) const RETRY_AFTER: u16 = 32;

struct Head {
    content: [u8; 16],
    pc: u32,
    /// recordings that failed, up to the per-prototype limit
    failed: u8,
    /// fresh prototypes that skipped the head since it was given up
    skipped: u16,
}

/// What one Vm remembers of the heads that failed.
#[derive(Default)]
pub(crate) struct FailMemory {
    heads: Vec<Head>,
}

impl FailMemory {
    fn find(&mut self, content: &[u8; 16], pc: u32) -> Option<usize> {
        self.heads
            .iter()
            .position(|h| h.pc == pc && h.content == *content)
    }

    /// A recording of `proto` at `pc` failed to compile.
    pub(super) fn failed(&mut self, proto: &crate::runtime::function::Proto, pc: u32) {
        let content = proto.stable_hash();
        let i = match self.find(&content, pc) {
            Some(i) => i,
            None => {
                if self.heads.len() == HEADS {
                    self.heads.remove(0);
                }
                self.heads.push(Head {
                    content,
                    pc,
                    failed: 0,
                    skipped: 0,
                });
                self.heads.len() - 1
            }
        };
        let h = &mut self.heads[i];
        h.failed = h.failed.saturating_add(1);
    }

    /// A recording of `proto` at `pc` compiled: the head is forgotten.
    pub(super) fn compiled(&mut self, proto: &crate::runtime::function::Proto, pc: u32) {
        if let Some(i) = self.find(&proto.stable_hash(), pc) {
            self.heads.remove(i);
        }
    }

    /// Whether `proto`, which has not failed at `pc` itself, should skip
    /// recording there because prototypes of the same content gave it up.
    fn skip(&mut self, proto: &crate::runtime::function::Proto, pc: u32) -> bool {
        let Some(i) = self.find(&proto.stable_hash(), pc) else {
            return false;
        };
        let h = &mut self.heads[i];
        if h.failed < MAX_TRACE_COMPILE_FAILURES {
            return false;
        }
        h.skipped += 1;
        if h.skipped < RETRY_AFTER {
            return true;
        }
        // one more try; its failures count again from zero
        h.skipped = 0;
        h.failed = 0;
        false
    }
}

/// Whether recording at `pc` of `proto` is left out because fresh
/// prototypes of the same content keep failing there. A skipped head is
/// given up on `proto` too, so its later back-edges do not ask again.
pub(super) fn trace_head_forgone(
    memory: &mut Option<Box<FailMemory>>,
    proto: Gc<crate::runtime::function::Proto>,
    pc: u32,
) -> bool {
    let Some(memory) = memory else {
        return false;
    };
    if proto
        .trace_compile_failures
        .borrow()
        .iter()
        .any(|f| f.head_pc == pc)
    {
        return false;
    }
    if !memory.skip(&proto, pc) {
        return false;
    }
    give_up_head(proto, pc);
    true
}
