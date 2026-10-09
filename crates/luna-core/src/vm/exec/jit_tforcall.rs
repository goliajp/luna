//! The trace JIT's generic `for` call.

use super::*;

impl Vm {
    /// Trace JIT helper for a generic `TForCall A 0 C` of any layout:
    /// `nvars` packs C with the layout's registers (`ForLayout::pack_call`);
    /// below, A+4 stands for the first variable and A+2 for the control
    /// (5.4's layout).
    ///
    /// Base path: copy R[A], R[A+1], R[A+2] → R[A+4..=A+6] + `begin_call`.
    /// ipairs `inext` fast path at the top — skip begin_call
    ///     when R[A]=Native(ipairs_iter), R[A+1]=Table no-mt,
    ///     R[A+2]=Int.
    /// Batched out-ptr writeback — fill ctrl/key/val raws into
    ///     caller-provided buffers + return R[A+4]'s tag byte. Lets
    ///     emit skip 3 separate `luna_jit_stack_load` calls and 1
    ///     `luna_jit_stack_tag` call by reading the buffer via
    ///     cranelift `stack_load` IR instead. Returns -1 on deopt,
    ///     else R[A+4]'s tag byte | R[A+5]'s tag byte << 8 (the value's
    ///     tag only when `nvars >= 2`, 0 otherwise).
    #[doc(hidden)]
    pub fn jit_op_tforcall(
        &mut self,
        slot_offset: u32,
        nvars: i32,
        ctrl_out: &mut i64,
        key_out: &mut i64,
        val_out: &mut i64,
    ) -> i64 {
        let Some(f) = self.jit_last_lua_frame() else {
            return -1;
        };
        let (nvars, var, ctl) = crate::vm::isa::ForLayout::unpack_call(nvars);
        let nvars = nvars as i32;
        let abs = f.base + slot_offset;
        let (first, control) = (abs + var, abs + ctl);
        let need = (first + 3) as usize;
        if self.stack.len() < need {
            self.grow_stack_or_abort(need);
        }
        // ipairs fast path
        let took_fast_path = if let Value::Native(n) = self.stack[abs as usize]
            && n.builtin == crate::runtime::Builtin::IpairsIter
            && let Value::Table(t) = self.stack[(abs + 1) as usize]
            && t.metatable().is_none()
            && let Value::Int(i) = self.stack[control as usize]
        {
            let next_i = i.wrapping_add(1);
            let v = t.get_int(next_i);
            if v.is_nil() {
                self.stack[first as usize] = Value::Nil;
            } else {
                self.stack[first as usize] = Value::Int(next_i);
                if (nvars as usize) >= 2 {
                    self.stack[(first + 1) as usize] = v;
                }
                for j in 2..nvars as usize {
                    let slot = first + j as u32;
                    if (slot as usize) < self.stack.len() {
                        self.stack[slot as usize] = Value::Nil;
                    }
                }
            }
            true
        } else {
            false
        };
        if !took_fast_path {
            // slow path: copy R[A..=A+2] → R[A+4..=A+6], then
            // route through begin_call. Lua-closure iters would push
            // a Lua frame mid-trace → deopt.
            self.stack[(first + 2) as usize] = self.stack[control as usize];
            self.stack[(first + 1) as usize] = self.stack[(abs + 1) as usize];
            self.stack[first as usize] = self.stack[abs as usize];
            // the interpreter raises the call's error itself; and a native
            // that `begin_call` hands to the interpreter loop (pcall, xpcall,
            // pairs, an async native) pushes frames or parks a future
            // instead of returning its results here
            let runs_to_completion = match self.stack[abs as usize] {
                Value::Native(nc) => nc.kind == NativeKind::Plain,
                _ => false,
            };
            if !runs_to_completion || self.begin_call(first, Some(2), nvars, false).is_err() {
                self.jit.counters.deopt += 1;
                return -1;
            }
        }
        // Batched writeback — fill the caller's buffers with the
        // raw bits of R[A+2] / R[A+4] / R[A+5] so the trace IR can
        // reload via cranelift `stack_load` instead of separate
        // `luna_jit_stack_load` helper calls.
        // SAFETY: every `RawVal` `unpack` returns has all 8 bytes initialised (`RawVal::NIL` for nil and booleans), so reading them as `zero` is defined
        let ctrl_raw = unsafe { self.stack[control as usize].unpack().1.zero };
        let (key_tag, key_rv) = self.stack[first as usize].unpack();
        // SAFETY: `key_rv` came from `unpack`, whose payload has all 8 bytes initialised
        let key_raw = unsafe { key_rv.zero };
        let (val_tag, val_raw) = if (nvars as usize) >= 2 {
            let (tag, rv) = self.stack[(first + 1) as usize].unpack();
            // SAFETY: `rv` came from `unpack`, whose payload has all 8 bytes initialised
            (tag, unsafe { rv.zero })
        } else {
            (0, 0u64)
        };
        *ctrl_out = ctrl_raw as i64;
        *key_out = key_raw as i64;
        *val_out = val_raw as i64;
        i64::from(key_tag) | i64::from(val_tag) << 8
    }
}

/// The registers a generic-for exit leaves as they are, counted from the
/// head frame (the exit's frame of `proto` starts at register `off`): its
/// TForCall wrote the loop variables to the stack with tags the trace did
/// not compile for.
pub(super) fn keep_tfor_vars(
    proto: &crate::runtime::function::Proto,
    decode_body: u64,
    cont_pc: u32,
    off: usize,
) -> std::ops::Range<usize> {
    if decode_body & crate::jit::trace_types::EXIT_KEEP_TFOR_VARS == 0 {
        return 0..0;
    }
    let call = proto.code[cont_pc as usize - 1];
    let lay = call.op().for_layout().expect("an exit after a TForCall");
    let first = off + (call.a() + lay.var()) as usize;
    first..first + call.c() as usize
}
