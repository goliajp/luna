//! Entering a whole-function JIT compile from a call: the cache lookup
//! and the compiled-code invocation.

use super::*;

mod next;
use next::take_jit_next;

impl Vm {
    /// Populate the cache slot. Flips `Untried` to either
    /// `Compiled { … }` or `Failed`; idempotent on already-populated
    /// states (call sites guard with a get before invoking).
    ///
    /// Consults a thread-local cross-`Vm` cache keyed by a hash of
    /// `proto.code`. Compiled artefacts live in the thread-local
    /// `JITModule` so their mmap pages outlive the `Vm`; subsequent
    /// `Vm`s loading the same source skip the cranelift compile step
    /// entirely.
    pub(super) fn populate_jit_cache(&mut self, proto: Gc<crate::runtime::function::Proto>) {
        use crate::runtime::function::JitProtoState;
        let version = self.version();
        let pre53 = version <= crate::version::LuaVersion::Lua53;
        // 5.1 and 5.2 have no Int subtype (all numbers
        // are Float). The JIT's `GetUpval` ValueRead path uses this
        // to default-pin upvalue reads to Float without a tag check.
        let float_only = version <= crate::version::LuaVersion::Lua52;
        // Split-borrow JitState so the
        // trait method can take `&mut dyn JitStorage` without
        // double-borrowing self.jit.
        let jit = &mut self.jit;
        jit.storage.claim(self.jit_owner_id);
        let storage: &mut dyn crate::jit::JitStorage = jit.storage.as_mut();
        match jit
            .chunk_compiler
            .try_compile(storage, proto, pre53, float_only)
        {
            crate::jit::CompileResult::Compiled {
                entry,
                num_args,
                returns_one,
                arg_float_mask,
                arg_table_mask,
                ret_is_float,
                ret_is_table,
            } => {
                self.jit.counters.method_compiled += 1;
                proto.jit.set(JitProtoState::Compiled {
                    entry,
                    num_args,
                    returns_one,
                    arg_float_mask,
                    arg_table_mask,
                    ret_is_float,
                    ret_is_table,
                });
            }
            crate::jit::CompileResult::Skipped => {
                proto.jit.set(JitProtoState::Failed);
            }
        }
    }

