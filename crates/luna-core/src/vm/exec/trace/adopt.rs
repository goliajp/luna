//! Installing traces another Vm compiled for code of the same content, in
//! place of recording and compiling one here.

use super::*;

impl Vm {
    /// Where a recording of `proto` at `head_pc` (frame base `base`) would
    /// start: installs the traces the trace compiler has for it instead.
    /// `true` when it installed any; nothing was recorded then.
    #[inline(never)]
    pub(in crate::vm::exec) fn trace_try_adopt(
        &mut self,
        proto: Gc<crate::runtime::function::Proto>,
        head_pc: u32,
        base: usize,
        side_parent: Option<(crate::jit::trace::ParentId, usize)>,
        call_triggered: bool,
    ) -> bool {
        // a loop head's trace brings those of the function's other loops
        let ahead = side_parent.is_none() && !call_triggered && !proto.trace_ahead.borrow().asked;
        let Some((mut adopted, _)) =
            self.trace_adopt_request(proto, head_pc, base, side_parent, call_triggered, ahead)
        else {
            return false;
        };
        if ahead && !adopted.is_empty() {
            proto.trace_ahead.borrow_mut().asked = true;
        }
        // the backend gives those of other heads last
        if let Some(i) = adopted.iter().position(|a| a.ahead) {
            let later = adopted.split_off(i);
            self.trace_hold_ahead(proto, later);
        }
        self.trace_install_adopted(proto, adopted, Vec::new())
    }

    /// What the trace compiler has for a recording of `proto` at `head_pc`
    /// (frame base `base`), taken up into this Vm's code memory, and the
    /// frame's tags it was asked for; `None` when traces are not shared or
    /// the frame is past the stack.
    pub(in crate::vm::exec) fn trace_adopt_request(
        &mut self,
        proto: Gc<crate::runtime::function::Proto>,
        head_pc: u32,
        base: usize,
        side_parent: Option<(crate::jit::trace::ParentId, usize)>,
        call_triggered: bool,
        ahead: bool,
    ) -> Option<(Vec<crate::jit::trace::AdoptedTrace>, Vec<u8>)> {
        if !self.jit.share_traces {
            return None;
        }
        let max_stack = proto.max_stack as usize;
        let regs = self.stack.get(base..base + max_stack)?;
        let entry_tags: Vec<u8> = regs.iter().map(|v| v.unpack().0).collect();
        let version = self.version();
        let opts = crate::jit::trace::CompileOptions {
            internal_loop: side_parent.is_none(),
            pre53: version <= LuaVersion::Lua53,
            aot: false,
            tier: self.jit.trace_tier,
            tier_up_at: self.jit.tier_up_at,
            dialect: Some(version),
        };
        let settings = self.jit.recording_settings();
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
            roots: self.heap.chunk_roots(),
            mm_names: &self.mm_names,
            ahead,
        };
        let adopted = jit.trace_compiler.adopt_traces(jit.storage.as_mut(), &req);
        Some((adopted, entry_tags))
    }

    /// Installs the traces `adopted`, taken up for code of `proto`, each
    /// side trace after its parent; `named`: the traces installed already
    /// that some may be side traces of, by the image each was taken up from.
    /// `true` when it installed any.
    pub(in crate::vm::exec) fn trace_install_adopted(
        &mut self,
        proto: Gc<crate::runtime::function::Proto>,
        adopted: Vec<crate::jit::trace::AdoptedTrace>,
        mut named: Vec<(u64, crate::jit::trace::ParentId)>,
    ) -> bool {
        use crate::jit::trace::{AdoptedParent, ParentId};
        let mut any = false;
        for a in adopted {
            let mut ct = a.trace;
            // a side trace whose parent was not installed is not either
            let side_parent = match a.side_parent {
                None => None,
                Some((AdoptedParent::Asked(p), exit)) => Some((p, exit)),
                Some((AdoptedParent::Image(id), exit)) => {
                    match named.iter().find(|(i, _)| *i == id) {
                        Some(&(_, p)) => Some((p, exit)),
                        None => continue,
                    }
                }
            };
            // no room for what running it needs: not installed
            if self.trace_room(ct.window_size as usize).is_err() {
                self.jit.counters.install_no_room += 1;
                continue;
            }
            any = true;
            self.tally_compiled_trace(&ct);
            let only = crate::jit::send_compat::TArc::get_mut(&mut ct);
            let wired = self.wire_side_trace(only.expect("held by no other yet"), side_parent);
            if side_parent.is_none() {
                let at = proto.traces.borrow().len() as u32;
                let id = ParentId {
                    proto,
                    head_pc: ct.head_pc,
                    root: at,
                    side: 0,
                };
                named.push((a.image, id));
            }
            let ct = super::cache::cache_shared(proto, ct);
            self.trace_hold(wired, ct, proto);
            keep_inlined(proto, a.inlined);
            self.heap.barrier_back(proto);
            self.jit.counters.adopted += 1;
        }
        any
    }
}
