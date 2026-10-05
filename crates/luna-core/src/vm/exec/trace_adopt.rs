//! Installing traces another Vm compiled for code of the same content, in
//! place of recording and compiling one here.

use super::*;

impl Vm {
    /// Where a recording of `proto` at `head_pc` (frame base `base`) would
    /// start: installs the traces the trace compiler has for it instead.
    /// `true` when it installed any; nothing was recorded then.
    #[inline(never)]
    pub(super) fn trace_try_adopt(
        &mut self,
        proto: Gc<crate::runtime::function::Proto>,
        head_pc: u32,
        base: usize,
        side_parent: Option<(Gc<crate::runtime::function::Proto>, u32, usize)>,
        call_triggered: bool,
    ) -> bool {
        if !self.jit.share_traces {
            return false;
        }
        let max_stack = proto.max_stack as usize;
        let entry_tags: Vec<u8> = self
            .stack
            .get(base..base + max_stack)
            .map(|regs| regs.iter().map(|v| v.unpack().0).collect())
            .unwrap_or_default();
        if entry_tags.len() != max_stack {
            return false;
        }
        let version = self.version();
        let opts = crate::jit::trace::CompileOptions {
            internal_loop: side_parent.is_none(),
            pre53: version <= LuaVersion::Lua53,
            aot: false,
            tier: self.jit.trace_tier,
            tier_up_at: self.jit.tier_up_at,
        };
        let settings = self.jit.recording_settings();
        let adopted = {
            let jit = &mut self.jit;
            jit.storage.claim(self.jit_owner_id);
            let req = crate::jit::trace::AdoptRequest {
                proto,
                head_pc,
                entry_tags: &entry_tags,
                side_parent,
                call_triggered,
                opts,
                version,
                settings,
                roots: &self.heap.chunk_roots,
                mm_names: &self.mm_names,
            };
            jit.trace_compiler.adopt_traces(jit.storage.as_mut(), &req)
        };
        if adopted.is_empty() {
            return false;
        }
        for a in adopted {
            let mut ct = a.trace;
            self.tally_compiled_trace(&ct);
            let wired = self.wire_side_trace(&mut ct, a.side_parent);
            let ct = cache_trace(proto, ct);
            hold_side_trace(wired, ct);
            keep_inlined(proto, a.inlined);
            self.jit.counters.adopted += 1;
        }
        true
    }
}
