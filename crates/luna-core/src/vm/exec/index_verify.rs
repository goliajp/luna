//! `gc-verify`: the key an interpreter read takes must be live.

use super::*;

impl Vm {
    /// Panics when `key`, a string, was freed: what the dereference that
    /// follows would read is gone. The freed-pointer log is exact under a
    /// quarantining allocator (ASAN).
    pub(super) fn verify_index_key(&self, key: Value) {
        if matches!(key, Value::Str(_)) {
            let h = match key {
                Value::Str(s) => s.as_ptr() as usize,
                _ => unreachable!(),
            };
            if self.heap.recently_freed.contains(&h) {
                let (pc, reg_info) = match self.frames.last() {
                    Some(CallFrame::Lua(f)) => {
                        let pc = f.pc as usize;
                        let inst = f.closure.proto.code.get(pc.wrapping_sub(1));
                        (
                            pc,
                            inst.map(|i| {
                                format!(
                                    "op[pc-1]={:?} a={} b={} c={} base={}",
                                    i.op(),
                                    i.a(),
                                    i.b(),
                                    i.c(),
                                    f.base
                                )
                            })
                            .unwrap_or_default(),
                        )
                    }
                    _ => (0, String::new()),
                };
                let fun = match self.frames.last() {
                    Some(CallFrame::Lua(f)) => {
                        let (c, p) = (
                            f.closure.as_ptr() as usize,
                            f.closure.proto.as_ptr() as usize,
                        );
                        format!(
                            "closure {c:#x} freed {} {}, proto {p:#x} freed {} defined at {}; frames {}",
                            self.heap.recently_freed.contains(&c),
                            self.heap
                                .describe(c as *const crate::runtime::heap::GcHeader),
                            self.heap.recently_freed.contains(&p),
                            f.closure.proto.line_defined,
                            self.frames.len()
                        )
                    }
                    _ => String::new(),
                };
                panic!(
                    "[gc-verify] op_index READ of dead string key {h:#x} {:?} \
                 (gc_top {}, top {}, pc {pc}, {reg_info}; {fun})",
                    crate::runtime::gc_verify_probe::freed_str(h),
                    self.gc_top,
                    self.top,
                );
            }
        }
    }
}
