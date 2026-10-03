//! Entering a whole-function JIT compile from a call: the cache lookup
//! and the compiled-code invocation.

use super::*;

impl Vm {
    /// Peek/populate the Proto's JIT cache slot, returning
    /// `Some(values)` when the cached native fn is callable for a
    /// zero-arg call. (Non-zero-arg dispatch is handled by
    /// `try_jit_call_op` from inside `begin_call`.)
    pub(super) fn try_jit_call(&mut self, cl: Gc<LuaClosure>) -> Option<Vec<Value>> {
        use crate::runtime::function::JitProtoState;
        if !self.jit.enabled {
            return None;
        }
        let proto = cl.proto;
        if let JitProtoState::Untried = proto.jit.get() {
            self.populate_jit_cache(proto);
        }
        match proto.jit.get() {
            JitProtoState::Compiled {
                entry,
                num_args: 0,
                returns_one,
                arg_float_mask: _,
                arg_table_mask: _,
                ret_is_float,
                ret_is_table,
            } => {
                // SAFETY: the source `*const u8` is a JIT-compiled function entry pointer produced by Cranelift with the target `fn`-pointer signature (IntChunkFn / IntFnN); the JitVmGuard above keeps the JIT_VM TLS slot live across the call.
                let f: crate::jit::IntChunkFn = unsafe { std::mem::transmute(entry) };
                // Install the active Vm + closure
                // for any Rust helper the JIT'd code may call (e.g.
                // `luna_jit_new_table`, `luna_jit_upval_get`) via
                // cranelift `Linkage::Import`. RAII clear on return.
                // Chunks with no upvalue reads don't touch the closure
                // slot, paying nothing.
                // Route through chunk_compiler so
                // the NullJitBackend path stays inert. Raw-ptr arg
                // avoids the &mut self borrow conflict against the
                // shared self.jit.chunk_compiler read.
                let vm_ptr: *mut Vm = self;
                let _jit_vm_guard = self.jit.chunk_compiler.enter(vm_ptr, Some(cl));
                // SAFETY: `f` is the compiled chunk's entry, transmuted above from the entry pointer the backend returned for this proto with this signature; the guard above pins this Vm and `cl` for the helpers the code calls
                let r = unsafe { f() };
                drop(_jit_vm_guard);
                // A JIT helper may have detected a metatable
                // on a table operand and parked a deopt request here.
                // Discard the sentinel value and return None so the caller
                // re-runs the call through the interpreter, which honours
                // __index/__newindex.
                if self.jit.pending_err.take().is_some() {
                    return None;
                }
                Some(if returns_one {
                    let v = if ret_is_float {
                        Value::Float(f64::from_bits(r as u64))
                    } else if ret_is_table {
                        // SAFETY: a chunk compiled with `ret_is_table` returns a table it was passed or allocated through a helper, which the heap manages; nothing collects before the caller stores the value
                        Value::Table(unsafe {
                            crate::runtime::Gc::from_ptr(r as *mut crate::runtime::Table)
                        })
                    } else {
                        Value::Int(r)
                    };
                    vec![v]
                } else {
                    Vec::new()
                })
            }
            // Non-zero-arg Compiled state: call_value's empty-args
            // fast path can't drive it. Op::Call handles those.
            JitProtoState::Compiled { .. } | JitProtoState::Failed | JitProtoState::Untried => None,
        }
    }

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

    /// `Op::Call` JIT fast path. Run inside `begin_call`
    /// before `push_frame`. Returns `true` when the call was handled
    /// in-place (no new Lua frame). Constraints: every arg slot must
    /// be `Value::Int`, the cached arity must match the call site's
    /// `nargs`, the host wanted-count `wanted` is honoured by
    /// `finish_results`. Also bails when a debug hook is armed —
    /// JIT'd code does not fire line / call / return hooks, so any
    /// active hook makes the interpreter the source of truth.
    pub(super) fn try_jit_call_op(
        &mut self,
        cl: Gc<LuaClosure>,
        func_slot: u32,
        nargs: u32,
        wanted: i32,
    ) -> bool {
        use crate::runtime::function::JitProtoState;
        if !self.jit.enabled {
            return false;
        }
        // Any active debug hook means the interpreter has to run the
        // call so the hook gets the expected events.
        if self.hook.func.is_some() || self.hook.rust_func.is_some() {
            return false;
        }
        let proto = cl.proto;
        if let JitProtoState::Untried = proto.jit.get() {
            self.populate_jit_cache(proto);
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
            return false;
        };
        if num_args as u32 != nargs {
            return false;
        }
        // Pack args into i64 bit-patterns per the per-slot expected
        // kind. A Float-typed slot accepts Value::Float verbatim (and on
        // 5.1/5.2 promotes Value::Int(x) via i64 → f64); a Table-typed slot
        // accepts only Value::Table and passes the raw Gc ptr; an
        // Int-typed slot accepts only Value::Int. Any other shape
        // bails to the interpreter so the call's actual dynamics
        // (metamethod dispatch / type-coerce) take over.
        let mut args: [i64; crate::jit::MAX_JIT_ARITY as usize] =
            [0; crate::jit::MAX_JIT_ARITY as usize];
        // From 5.3 an integer is its own subtype: turned into a float for
        // a float-typed parameter, it would come back out (returned,
        // stored, printed) as a float. Only 5.1/5.2, where every number
        // is a float, may convert it.
        let int_as_float = self.version() <= crate::version::LuaVersion::Lua52;
        for i in 0..num_args as usize {
            let v = self.stack[(func_slot + 1) as usize + i];
            let want_float = (arg_float_mask >> i) & 1 == 1;
            let want_table = (arg_table_mask >> i) & 1 == 1;
            args[i] = match (want_table, want_float, v) {
                (true, _, Value::Table(t)) => t.as_ptr() as i64,
                (false, false, Value::Int(x)) => x,
                (false, true, Value::Float(f)) => f.to_bits() as i64,
                (false, true, Value::Int(x)) if int_as_float => (x as f64).to_bits() as i64,
                _ => return false,
            };
        }
        // Vm + closure pin for helpers, routed through chunk_compiler;
        // see the matching guard in `try_jit_call`.
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
        // See matching path in `try_jit_call`. A helper
        // flagged a metatable on a table operand; bail to the interpreter
        // so `push_frame` runs the call from scratch.
        if self.jit.pending_err.take().is_some() {
            return false;
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
        true
    }
}
