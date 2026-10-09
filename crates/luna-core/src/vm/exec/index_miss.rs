//! The rest of a table read or write the fast loop's probe could not
//! finish: the `__index` / `__newindex` chains, entered with the operands
//! worked out again from the instruction.

// gc-verify builds take every read and write through the probed paths
#![cfg_attr(feature = "gc-verify", allow(dead_code, unused_imports))]

use super::*;
use crate::runtime::string::LuaStr;
use crate::runtime::value::tag;
use fast_arith::{raw_gc, raw_tag};

impl Vm {
    /// The rest of a `GetI` or `GetTabUp` read the fast loop's probe could
    /// not finish: the operands are decoded again from `inst` and the frame
    /// here (PUC `luaV_finishget` after `luaV_fastget`), so that none of
    /// them stays live across the probe's own out-of-line calls in the loop,
    /// where they would push the loop's state out of registers. The other
    /// reads go on through [`Self::index_miss_at`] from their arm.
    ///
    /// # Safety
    /// `fr` points at the running frame, and `regs` / `kptr` at its
    /// register window and its proto's constants.
    #[inline(never)]
    pub(super) unsafe fn index_op_miss(
        &mut self,
        inst: Inst,
        regs: *const Value,
        kptr: *const Value,
        fr: *const Frame,
    ) -> Result<(), LuaError> {
        // SAFETY: the caller's contract; the compiler and the verifier keep
        // the operands inside the register window and the constants
        let (t, key, dst) = unsafe {
            let dst = (*fr).base + inst.a();
            match inst.op() {
                Op::GetI => (
                    *regs.add(inst.b() as usize),
                    Value::Int(inst.c() as i64),
                    dst,
                ),
                Op::GetTabUp => (
                    self.upval_get((*fr).closure, inst.b()),
                    *kptr.add(inst.c() as usize),
                    dst,
                ),
                Op::GetGlobal => (
                    self.upval_get((*fr).closure, 0),
                    *kptr.add(inst.bx() as usize),
                    dst,
                ),
                Op::GetTabUpR => (
                    self.upval_get((*fr).closure, inst.b()),
                    if inst.k() {
                        *kptr.add(inst.c() as usize)
                    } else {
                        *regs.add(inst.c() as usize)
                    },
                    dst,
                ),
                op => unreachable!("index_op_miss on {op:?}"),
            }
        };
        self.index_miss(t, key, dst)
    }

    /// The rest of a table read with the object and the key in place (a
    /// register, or a constant for the key): a string key goes straight on
    /// to the `__index` chain, the rest out of line. `GetTable`, `GetField`
    /// and `SelfOp` call this from their arm, after the probe, with the
    /// pointers worked out again from the instruction there.
    ///
    /// # Safety
    /// `pt` and `pk` point at initialised values.
    #[inline(always)]
    pub(super) unsafe fn index_miss_at(
        &mut self,
        pt: *const Value,
        pk: *const Value,
        dst: u32,
    ) -> Result<(), LuaError> {
        // SAFETY: the caller's contract
        unsafe {
            #[cfg(not(feature = "gc-verify"))]
            if raw_tag(pk) == tag::STR {
                let key = Gc::from_ptr_unchecked(raw_gc(pk) as *mut LuaStr);
                return self.index_str_miss(pt, key, dst);
            }
            self.index_miss_cold(*pt, *pk, dst)
        }
    }

    #[cold]
    #[inline(never)]
    fn index_miss_cold(&mut self, t: Value, key: Value, dst: u32) -> Result<(), LuaError> {
        self.index_miss(t, key, dst)
    }

    /// [`Self::index_op_miss`] for a table write (`SetTable`, `SetField`,
    /// `SetI`, `SetTabUp`).
    ///
    /// # Safety
    /// `fr` points at the running frame.
    #[inline(never)]
    pub(super) unsafe fn newindex_op_miss(
        &mut self,
        inst: Inst,
        fr: *const Frame,
    ) -> Result<(), LuaError> {
        // SAFETY: the caller's contract; the compiler and the verifier keep
        // a constant operand inside the proto's constants, which `consts`
        // points at
        let (t, key, v) = unsafe {
            let (base, cl) = ((*fr).base, (*fr).closure);
            let r = |i: u32| self.stack[(base + i) as usize];
            let k = |i: u32| *cl.consts.add(i as usize);
            if inst.op() == Op::SetGlobal {
                let t = self.upval_get(cl, 0);
                return self.newindex_miss(t, k(inst.bx()), r(inst.a()));
            }
            let v = if inst.k() { k(inst.c()) } else { r(inst.c()) };
            let (t, key) = match inst.op() {
                Op::SetTable => (r(inst.a()), r(inst.b())),
                Op::SetTableK => (r(inst.a()), k(inst.b())),
                Op::SetField => (r(inst.a()), k(inst.b())),
                Op::SetI => (r(inst.a()), Value::Int(inst.b() as i64)),
                Op::SetTabUp | Op::SetTabUpK => (self.upval_get(cl, inst.a()), k(inst.b())),
                Op::SetTabUpR => (self.upval_get(cl, inst.a()), r(inst.b())),
                op => unreachable!("newindex_op_miss on {op:?}"),
            };
            (t, key, v)
        };
        self.newindex_miss(t, key, v)
    }

