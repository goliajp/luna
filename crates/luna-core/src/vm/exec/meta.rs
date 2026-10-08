//! Metamethod lookup and the continuation plumbing for metamethod calls.

use super::*;

/// Outcome of an index/newindex/comparison fast path: either a directly
/// computed result, or a metamethod (with the receiver it resolved against) the
/// caller must invoke — synchronously (C context) or yieldably (VM opcode).
pub(super) enum MmOut {
    /// index → the looked-up value; newindex → done (raw set performed);
    /// comparison → the boolean result already known
    Done(Value),
    /// a metamethod to call; `recv` is the chain element it was found on (the
    /// extra args — key / value — are supplied by the caller)
    Mm { func: Value, recv: Value },
    /// ≤5.3 `a <= b` synthesised via `not __lt(b, a)` when neither operand
    /// carries `__le` — `op_compare` swaps the args and negates the result.
    /// Lives separate from `Mm` so the synth path can stay yieldable without
    /// every other Mm caller learning a swap flag they would never set.
    CompareSynth { func: Value },
}

/// Metamethod events; discriminants index `Vm::mm_names`.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
pub(crate) enum Mm {
    Index,
    NewIndex,
    Call,
    ToString,
    Metatable,
    Name,
    Eq,
    Lt,
    Le,
    Concat,
    Len,
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Pow,
    IDiv,
    BAnd,
    BOr,
    BXor,
    Shl,
    Shr,
    Unm,
    BNot,
    Close,
    Gc,
    Pairs,
}

// one absent bit per event in `Table::flags`, below the read-only bit
const _: () = assert!(MM_NAMES.len() <= 31);

pub(super) const MM_NAMES: [&str; 28] = [
    "__index",
    "__newindex",
    "__call",
    "__tostring",
    "__metatable",
    "__name",
    "__eq",
    "__lt",
    "__le",
    "__concat",
    "__len",
    "__add",
    "__sub",
    "__mul",
    "__div",
    "__mod",
    "__pow",
    "__idiv",
    "__band",
    "__bor",
    "__bxor",
    "__shl",
    "__shr",
    "__unm",
    "__bnot",
    "__close",
    "__gc",
    "__pairs",
];

/// The metamethod event an opcode dispatches, without the `__` prefix (PUC
/// funcnamefromcode), for "(metamethod 'event')" call-error suffixes.
pub(super) fn mm_event_name(op: crate::vm::isa::Op) -> Option<&'static str> {
    use crate::vm::isa::Op;
    Some(match op {
        Op::Add => "add",
        Op::Sub => "sub",
        Op::Mul => "mul",
        Op::Div => "div",
        Op::Mod => "mod",
        Op::Pow => "pow",
        Op::IDiv => "idiv",
        Op::BAnd => "band",
        Op::BOr => "bor",
        Op::BXor => "bxor",
        Op::Shl => "shl",
        Op::Shr => "shr",
        Op::Unm => "unm",
        Op::BNot => "bnot",
        Op::Concat => "concat",
        Op::Len => "len",
        Op::GetField | Op::GetTable | Op::GetTableK | Op::GetTabUpR | Op::GetI | Op::SelfOp => {
            "index"
        }
        Op::SetField | Op::SetTable | Op::SetTableK | Op::SetTabUpR | Op::SetTabUpK | Op::SetI => {
            "newindex"
        }
        Op::Eq | Op::EqK => "eq",
        Op::Lt => "lt",
        Op::Le => "le",
        _ => return None,
    })
}

impl Vm {
    /// Call a metamethod with a single expected result.
    pub(super) fn call_mm1(&mut self, f: Value, args: &[Value]) -> Result<Value, LuaError> {
        let mut r = self.call_value(f, args)?;
        Ok(if r.is_empty() {
            Value::Nil
        } else {
            r.swap_remove(0)
        })
    }

    /// The level a call made while a message handler runs fails at with
    /// "error in error handling" (see `ERRERR_C_DEPTH`).
    pub(crate) fn errerr_c_depth(&self) -> u32 {
        if self.version >= LuaVersion::Lua54 {
            ERRERR_C_DEPTH
        } else {
            ERRERR_C_DEPTH_PRE54
        }
    }

    /// PUC's check before a call that takes a C level (5.4
    /// `luaE_checkcstack`, 5.1 `luaD_call`): the call that would run at
    /// `MAX_C_DEPTH` fails with "C stack overflow", positioned when Lua
    /// code made it; a message handler running on that error gets a tenth
    /// more levels before "error in error handling". Whoever passes the
    /// check takes the level (`c_depth` or `pcall_depth`).
    #[inline(always)]
    pub(crate) fn check_c_level(&mut self, positioned: bool) -> Result<(), LuaError> {
        if self.g.nccalls + 1 < MAX_C_DEPTH {
            return Ok(());
        }
        self.c_level_overflow(positioned)
    }

