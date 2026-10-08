//! The `gc-verify` check of the rooted registers after a collection.

use super::*;

impl Vm {
    /// `gc-verify`: after a collect, every register slot the
    /// collector just rooted (`[0, max(gc_top, top))` — the same bound
    /// `gc_roots` uses) must hold a live value. A dead value inside the
    /// rooted range means the root snapshot and the sweep disagreed —
    /// a use-after-free waiting to happen. (Slots ABOVE the bound may hold
    /// stale dead values legitimately; the interpreter's contract is
    /// that it writes them before reading.)
    #[cfg(feature = "gc-verify")]
    pub(crate) fn verify_frame_regs_live(&self, ctx: &str) {
        let live = self.heap.debug_live_set();
        let header = |v: Value| -> Option<usize> {
            match v {
                Value::Str(s) => Some(s.as_ptr() as usize),
                Value::Table(t) => Some(t.as_ptr() as usize),
                Value::Closure(c) => Some(c.as_ptr() as usize),
                Value::Native(n) => Some(n.as_ptr() as usize),
                Value::Coro(c) => Some(c.as_ptr() as usize),
                Value::Userdata(u) => Some(u.as_ptr() as usize),
                _ => None,
            }
        };
        let bound = (self.gc_top as usize).min(self.stack.len());
        for i in 0..bound {
            if let Some(h) = header(self.stack[i])
                && !live.contains(&h)
            {
                panic!(
                    "[gc-verify] {ctx}: rooted stack slot {i} (gc_top {}, top {}) \
                         holds a dead value {h:#x} after collect",
                    self.gc_top, self.top,
                );
            }
        }
        // Diagnostic tier: a dead value ABOVE the cursor is only a bug if
        // that register is a named local still in scope (the interpreter
        // WILL read it). Cross-check against the proto's LocVar table.
        for (fi, cf) in self.frames.iter().enumerate() {
            if let CallFrame::Lua(f) = cf {
                let base = f.base as usize;
                let maxs = f.closure.proto.max_stack as usize;
                let hi = (base + maxs).min(self.stack.len());
                let pc = f.pc;
                for i in bound.max(base)..hi {
                    if let Some(h) = header(self.stack[i])
                        && !live.contains(&h)
                    {
                        let reg = (i - base) as u32;
                        if let Some(lv) = f
                            .closure
                            .proto
                            .locvars
                            .iter()
                            .find(|lv| lv.reg == reg && lv.start_pc <= pc && pc < lv.end_pc)
                        {
                            panic!(
                                "[gc-verify] {ctx}: frame {fi} IN-SCOPE LOCAL '{}' \
                                     (reg {reg}, abs {i}, pc {pc}, gc_top {}) holds a \
                                     dead value {h:#x} — live_top cursor excluded a \
                                     live named local",
                                lv.name, self.gc_top,
                            );
                        }
                    }
                }
            }
        }
    }
}