    /// A read opcode that [`Self::index_raw_at`] could not finish: continues the
    /// `__index` chain without repeating the raw probe.
    #[inline(always)]
    fn index_miss(&mut self, t: Value, key: Value, dst: u32) -> Result<(), LuaError> {
        #[cfg(not(feature = "gc-verify"))]
        {
            if let Value::Str(s) = key {
                // SAFETY: a local value
                return unsafe { self.index_str_miss(&t, s, dst) };
            }
            self.op_index_from(t, key, dst, matches!(t, Value::Table(_)))
        }
        #[cfg(feature = "gc-verify")]
        self.op_index(t, key, dst)
    }

    /// A write opcode that [`Self::newindex_raw_at`] could not finish: runs the
    /// `__newindex` chain without repeating that probe.
    #[inline(always)]
    fn newindex_miss(&mut self, t: Value, key: Value, v: Value) -> Result<(), LuaError> {
        // with no `__newindex` to call the write is a raw set (PUC
        // `luaV_finishset`)
        #[cfg(not(feature = "gc-verify"))]
        if let Value::Table(tb) = t
            && tb
                .metatable()
                .is_none_or(|mt| self.fast_tm(mt, Mm::NewIndex).is_nil())
        {
            return self.raw_set(tb, key, v);
        }
        // the fast path tried the existing key unless the table's flags
        // sent it here (`plain_store`): a black table still has it to do
        let probed = cfg!(not(feature = "gc-verify"))
            && matches!(t, Value::Table(tb) if tb.hdr.plain_store());
        self.op_newindex_from(t, key, v, probed)
    }

    /// `R[dst] := t[key]` for a string key when `t` is a table whose raw
    /// `t[key]` is nil, or not a table: follows up to four table-valued
    /// `__index` links with the pointer-chain lookup (PUC `luaV_finishget`
    /// with `fasttm`). Anything else restarts the full chain from `t`, so
    /// the loop limit and errors are those of `index_step`. `pt` points at
    /// the object, read before anything else, so that the fast loop can
    /// hand its register over and jump here.
    ///
    /// # Safety
    /// `pt` points at an initialised value.
    #[cfg(not(feature = "gc-verify"))]
    #[inline(never)]
    pub(super) unsafe fn index_str_miss(
        &mut self,
        pt: *const Value,
        key: Gc<crate::runtime::string::LuaStr>,
        dst: u32,
    ) -> Result<(), LuaError> {
        use super::fast_arith::{raw_gc, raw_tag};
        use crate::runtime::value::tag;
        // the object's tag and payload are read on their own: the register
        // was usually just written, and a whole-value read of it would wait
        // for that store to reach the cache
        // SAFETY: the caller's contract; a table tag means a live table
        let (mut on_table, mut mt) = unsafe {
            if raw_tag(pt) == tag::TABLE {
                (true, (*(raw_gc(pt) as *const Table)).metatable())
            } else {
                (false, self.metatable_of(*pt))
            }
        };
        // the slots are read in place and the value copied to its register
        // directly (see `table_get_into`)
        let out = self.stack.as_mut_ptr().wrapping_add(dst as usize);
        for _ in 0..4 {
            let Some(m) = mt else { break };
            let Some(link) = self.fast_tm_slot(m, Mm::Index) else {
                if on_table {
                    self.stack[dst as usize] = Value::Nil;
                    return Ok(());
                }
                break;
            };
            // SAFETY: a slot of a live table; a table tag means a live table
            unsafe {
                if raw_tag(link) != tag::TABLE {
                    break;
                }
                let next = &*(raw_gc(link) as *const Table);
                mt = next.metatable();
                match next.str_slot_by_ptr(key) {
                    Some(slot) if raw_tag(slot) != tag::NIL || mt.is_none() => {
                        // `dst` is a register of the running frame
                        Value::copy_raw(out, slot);
                        return Ok(());
                    }
                    Some(_) => {}
                    None if !key.is_short() => {
                        let v = next.get_str(key);
                        if !v.is_nil() || mt.is_none() {
                            self.stack[dst as usize] = v;
                            return Ok(());
                        }
                    }
                    None if mt.is_none() => {
                        self.stack[dst as usize] = Value::Nil;
                        return Ok(());
                    }
                    None => {}
                }
            }
            on_table = true;
        }
        // SAFETY: the caller's contract; nothing above wrote the stack
        let t = unsafe { *pt };
        self.op_index_from(t, Value::Str(key), dst, matches!(t, Value::Table(_)))
    }
}

impl Vm {
    /// [`Self::fast_tm`] as the slot holding the metamethod, `None` when it
    /// is absent (nil).
    #[inline]
    #[cfg_attr(feature = "gc-verify", allow(dead_code))]
    pub(crate) fn fast_tm_slot(&self, mt: Gc<Table>, mm: Mm) -> Option<*const Value> {
        let bit = 1u32 << mm as u32;
        if mt.absent_mm() & bit != 0 {
            return None;
        }
        // metamethod names are interned, so the pointer walk is exact
        match mt.str_slot_by_ptr(self.mm_names[mm as usize]) {
            Some(v) if !v.is_nil() => Some(v as *const Value),
            _ => {
                Table::note_absent_mm(mt, bit);
                None
            }
        }
    }
}
