//! Table reads and writes of the interpreter: the `__index` / `__newindex`
//! chains, `#`, and the raw set they end in.

use super::*;

impl Vm {
    /// `t[key]` without `__index` (PUC `luaV_fastget`): a raw hit on a table
    /// is the result, since `__index` is consulted only when the raw value is
    /// nil, and a miss on a table without a metatable is nil. `None` leaves
    /// the read to [`Self::index_miss`].
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) fn index_raw(&self, t: Value, key: Value) -> Option<Value> {
        // gc-verify builds keep every read on the probed path
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t {
            let v = match key {
                Value::Str(s) => tb.get_str(s),
                Value::Int(i) => tb.get_int(i),
                k => tb.get(k),
            };
            if !v.is_nil() || tb.metatable().is_none() {
                return Some(v);
            }
        }
        None
    }

    /// A read opcode that [`Self::index_raw`] could not finish: continues the
    /// `__index` chain without repeating the raw probe.
    #[inline(never)]
    pub(super) fn index_miss(&mut self, t: Value, key: Value, dst: u32) -> Result<(), LuaError> {
        #[cfg(not(feature = "gc-verify"))]
        {
            if let Value::Str(s) = key {
                return self.index_str_miss(t, s, dst);
            }
            self.op_index_from(t, key, dst, matches!(t, Value::Table(_)))
        }
        #[cfg(feature = "gc-verify")]
        self.op_index(t, key, dst)
    }

    /// `R[dst] := t[key]` for a string key when `t` is a table whose raw
    /// `t[key]` is nil, or not a table: follows up to four table-valued
    /// `__index` links with the pointer-chain lookup (PUC `luaV_finishget`
    /// with `fasttm`). Anything else restarts the full chain from `t`, so
    /// the loop limit and errors are those of `index_step`.
    #[cfg(not(feature = "gc-verify"))]
    #[inline(never)]
    fn index_str_miss(
        &mut self,
        t: Value,
        key: Gc<crate::runtime::string::LuaStr>,
        dst: u32,
    ) -> Result<(), LuaError> {
        let mut on_table = matches!(t, Value::Table(_));
        let mut mt = self.metatable_of(t);
        for _ in 0..4 {
            let Some(m) = mt else { break };
            match self.fast_tm(m, Mm::Index) {
                Value::Nil if on_table => {
                    self.stack[dst as usize] = Value::Nil;
                    return Ok(());
                }
                Value::Table(next) => {
                    let v = next.get_str(key);
                    mt = next.metatable();
                    if !v.is_nil() || mt.is_none() {
                        self.stack[dst as usize] = v;
                        return Ok(());
                    }
                    on_table = true;
                }
                _ => break,
            }
        }
        self.op_index_from(t, Value::Str(key), dst, matches!(t, Value::Table(_)))
    }

    /// `R[dst] := t[key]` for a VM read opcode, resolving `__index` yieldably.
    #[cfg(feature = "gc-verify")]
    fn op_index(&mut self, t: Value, key: Value, dst: u32) -> Result<(), LuaError> {
        self.op_index_from(t, key, dst, false)
    }

    /// [`Self::op_index`]; `probed` = `t` is a table whose raw `t[key]` the
    /// caller already found nil.
    fn op_index_from(
        &mut self,
        t: Value,
        key: Value,
        dst: u32,
        probed: bool,
    ) -> Result<(), LuaError> {
        // Read-time probe: a collectable key must be live at
        // the moment it is used. O(1) membership test against the
        // freed-pointer log — gc-verify diagnostic builds only; exact
        // under quarantining allocators (ASAN).
        #[cfg(feature = "gc-verify")]
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
                panic!(
                    "[gc-verify] op_index READ of dead string key {h:#x} \
                     (gc_top {}, top {}, pc {pc}, {reg_info})",
                    self.gc_top, self.top,
                );
            }
        }
        match self.index_step_from(t, key, probed)? {
            MmOut::Done(v) => self.stack[dst as usize] = v,
            MmOut::Mm { func, recv } => {
                self.begin_meta_call(func, &[recv, key], MetaAction::Store { dst })?;
            }
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from index_step"),
        }
        Ok(())
    }

    /// `t[key] := v` without `__newindex` (PUC `luaV_fastset`): overwriting a
    /// key that is present with a non-nil value never involves `__newindex`.
    /// `false` leaves the write to [`Self::newindex_miss`], having done nothing.
    #[inline(always)]
    #[cfg_attr(feature = "gc-verify", allow(unused_variables))]
    pub(super) fn newindex_raw(&mut self, t: Value, key: Value, v: Value) -> bool {
        // gc-verify builds keep every write on the probed path
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            && unsafe { tb.as_mut() }.try_set_existing(key, v)
        {
            self.heap
                .barrier_back(tb.as_ptr() as *mut crate::runtime::heap::GcHeader);
            return true;
        }
        false
    }

    /// A write opcode that [`Self::newindex_raw`] could not finish: runs the
    /// `__newindex` chain without repeating that probe.
    #[inline(never)]
    pub(super) fn newindex_miss(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        let probed = cfg!(not(feature = "gc-verify")) && matches!(t, Value::Table(_));
        self.op_newindex_from(t, key, v, probed)
    }

    /// [`Self::op_newindex`]; `probed` = `t` is a table on which the caller's
    /// `try_set_existing` already failed.
    fn op_newindex_from(
        &mut self,
        t: Value,
        key: Value,
        v: Value,
        probed: bool,
    ) -> Result<(), LuaError> {
        match self.newindex_step_from(t, key, v, probed)? {
            MmOut::Done(_) => {}
            MmOut::Mm { func, recv } => {
                self.begin_meta_call(func, &[recv, key, v], MetaAction::Discard)?;
            }
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from newindex_step"),
        }
        Ok(())
    }

    /// Length fast path: a string's byte count or a table's raw border when no
    /// `__len` is present (`Done`); otherwise the `__len` metamethod (`Mm`),
    /// called with the operand twice. Errors for a non-table with no `__len`.
    pub(super) fn len_step(&mut self, v: Value) -> Result<MmOut, LuaError> {
        match v {
            Value::Str(s) => Ok(MmOut::Done(Value::Int(s.len() as i64))),
            Value::Table(t) => {
                // PUC 5.1's `__len` applies to userdata only — `luaV_objlen`
                // there takes the raw border for a table without consulting
                // the metatable, so `#setmetatable({}, {__len = f})` is 0 on
                // 5.1 and 7 on 5.2+. Verified against stock 5.1.5 / 5.2.4.
                if self.version() == crate::version::LuaVersion::Lua51 {
                    return Ok(MmOut::Done(Value::Int(t.len())));
                }
                let mm = self.get_mm(v, Mm::Len);
                if mm.is_nil() {
                    Ok(MmOut::Done(Value::Int(t.len())))
                } else {
                    Ok(MmOut::Mm { func: mm, recv: v })
                }
            }
            _ => {
                let mm = self.get_mm(v, Mm::Len);
                if mm.is_nil() {
                    Err(self.type_err("get length of", v))
                } else {
                    Ok(MmOut::Mm { func: mm, recv: v })
                }
            }
        }
    }

    pub(crate) fn index_value(&mut self, t: Value, key: Value) -> Result<Value, LuaError> {
        match self.index_step(t, key)? {
            MmOut::Done(v) => Ok(v),
            MmOut::Mm { func, recv } => self.call_mm1(func, &[recv, key]),
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from index_step"),
        }
    }

    /// PUC `MAXTAGLOOP`: 100 links of `__index`/`__newindex` in 5.1/5.2,
    /// 2000 from 5.3.
    pub(super) fn tag_loop_limit(&self) -> u32 {
        if self.version <= LuaVersion::Lua52 {
            100
        } else {
            MAX_TAG_LOOP
        }
    }

    /// Resolve `t[key]` through the `__index` chain, stopping at the first raw
    /// hit (`Done`) or function metamethod (`Mm`). Table-valued `__index` links
    /// are followed inline (no yield possible); only a function link can yield.
    pub(super) fn index_step(&mut self, t: Value, key: Value) -> Result<MmOut, LuaError> {
        self.index_step_from(t, key, false)
    }

    /// [`Self::index_step`]; `probed` skips the raw probe of `t` itself.
    fn index_step_from(&mut self, t: Value, key: Value, probed: bool) -> Result<MmOut, LuaError> {
        let mut cur = t;
        let mut skip = probed;
        for _ in 0..self.tag_loop_limit() {
            let mm = match cur {
                Value::Table(tb) => {
                    if !std::mem::take(&mut skip) {
                        let v = match key {
                            Value::Str(s) => tb.get_str(s),
                            k => tb.get(k),
                        };
                        if !v.is_nil() {
                            return Ok(MmOut::Done(v));
                        }
                    }
                    let mm = self.get_mm(cur, Mm::Index);
                    if mm.is_nil() {
                        return Ok(MmOut::Done(Value::Nil));
                    }
                    mm
                }
                v => {
                    let mm = self.get_mm(v, Mm::Index);
                    if mm.is_nil() {
                        return Err(self.type_err("index", v));
                    }
                    mm
                }
            };
            match mm {
                Value::Closure(_) | Value::Native(_) => {
                    return Ok(MmOut::Mm {
                        func: mm,
                        recv: cur,
                    });
                }
                next => cur = next,
            }
        }
        Err(self.runerror(if self.version <= LuaVersion::Lua52 {
            "loop in gettable"
        } else {
            "'__index' chain too long; possible loop"
        }))
    }

    pub(crate) fn newindex_value(
        &mut self,
        t: Value,
        key: Value,
        v: Value,
    ) -> Result<(), LuaError> {
        match self.newindex_step(t, key, v)? {
            MmOut::Done(_) => Ok(()),
            MmOut::Mm { func, recv } => {
                self.call_value(func, &[recv, key, v])?;
                Ok(())
            }
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from newindex_step"),
        }
    }

    /// Resolve `t[key] = v` through the `__newindex` chain. A raw assignment is
    /// performed inline (returning `Done`); only a function metamethod (`Mm`)
    /// needs an actual call — which the caller may run yieldably.
    pub(super) fn newindex_step(
        &mut self,
        t: Value,
        key: Value,
        v: Value,
    ) -> Result<MmOut, LuaError> {
        self.newindex_step_from(t, key, v, false)
    }

    /// [`Self::newindex_step`]; `probed` skips the in-place update attempt
    /// on `t` itself.
    fn newindex_step_from(
        &mut self,
        t: Value,
        key: Value,
        v: Value,
        probed: bool,
    ) -> Result<MmOut, LuaError> {
        // Read-time probe (gc-verify): a dead query key at a
        // WRITE site, attributed to the instruction that produced it.
        #[cfg(feature = "gc-verify")]
        if let Some(p) = match key {
            Value::Str(s) => Some(s.as_ptr() as usize),
            Value::Table(t2) => Some(t2.as_ptr() as usize),
            _ => None,
        } && crate::runtime::gc_verify_probe::is_freed(p)
        {
            let detail = match self.frames.last() {
                Some(CallFrame::Lua(f)) => {
                    let pc = f.pc as usize;
                    let mut w = String::new();
                    for q in pc.saturating_sub(6)..(pc + 2) {
                        if let Some(inst) = f.closure.proto.code.get(q) {
                            w.push_str(&format!(
                                "\n  [{q}] {:?} a={} b={} c={} k={}",
                                inst.op(),
                                inst.a(),
                                inst.b(),
                                inst.c(),
                                inst.k()
                            ));
                        }
                    }
                    format!("pc={pc} base={} gc_top={} window:{w}", f.base, self.gc_top)
                }
                _ => "non-Lua frame".into(),
            };
            panic!("[gc-verify] newindex_step QUERY key {p:#x} freed. {detail}");
        }
        let mut cur = t;
        let mut skip = probed;
        for _ in 0..self.tag_loop_limit() {
            let mm = match cur {
                Value::Table(tb) => {
                    // Single-walk collapse — Table::try_set_existing
                    // fuses the prior `tb.get(key).is_nil()` gate and
                    // `raw_set` walk into one chain traversal when the
                    // key is already present with a non-nil value. The
                    // __newindex chain semantics are preserved by the
                    // identity (slot_nil ⇔ fire_newindex).
                    //
                    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the
                    // heap is single-threaded and the pointer is live as
                    // long as it is reachable from active roots (see
                    // heap.rs:5-7). Mirrors the raw_set wrapper below.
                    if !std::mem::take(&mut skip) && unsafe { tb.as_mut() }.try_set_existing(key, v)
                    {
                        self.heap
                            .barrier_back(tb.as_ptr() as *mut crate::runtime::heap::GcHeader);
                        return Ok(MmOut::Done(Value::Nil));
                    }
                    let mm = self.get_mm(cur, Mm::NewIndex);
                    if mm.is_nil() {
                        self.raw_set(tb, key, v)?;
                        return Ok(MmOut::Done(Value::Nil));
                    }
                    mm
                }
                bad => {
                    let mm = self.get_mm(bad, Mm::NewIndex);
                    if mm.is_nil() {
                        return Err(self.type_err("index", bad));
                    }
                    mm
                }
            };
            match mm {
                Value::Closure(_) | Value::Native(_) => {
                    return Ok(MmOut::Mm {
                        func: mm,
                        recv: cur,
                    });
                }
                next => cur = next,
            }
        }
        Err(self.runerror(if self.version <= LuaVersion::Lua52 {
            "loop in settable"
        } else {
            "'__newindex' chain too long; possible loop"
        }))
    }

    pub(crate) fn raw_set(&mut self, t: Gc<Table>, key: Value, v: Value) -> Result<(), LuaError> {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        match unsafe { t.as_mut() }.set_inlined(&mut self.heap, key, v) {
            Ok(()) => {
                self.heap
                    .barrier_back(t.as_ptr() as *mut crate::runtime::heap::GcHeader);
                Ok(())
            }
            Err(TableError::NilIndex) => Err(self.runerror("table index is nil")),
            Err(TableError::NanIndex) => Err(self.runerror("table index is NaN")),
            Err(TableError::Overflow) => Err(self.runerror("table overflow")),
            Err(TableError::InvalidNext) => unreachable!(),
        }
    }
}