    /// [`Vm::check_c_level`] at the limit.
    #[cold]
    #[inline(never)]
    fn c_level_overflow(&mut self, positioned: bool) -> Result<(), LuaError> {
        let next = self.g.nccalls + 1;
        if next == MAX_C_DEPTH || self.msgh_depth == 0 {
            let e = if positioned {
                self.runerror("C stack overflow")
            } else {
                self.plain_err("C stack overflow")
            };
            // the refused call keeps its level while its handler runs
            self.c_overflow_err = Some(e.0);
            return Err(e);
        }
        if next >= self.errerr_c_depth() {
            return Err(LuaError(self.errerr()));
        }
        Ok(())
    }

    /// Count a metamethod, `__pairs` or `__close` call as PUC counts the C
    /// call it makes. The caller pushes the continuation that holds the
    /// level.
    pub(super) fn enter_c_level(&mut self, positioned: bool) -> Result<(), LuaError> {
        self.check_c_level(positioned)?;
        self.g.nccalls += 1;
        self.g.meta_conts += 1;
        Ok(())
    }

    /// Begin a *yieldable* metamethod call from a VM instruction: `func(args…)`
    /// driven through the interpreter loop with a `Meta` continuation, so a
    /// `coroutine.yield` inside the metamethod suspends and resumes cleanly.
    /// On the metamethod's return the loop head runs `finish_meta(action, …)`.
    /// Returns to the caller with the call set up — the opcode arm must do no
    /// further work on the running frame and let the loop iterate. `tm` is
    /// the metamethod event name (e.g. "index", "add"); a Lua handler frame
    /// born from this call inherits it via `pending_tm`, so
    /// `debug.getinfo(1).namewhat == "metamethod"` and `.name == tm`
    /// (db.lua :878).
    pub(super) fn begin_meta_call(
        &mut self,
        func: Value,
        args: &[Value],
        action: MetaAction,
    ) -> Result<(), LuaError> {
        self.enter_c_level(true)?;
        let saved_top = self.top;
        // PUC calls it at `L->top`: the frame's whole window, or for a
        // concatenation the top of the operands left
        let cont_slot = match action {
            MetaAction::Concat { .. } => self.top,
            _ => self.lua_window_end(),
        };
        self.place_call(cont_slot, func, args);
        self.top = cont_slot + 1 + args.len() as u32;
        frames_push_sync(
            &mut self.frames,
            &mut self.frames_top,
            &mut self.trap,
            CallFrame::Cont(NativeCont {
                kind: ContKind::Meta(MetaCont { action, saved_top }),
                func_slot: cont_slot,
                nresults: 1,
            }),
        );
        let saved_tm = self
            .pending_tm
            .replace(crate::runtime::function::FrameTm::Meta);
        // begin_call drives a Lua metamethod through the loop (returns true) or
        // runs a native one inline (returns false, leaving results at cont_slot
        // for the loop head to pick up); either way the Meta cont resolves there.
        let r = self.begin_call(cont_slot, Some(args.len() as u32), 1, true);
        // Native callees never consumed pending_tm (push_frame is only hit on
        // a Lua callee); restore so it doesn't leak to a later push_frame.
        self.pending_tm = saved_tm;
        r?;
        Ok(())
    }

    /// Apply a comparison opcode's outcome: a known boolean drives the
    /// conditional skip directly; a metamethod is called yieldably, its
    /// truthiness driving the skip on return.
    pub(super) fn op_compare(
        &mut self,
        step: MmOut,
        l: Value,
        r: Value,
        k: bool,
    ) -> Result<(), LuaError> {
        match step {
            MmOut::Done(v) => self.cond_skip(v.truthy(), k),
            MmOut::Mm { func, .. } => {
                self.begin_meta_call(func, &[l, r], MetaAction::Compare { k, negate: false })?;
            }
            MmOut::CompareSynth { func } => {
                // ≤5.3 `__le` falls back to `not __lt(r, l)`; the swap and
                // negation are driven through `MetaAction::Compare` so the
                // metamethod call can yield like any other compare.
                self.begin_meta_call(func, &[r, l], MetaAction::Compare { k, negate: true })?;
            }
        }
        Ok(())
    }

    /// Complete a VM instruction whose metamethod just returned `result` (PUC
    /// `luaV_finishOp`). The running frame is already back on top.
    pub(super) fn finish_meta(
        &mut self,
        action: MetaAction,
        result: Value,
    ) -> Result<(), LuaError> {
        match action {
            MetaAction::Store { dst } => self.stack[dst as usize] = result,
            MetaAction::Discard => {}
            MetaAction::Compare { k, negate } => {
                let t = if negate {
                    !result.truthy()
                } else {
                    result.truthy()
                };
                self.cond_skip(t, k);
            }
            MetaAction::Concat { dst, base_a } => {
                self.stack[dst as usize] = result;
                self.top = dst + 1;
                self.concat_run(base_a)?;
            }
        }
        Ok(())
    }

