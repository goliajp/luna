//! Table reads and writes of the interpreter: the `__index` / `__newindex`
//! chains, `#`, and the raw set they end in.

use super::*;

pub(super) mod fast;
mod miss;
mod pushed;
mod raw;
mod set;
#[cfg(feature = "gc-verify")]
mod verify;

impl Vm {
    /// [`Self::op_index`]; `probed` = `t` is a table whose raw `t[key]` the
    /// caller already found nil.
    pub(super) fn op_index_from(
        &mut self,
        t: Value,
        key: Value,
        dst: u32,
        probed: bool,
    ) -> Result<(), LuaError> {
        self.hook_index_key(key);
        match self.index_step_from(t, key, probed)? {
            MmOut::Done(v) => self.stack[dst as usize] = v,
            MmOut::Mm { func, recv } => {
                self.begin_meta_call(func, &[recv, key], MetaAction::Store { dst })?;
            }
            MmOut::CompareSynth { .. } => unreachable!("CompareSynth from index_step"),
        }
        Ok(())
    }

    /// [`Self::op_newindex`]; `probed` = `t` is a table on which the caller's
    /// `try_set_existing` already failed.
    pub(super) fn op_newindex_from(
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
        self.index_value_pushed(t, key, 0)
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
        self.newindex_value_pushed(t, key, v, 0)
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
        self.hook_written_key(key);
        let mut cur = t;
        let mut skip = probed;
        for _ in 0..self.tag_loop_limit() {
            let mm = match cur {
                Value::Table(tb) => {
                    // a read-only table refuses the write whatever its
                    // metatable says, here and at every table further
                    // along the chain (Redis `luaV_settable`)
                    // (the write barrier's one bit test covers the read-only
                    // check too; with nothing stored yet it sends no table
                    // back to gray)
                    if !self.heap.store_barrier(tb, &[]) {
                        return Err(self.readonly_error());
                    }
                    // Single-walk collapse — Table::try_set_existing
                    // fuses the prior `tb.get(key).is_nil()` gate and
                    // `raw_set` walk into one chain traversal when the
                    // key is already present with a non-nil value. The
                    // __newindex chain semantics are preserved by the
                    // identity (slot_nil ⇔ fire_newindex).
                    //
                    // SAFETY: `tb` is `cur`, the table operand or a table
                    // reached through `__newindex`, held by the operand's
                    // register or by the metatable it came from; the borrow
                    // lives for the one `try_set_existing`, which touches only
                    // the table and does not collect
                    if !std::mem::take(&mut skip) && unsafe { tb.as_mut() }.set_existing_raw(key, v)
                    {
                        self.heap.store_barrier(tb, &[v]);
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
}
