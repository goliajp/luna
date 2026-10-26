//! The VM's hooks of the builds only tests use (see `runtime::gc_hooks`):
//! each is empty, or a constant, in the product build. What a hook does
//! lives in the modules only those builds compile (`gc_stress`,
//! `gc::trace_k`, `gc::gc_verify`, `gc_refuse`, `index_verify`,
//! `jit_rt::recording_check`).

use super::*;
#[cfg(feature = "gc-stress")]
use crate::runtime::puc::api::PucCollector;

/// gc-stress: which checkpoints collect; nothing in the product.
#[cfg(feature = "gc-stress")]
pub(in super::super) use super::stress::Stress as StressState;
#[cfg(not(feature = "gc-stress"))]
#[derive(Default)]
pub(in super::super) struct StressState;

impl Vm {
    /// A checkpoint: gc-trace writes it, gc-verify checks the live top
    /// cuts into no running Lua frame, gc-stress collects (true: the
    /// checkpoint is done).
    #[inline(always)]
    #[track_caller]
    pub(in super::super) fn hook_checkpoint(&mut self, live_top: u32) -> bool {
        #[cfg(feature = "gc-trace")]
        self.trace_checkpoint();
        #[cfg(feature = "gc-verify")]
        self.verify_live_top(live_top);
        #[cfg(feature = "gc-stress")]
        if !self.heap.gc_is_stopped() {
            self.stress_checkpoint(live_top);
            return true;
        }
        let _ = live_top;
        false
    }

    /// gc-trace: a checkpoint where no collection may run (a load that
    /// holds), written all the same.
    #[inline(always)]
    #[track_caller]
    pub(in super::super) fn hook_trace_checkpoint(&mut self) {
        #[cfg(feature = "gc-trace")]
        self.trace_checkpoint();
    }

    /// gc-verify: after a collection, every register the collector rooted
    /// holds a live value.
    #[inline(always)]
    pub(in super::super) fn hook_frames_live(&self, ctx: &str) {
        #[cfg(feature = "gc-verify")]
        self.verify_frame_regs_live(ctx);
        let _ = ctx;
    }

    /// gc-verify: the key an interpreter read takes is live.
    #[inline(always)]
    pub(in super::super) fn hook_index_key(&self, key: Value) {
        #[cfg(feature = "gc-verify")]
        self.verify_index_key(key);
        let _ = key;
    }

    /// gc-verify: the key a write takes is live.
    #[inline(always)]
    pub(in super::super) fn hook_written_key(&self, key: Value) {
        #[cfg(feature = "gc-verify")]
        self.verify_written_key(key);
        let _ = key;
    }

    /// gc-verify: a parser table the debug library is shown has the parts
    /// PUC's has (`luna`'s, `puc`: array slots and nodes).
    #[inline(always)]
    pub(in super::super) fn hook_reader_sizes(&self, luna: (u64, u64), puc: (u64, u64)) {
        #[cfg(feature = "gc-verify")]
        assert_eq!(
            luna, puc,
            "[gc-verify] a parser table shown with other parts than PUC's"
        );
        let _ = (luna, puc);
    }

    /// gc-stress, gc-verify: a trace being recorded holds nothing the
    /// collection freed.
    #[inline(always)]
    pub(crate) fn hook_recording_live(&self) {
        #[cfg(any(feature = "gc-stress", feature = "gc-verify"))]
        self.check_recording_live();
    }

    /// Answer a refused growth, if one waits.
    #[inline(always)]
    pub(crate) fn refused_growth(&mut self) -> Result<(), LuaError> {
        #[cfg(feature = "gc-trace")]
        if self.heap.refusal_waits() {
            return self.answer_refusal();
        }
        Ok(())
    }

    /// [`Vm::refused_growth`] after the stack grew from `old` slots for
    /// room above `top`: 5.4 on reallocate it with emergency collections
    /// stopped, so it stays as it was and the growth is a memory error.
    #[inline(always)]
    pub(crate) fn refused_stack(&mut self, old: u32, top: u32) -> Result<(), LuaError> {
        #[cfg(feature = "gc-trace")]
        if self.heap.refusal_waits() {
            return self.answer_refused_stack(old, top);
        }
        let _ = (old, top);
        Ok(())
    }

    /// [`Vm::refused_growth`] after a table operation: a memory error is
    /// the table's.
    #[inline(always)]
    pub(crate) fn refused_table(&mut self) -> Result<(), crate::runtime::table::TableError> {
        self.refused_growth().map_err(|_| {
            crate::runtime::table::TableError::Memory(crate::runtime::mem::Oom(self.heap.mem()))
        })
    }

    /// `f`, its checkpoints shown in the accounting trace as made in a C
    /// function (`in_c`) or in a call hook (`call_hook`) of PUC's.
    pub(in super::super) fn traced_as<R>(
        &mut self,
        in_c: bool,
        call_hook: bool,
        f: impl FnOnce(&mut Vm) -> R,
    ) -> R {
        let was = self.heap.trace_context((in_c, call_hook));
        let r = f(self);
        self.heap.trace_context(was);
        r
    }
}