    // ---- metatables ----

    pub(crate) fn metatable_of(&self, v: Value) -> Option<Gc<Table>> {
        match v {
            Value::Table(t) => t.metatable(),
            Value::Userdata(u) => u.metatable(),
            v => type_mt_slot(v).and_then(|i| self.type_mt[i]),
        }
    }

    /// Set the shared metatable for `v`'s basic type (debug.setmetatable on a
    /// non-table). No-op for tables (they carry their own).
    pub(crate) fn set_type_metatable(&mut self, v: Value, mt: Option<Gc<Table>>) {
        if let Some(i) = type_mt_slot(v) {
            self.type_mt[i] = mt;
        }
    }

    /// The metamethod of `v` for `mm`, or nil.
    pub(crate) fn get_mm(&self, v: Value, mm: Mm) -> Value {
        match self.metatable_of(v) {
            Some(mt) => self.fast_tm(mt, mm),
            None => Value::Nil,
        }
    }

    /// `mt[mm]` through the table's absent-metamethod bits (PUC
    /// `fasttm`): a miss sets the event's bit, and any key the table gains
    /// clears them all.
    #[inline]
    pub(crate) fn fast_tm(&self, mt: Gc<Table>, mm: Mm) -> Value {
        let bit = 1u32 << mm as u32;
        if mt.absent_mm() & bit != 0 {
            return Value::Nil;
        }
        let v = mt.get_str(self.mm_names[mm as usize]);
        if v.is_nil() {
            Table::note_absent_mm(mt, bit);
        }
        v
    }

    /// PUC 5.1 `get_compTM`: a comparison metamethod (`__eq` / `__lt` / `__le`)
    /// only fires when both operands carry a metatable that exposes the same
    /// implementation. Returns the metamethod to call, or `Nil` when no
    /// compatible match exists. Used to honour events.lua 5.1 :262's rule
    /// that `c == d` (where `d` has no metatable) falls back to raw equality.
    pub(crate) fn get_comp_mm(&self, l: Value, r: Value, mm: Mm) -> Value {
        let mt1 = self.metatable_of(l);
        let Some(mt1) = mt1 else { return Value::Nil };
        let tm1 = self.fast_tm(mt1, mm);
        if tm1.is_nil() {
            return Value::Nil;
        }
        let mt2 = self.metatable_of(r);
        let Some(mt2) = mt2 else { return Value::Nil };
        if mt1.as_ptr() == mt2.as_ptr() {
            return tm1;
        }
        let tm2 = self.fast_tm(mt2, mm);
        if tm2.is_nil() {
            return Value::Nil;
        }
        if tm1.raw_eq(tm2) {
            return tm1;
        }
        Value::Nil
    }

    /// PUC `luaT_objtypename`: the type name shown in error messages. A table
    /// or full userdata whose metatable carries a string `__name` reports that
    /// (e.g. "FILE*", "My Type") instead of the bare "table"/"userdata".
    pub(crate) fn obj_typename(&self, v: Value) -> String {
        // `__name` (luaT_objtypename) arrived in 5.3
        if self.version >= LuaVersion::Lua53
            && matches!(v, Value::Table(_) | Value::Userdata(_))
            && let Value::Str(s) = self.get_mm(v, Mm::Name)
        {
            return String::from_utf8_lossy(s.as_bytes()).into_owned();
        }
        v.type_name().to_string()
    }

    pub(super) fn call_at(
        &mut self,
        func_slot: u32,
        nargs: u32,
        from_c: bool,
    ) -> Result<Vec<Value>, LuaError> {
        let depth = self.frames.len();
        match self.begin_call(func_slot, Some(nargs), -1, from_c) {
            // run until every frame the call pushed has returned: a pcall /
            // xpcall / __pairs continuation sits below the frame of the
            // function it called, and it is the continuation that produces
            // the call's results (`true, ...`, or `false, msg` on an error)
            Ok(true) => self.exec_with(depth + 1),
            // native completed inline; results at func_slot..top
            Ok(false) => Ok(self.take_results(func_slot)),
            // pcall / xpcall pushed their continuation and then failed to
            // call their function (`pcall("x")`): the continuation catches
            // that error, as it does in the dispatch loop
            Err(e)
                if self.frames.len() > depth
                    && self.yielding.is_none()
                    && self.terminating.is_none()
                    && !self.host_yield_pending
                    && self.pending_async_native_fut.is_none() =>
            {
                match self.unwind(e.0, depth + 1) {
                    Unwound::Caught => self.exec_with(depth + 1),
                    Unwound::CaughtReturn(vals) => Ok(vals),
                    Unwound::Propagated(err) => Err(err),
                }
            }
            Err(e) => Err(e),
        }
    }
}
