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
    /// The rest of a table read the fast loop's probe could not finish
    /// (`GetTable`, `GetField`, `GetI`, `GetTabUp`, `SelfOp`): the operands
    /// are decoded again from `inst` and the frame here (PUC
    /// `luaV_finishget` after `luaV_fastget`), so that none of them stays
    /// live across the probe's own out-of-line calls in the loop, where they
    /// would push the loop's state out of registers. Not marked cold: a
    /// method call through `__index` takes this path every time.
    #[inline(never)]
    pub(super) fn index_op_miss(
        &mut self,
        inst: Inst,
        regs: *const Value,
        kptr: *const Value,
        fr: *const Frame,
    ) -> Result<(), LuaError> {
        // SAFETY: `fr` is the running frame and `regs` / `kptr` its
        // register window and constants, which the operands index (the
        // compiler and the verifier keep them in range)
        unsafe {
            let dst = (*fr).base + inst.a();
            let (pt, pk): (*const Value, *const Value) = match inst.op() {
                Op::GetTable => (regs.add(inst.b() as usize), regs.add(inst.c() as usize)),
                Op::GetField => (regs.add(inst.b() as usize), kptr.add(inst.c() as usize)),
                // the object was copied to `R[A+1]` before the probe
                Op::SelfOp => (
                    regs.add(inst.a() as usize + 1),
                    if inst.k() {
                        kptr.add(inst.c() as usize)
                    } else {
                        regs.add(inst.c() as usize)
                    },
                ),
                Op::GetI => {
                    let t = *regs.add(inst.b() as usize);
                    return self.index_miss(t, Value::Int(inst.c() as i64), dst);
                }
                Op::GetTabUp => {
                    let cl = (*fr).closure;
                    let t = self.upval_get(cl, inst.b());
                    return self.index_miss(t, *kptr.add(inst.c() as usize), dst);
                }
                op => unreachable!("index_op_miss on {op:?}"),
            };
            // a string key, nearly always, goes on with the object still in
            // its register: all the arguments fit in registers, so this is a
            // jump rather than a call
            #[cfg(not(feature = "gc-verify"))]
            if raw_tag(pk) == tag::STR {
                let key = Gc::from_ptr_unchecked(raw_gc(pk) as *mut LuaStr);
                return self.index_str_miss(pt, key, dst);
            }
            self.index_miss(*pt, *pk, dst)
        }
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
    #[inline(never)]
    pub(super) fn newindex_op_miss(
        &mut self,
        inst: Inst,
        fr: *const Frame,
    ) -> Result<(), LuaError> {
        // SAFETY: `fr` is the running frame
        let (base, cl) = unsafe { ((*fr).base, (*fr).closure) };
        let r = |vm: &Vm, i: u32| vm.stack[(base + i) as usize];
        // SAFETY: as in `index_op_miss`
        let k = |i: u32| unsafe { *cl.consts.add(i as usize) };
        let v = r(self, inst.c());
        let (t, key) = match inst.op() {
            Op::SetTable => (r(self, inst.a()), r(self, inst.b())),
            Op::SetField => (r(self, inst.a()), k(inst.b())),
            Op::SetI => (r(self, inst.a()), Value::Int(inst.b() as i64)),
            Op::SetTabUp => (self.upval_get(cl, inst.a()), k(inst.b())),
            op => unreachable!("newindex_op_miss on {op:?}"),
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
        let probed = cfg!(not(feature = "gc-verify")) && matches!(t, Value::Table(_));
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
        let mut on_table = unsafe { raw_tag(pt) } == tag::TABLE;
        let mut mt = if on_table {
            // SAFETY: as above
            unsafe { (*(raw_gc(pt) as *const Table)).metatable() }
        } else {
            // SAFETY: the caller's contract
            self.metatable_of(unsafe { *pt })
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
                // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
                unsafe { mt.as_mut() }.note_absent_mm(bit);
                None
            }
        }
    }
}