    /// The JIT fast path of a call. Run inside `begin_call`
    /// before `push_frame`. Returns `true` when the call was handled
    /// in-place (no new Lua frame). Constraints: every arg slot must
    /// be `Value::Int`, the cached arity must match the call site's
    /// `nargs`, the host wanted-count `wanted` is honoured by
    /// `finish_results`. Also bails when a debug hook is armed — JIT'd
    /// code fires no line / call / return hooks, so the interpreter is the
    /// source of truth (an armed budget or cap keeps `jit.gate` off).
    pub(super) fn try_jit_call_op(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        wanted: i32,
    ) -> JitRun {
        use crate::runtime::function::JitProtoState;
        // a debug hook gets its events from the interpreter (compiled
        // self-recursion checks the native stack itself)
        if !self.jit.gate || self.hook.func.is_some() || self.hook.rust_func.is_some() {
            return JitRun::NotRun;
        }
        let proto = cl.proto;
        if let JitProtoState::Untried = proto.jit.get() {
            if native_stack::is_low(native_stack::COMPILE_RESERVE) {
                return JitRun::NotRun;
            }
            self.populate_jit_cache(proto);
        }
        if take_jit_next(&proto) {
            self.jit.counters.method_replaced += 1;
        }
        let JitProtoState::Compiled {
            entry,
            num_args,
            returns_one,
            arg_float_mask,
            arg_table_mask,
            ret_is_float,
            ret_is_table,
        } = proto.jit.get()
        else {
            return JitRun::NotRun;
        };
        if num_args as u32 != nargs {
            return JitRun::NotRun;
        }
        // each argument as the i64 its slot's kind takes (a float, an int,
        // a table's pointer), any other shape left to the interpreter; a
        // read-only table too, as compiled code stores without testing (and
        // runs nothing that could mark one)
        let mut args: [i64; crate::jit::MAX_JIT_ARITY as usize] =
            [0; crate::jit::MAX_JIT_ARITY as usize];
        // only 5.1/5.2 (every number a float) may pass an integer as a
        // float: from 5.3 it would come back out as one
        let int_as_float = self.version() <= crate::version::LuaVersion::Lua52;
        for i in 0..num_args as usize {
            let v = self.stack[(func_slot + 1) as usize + i];
            let want_float = (arg_float_mask >> i) & 1 == 1;
            let want_table = (arg_table_mask >> i) & 1 == 1;
            args[i] = match (want_table, want_float, v) {
                (true, _, Value::Table(t)) if !t.is_readonly() => t.as_ptr() as i64,
                (false, false, Value::Int(x)) => x,
                (false, true, Value::Float(f)) => f.to_bits() as i64,
                (false, true, Value::Int(x)) if int_as_float => (x as f64).to_bits() as i64,
                _ => return JitRun::NotRun,
            };
        }
        // the call's level, as `push_frame` counts it (an error is the call's)
        let levels = match self.jit_enter_levels(cl, func_slot, nargs) {
            Ok(Some(l)) => l,
            // the interpreter makes the call: its check finds a step due
            Ok(None) => return JitRun::NotRun,
            Err(e) => {
                self.jit.pending_raise = Some(e);
                return JitRun::Done;
            }
        };
        // the Vm and closure the helpers work on, until the guard drops (a
        // raw pointer: the compiler borrows `self` meanwhile)
        let vm_ptr: *mut Vm = self;
        let _jit_vm_guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
        // SAFETY: the source `*const u8` is a JIT-compiled function entry pointer produced by Cranelift with the target `fn`-pointer signature (IntChunkFn / IntFnN); the JitVmGuard above keeps the JIT_VM TLS slot live across the call.
        let r = unsafe {
            match num_args {
                0 => (std::mem::transmute::<*const u8, crate::jit::IntChunkFn>(entry))(),
                1 => (std::mem::transmute::<*const u8, crate::jit::IntFn1>(entry))(args[0]),
                2 => {
                    (std::mem::transmute::<*const u8, crate::jit::IntFn2>(entry))(args[0], args[1])
                }
                3 => (std::mem::transmute::<*const u8, crate::jit::IntFn3>(entry))(
                    args[0], args[1], args[2],
                ),
                4 => (std::mem::transmute::<*const u8, crate::jit::IntFn4>(entry))(
                    args[0], args[1], args[2], args[3],
                ),
                _ => unreachable!("MAX_JIT_ARITY enforces num_args <= 4"),
            }
        };
        drop(_jit_vm_guard);
        self.jit_leave_levels(levels);
        // the caller raises it
        if self.jit.pending_raise.is_some() {
            return JitRun::Done;
        }
        // a helper met a metatable: the interpreter runs the call afresh
        if self.jit.pending_err.take().is_some() {
            self.jit.handoff.clear();
            return JitRun::NotRun;
        }
        if !self.jit.handoff.is_empty() {
            return JitRun::Handed;
        }
        // Write result at func_slot, replacing the closure value, then
        // hand to finish_results to pad/truncate per the call site's
        // `wanted` count.
        if returns_one {
            let v = if ret_is_float {
                Value::Float(f64::from_bits(r as u64))
            } else if ret_is_table {
                // SAFETY: a chunk compiled with `ret_is_table` returns a table it was passed or allocated through a helper, which the heap manages; it is stored on the stack before anything can collect
                Value::Table(unsafe {
                    crate::runtime::Gc::from_ptr(r as *mut crate::runtime::Table)
                })
            } else {
                Value::Int(r)
            };
            self.stack[func_slot as usize] = v;
            self.finish_results(func_slot, 1, wanted);
        } else {
            self.finish_results(func_slot, 0, wanted);
        }
        JitRun::Done
    }

    /// "stack overflow" raised for a self call of compiled `cl`,
    /// positioned, as the interpreter would raise it, at that call in `cl`.
    pub(crate) fn jit_depth_error(
        &mut self,
        cl: crate::runtime::Gc<crate::runtime::LuaClosure>,
    ) -> LuaError {
        let proto = cl.proto;
        let call = proto
            .code
            .iter()
            .position(|i| i.op() == crate::vm::isa::Op::Call);
        let prefix = self.prefix_at(proto, call.map_or(0, |pc| pc + 1));
        // no emergency collection: the compiled frames below may hold
        // objects only in machine registers
        match self
            .heap
            .try_intern(format!("{prefix}stack overflow").as_bytes())
        {
            Ok(s) => LuaError(Value::Str(s)),
            Err(o) => o.into(),
        }
    }
}
