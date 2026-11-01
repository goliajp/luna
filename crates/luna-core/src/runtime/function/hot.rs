//! What an embedder can read of a prototype's JIT counters.

use super::Proto;

impl Proto {
    /// Backward jumps the interpreter took within this function while the
    /// trace JIT was on, up to `u32::MAX / 2`: the count that makes a loop
    /// a trace head.
    pub fn trace_hot_count(&self) -> u32 {
        self.trace_hot_count.get()
    }

    /// Calls into this function while the trace JIT was on, up to
    /// `u32::MAX / 2`: the count that makes its entry a trace head.
    pub fn call_hot_count(&self) -> u32 {
        self.call_hot_count.get()
    }
}
