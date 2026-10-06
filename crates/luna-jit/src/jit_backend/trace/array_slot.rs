//! Reading and writing a table's array part and taking its length inline,
//! the common case of `t[i]`, `t[i] = v` and `#t`; anything else goes
//! through the helpers.

use super::*;
use cranelift_codegen::ir::{Block, MemFlagsData};

const ACOUNT: i32 = super::super::TABLE_ACOUNT_OFFSET;
const APREFIX: i32 = super::super::TABLE_APREFIX_OFFSET;
const ALIMIT: i32 = super::super::TABLE_ALIMIT_OFFSET;
const LENHINT: i32 = super::super::TABLE_LENHINT_OFFSET;
const DIALECT: i32 = super::super::TABLE_DIALECT_OFFSET;

/// Which dialects' length rules the inline code serves: the Vm's dialect
/// when the trace was compiled for one, else all of them.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TableRules {
    /// 5.1–5.3: the array size bounds indexing, `#t` keeps no state
    Pre54,
    /// 5.4: indexing past `alimit` raises it, `#t` may move it
    V54,
    /// 5.5: the array size bounds indexing, `#t` sets the length hint
    V55,
    /// any of them
    Any,
}

impl TableRules {
    pub(crate) fn of(d: Option<luna_core::version::LuaVersion>) -> TableRules {
        use luna_core::version::LuaVersion as V;
        match d {
            Some(V::Lua51 | V::Lua52 | V::Lua53) => TableRules::Pre54,
            Some(V::Lua54 | V::MacroLua) => TableRules::V54,
            Some(V::Lua55) => TableRules::V55,
            None => TableRules::Any,
        }
    }
}

/// `(avals, atags)`: the array part's values and, after them, its raw tags.
fn array_part<E: Emit>(bcx: &mut E, t: Value, asize: Value) -> (Value, Value) {
    let avals = bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        super::super::TABLE_ARRAY_PTR_OFFSET as i32,
    );
    let bytes = bcx.ins().ishl_imm_u(asize, 3);
    let atags = bcx.ins().iadd(avals, bytes);
    (avals, atags)
}

/// The table's `alimit`: its array size, except in a 5.4 table after `#t`
/// lowered it (see `emit_array_bound`).
fn load_alimit<E: Emit>(bcx: &mut E, t: Value) -> Value {
    let l = bcx
        .ins()
        .load(types::I32, MemFlagsData::trusted(), t, ALIMIT);
    bcx.ins().uextend(types::I64, l)
}

/// Branch to `slot` when array index `idx` (key - 1) is in the array
/// part, to `miss` otherwise; leaves the builder in no block and seals
/// `slot`. A table that may be 5.4's is bounded by `alimit`, and an index
/// between it and the array size raises it to the key, as
/// `Table::array_index` does; others by the array size alone, which is
/// then returned for `slot` to use.
fn emit_array_bound<E: Emit>(
    bcx: &mut E,
    rules: TableRules,
    t: Value,
    idx: Value,
    slot: Block,
    miss: Block,
) -> Option<Value> {
    if matches!(rules, TableRules::Pre54 | TableRules::V55) {
        let asize = load_asize(bcx, t);
        // unsigned: also rejects keys below 1
        let in_array = bcx.ins().icmp(IntCC::UnsignedLessThan, idx, asize);
        bcx.ins().brif(in_array, slot, &[], miss, &[]);
        bcx.seal_block(slot);
        return Some(asize);
    }
    let alimit = load_alimit(bcx, t);
    let in_limit = bcx.ins().icmp(IntCC::UnsignedLessThan, idx, alimit);
    let past_blk = bcx.create_block();
    bcx.ins().brif(in_limit, slot, &[], past_blk, &[]);
    bcx.switch_to_block(past_blk);
    bcx.seal_block(past_blk);
    let asize = load_asize(bcx, t);
    let in_array = bcx.ins().icmp(IntCC::UnsignedLessThan, idx, asize);
    let raise_blk = bcx.create_block();
    bcx.ins().brif(in_array, raise_blk, &[], miss, &[]);
    bcx.switch_to_block(raise_blk);
    bcx.seal_block(raise_blk);
    let key = bcx.ins().iadd_imm_u(idx, 1);
    let key32 = bcx.ins().ireduce(types::I32, key);
    bcx.ins().store(MemFlagsData::trusted(), key32, t, ALIMIT);
    bcx.ins().jump(slot, &[]);
    bcx.seal_block(slot);
    None
}

fn load_asize<E: Emit>(bcx: &mut E, t: Value) -> Value {
    bcx.ins().load(
        types::I64,
        MemFlagsData::trusted(),
        t,
        super::super::TABLE_ASIZE_OFFSET as i32,
    )
}

/// Branch to `hit`, with the address of `t[key]`'s payload as its one
/// parameter, when the integer `key` falls in the array part and the slot
/// holds a value of raw tag `want` (not nil, so no `__index` is consulted);
/// to `miss` otherwise. Leaves the builder in no block; the caller seals
/// `hit` and `miss`.
pub(super) fn emit_array_get_check<E: Emit>(
    bcx: &mut E,
    rules: TableRules,
    t: Value,
    key: Value,
    want: u8,
    hit: Block,
    miss: Block,
) {
    let idx = bcx.ins().iadd_imm_s(key, -1);
    let slot_blk = bcx.create_block();
    let known = emit_array_bound(bcx, rules, t, idx, slot_blk, miss);
    bcx.switch_to_block(slot_blk);
    let asize = known.unwrap_or_else(|| load_asize(bcx, t));
    let (avals, atags) = array_part(bcx, t, asize);
    let tag_addr = bcx.ins().iadd(atags, idx);
    let tag = bcx
        .ins()
        .uload8(types::I64, MemFlagsData::trusted(), tag_addr, 0);
    let ok = bcx.ins().icmp_imm_u(IntCC::Equal, tag, i64::from(want));
    let off = bcx.ins().ishl_imm_u(idx, 3);
    let val_addr = bcx.ins().iadd(avals, off);
    bcx.ins().brif(ok, hit, &[val_addr.into()], miss, &[]);
}

/// `t[key] = val` for an integer `key` in the array part and a value of
/// raw tag `r` the collector does not trace (a collectable one needs the
/// write barrier), keeping the `acount` / `aprefix` counts in step as
/// `Table::note_atag_change` does: jump to `done` when stored, to `miss`
/// when the store is the helper's (out of the array part; a read-only
/// table, which the helper refuses, when `test_readonly`; a nil slot of a table with a
/// metatable, whose `__newindex` decides; a slot that extends the non-nil
/// prefix past further filled slots, which the table scans for). Leaves
/// the builder in no block; the caller seals `miss` and `done`.
pub(super) fn emit_array_set<E: Emit>(
    bcx: &mut E,
    rules: TableRules,
    t: Value,
    key: Value,
    val: Value,
    r: u8,
    done: Block,
    miss: Block,
    test_readonly: bool,
) {
    use luna_core::runtime::value::raw;
    debug_assert!(!raw::is_gc(r) && r != raw::NIL);
    let flags = MemFlagsData::trusted();
    if test_readonly {
        emit_writable_guard(bcx, t, miss);
    }
    let idx = bcx.ins().iadd_imm_s(key, -1);
    let slot_blk = bcx.create_block();
    let known = emit_array_bound(bcx, rules, t, idx, slot_blk, miss);

    bcx.switch_to_block(slot_blk);
    let asize = known.unwrap_or_else(|| load_asize(bcx, t));
    let (avals, atags) = array_part(bcx, t, asize);
    let tag_addr = bcx.ins().iadd(atags, idx);
    let off = bcx.ins().ishl_imm_u(idx, 3);
    let val_addr = bcx.ins().iadd(avals, off);
    let old = bcx.ins().uload8(types::I64, flags, tag_addr, 0);
    let was_nil = bcx.ins().icmp_imm_u(IntCC::Equal, old, i64::from(raw::NIL));
    let store_blk = bcx.create_block();
    let fill_blk = bcx.create_block();
    bcx.ins().brif(was_nil, fill_blk, &[], store_blk, &[]);

    // a nil slot gains a value: only without a metatable
    bcx.switch_to_block(fill_blk);
    bcx.seal_block(fill_blk);
    let mt = bcx.ins().load(
        types::I64,
        flags,
        t,
        super::super::TABLE_METATABLE_OFFSET as i32,
    );
    let no_mt = bcx.ins().icmp_imm_u(IntCC::Equal, mt, 0);
    let count_blk = bcx.create_block();
    bcx.ins().brif(no_mt, count_blk, &[], miss, &[]);

    bcx.switch_to_block(count_blk);
    bcx.seal_block(count_blk);
    let acount = bcx.ins().load(types::I32, flags, t, ACOUNT);
    let acount = bcx.ins().iadd_imm_u(acount, 1);
    let aprefix = bcx.ins().load(types::I32, flags, t, APREFIX);
    let aprefix = bcx.ins().uextend(types::I64, aprefix);
    let at_prefix = bcx.ins().icmp(IntCC::Equal, idx, aprefix);
    let plain_blk = bcx.create_block();
    let prefix_blk = bcx.create_block();
    bcx.ins().brif(at_prefix, prefix_blk, &[], plain_blk, &[]);

    bcx.switch_to_block(plain_blk);
    bcx.seal_block(plain_blk);
    bcx.ins().store(flags, acount, t, ACOUNT);
    bcx.ins().jump(store_blk, &[]);

    // the prefix grows by this slot when the next one is nil or past the end
    bcx.switch_to_block(prefix_blk);
    bcx.seal_block(prefix_blk);
    let next = bcx.ins().iadd_imm_u(idx, 1);
    let at_end = bcx.ins().icmp(IntCC::Equal, next, asize);
    let grow_blk = bcx.create_block();
    let peek_blk = bcx.create_block();
    bcx.ins().brif(at_end, grow_blk, &[], peek_blk, &[]);

    bcx.switch_to_block(peek_blk);
    bcx.seal_block(peek_blk);
    let next_tag = bcx.ins().uload8(types::I64, flags, tag_addr, 1);
    let next_nil = bcx
        .ins()
        .icmp_imm_u(IntCC::Equal, next_tag, i64::from(raw::NIL));
    bcx.ins().brif(next_nil, grow_blk, &[], miss, &[]);

    bcx.switch_to_block(grow_blk);
    bcx.seal_block(grow_blk);
    bcx.ins().store(flags, acount, t, ACOUNT);
    let next32 = bcx.ins().ireduce(types::I32, next);
    bcx.ins().store(flags, next32, t, APREFIX);
    bcx.ins().jump(store_blk, &[]);

    bcx.switch_to_block(store_blk);
    bcx.seal_block(store_blk);
    let tag = bcx.ins().iconst(types::I8, i64::from(r));
    bcx.ins().store(flags, tag, tag_addr, 0);
    bcx.ins().store(flags, val, val_addr, 0);
    bcx.ins().jump(done, &[]);
}

/// Branch to `miss` when table `t` is read-only (`Table::is_readonly`),
/// whose stores the helper on `miss` refuses; continue in a new block
/// otherwise: a byte load and a bit test of the table header's flag byte.
/// Where a store needs the test at all is `lower::readonly`'s choice.
pub(super) fn emit_writable_guard<E: Emit>(bcx: &mut E, t: Value, miss: Block) {
    let ok_blk = bcx.create_block();
    emit_writable_guard_to(bcx, t, miss, ok_blk);
    bcx.switch_to_block(ok_blk);
    bcx.seal_block(ok_blk);
}

/// Branch to `ro` when table `t` is read-only, to `ok` otherwise; leaves
/// the builder in no block.
pub(super) fn emit_writable_guard_to<E: Emit>(bcx: &mut E, t: Value, ro: Block, ok: Block) {
    let byte = bcx.ins().uload8(
        types::I64,
        MemFlagsData::trusted(),
        t,
        super::super::TABLE_READONLY_BYTE_OFFSET,
    );
    let bit = bcx
        .ins()
        .band_imm_u(byte, super::super::TABLE_READONLY_BYTE_MASK);
    let is_ro = bcx.ins().icmp_imm_u(IntCC::NotEqual, bit, 0);
    bcx.ins().brif(is_ro, ro, &[], ok, &[]);
}

/// `#t` from the array part's counts: branch to `hit` with the length when
/// the table has no metatable and its non-nil slots are exactly a prefix
/// shorter than the array part (`Table::len`'s first case), to `miss`
/// otherwise. That search leaves state behind in two dialects, kept here
/// as `Table::len` keeps it: 5.5 sets its length hint to the result, and
/// 5.4 may move `alimit` to it.
pub(super) fn emit_len_check<E: Emit>(
    bcx: &mut E,
    rules: TableRules,
    t: Value,
    hit: Block,
    miss: Block,
) {
    let flags = MemFlagsData::trusted();
    let mt = bcx.ins().load(
        types::I64,
        flags,
        t,
        super::super::TABLE_METATABLE_OFFSET as i32,
    );
    let no_mt = bcx.ins().icmp_imm_u(IntCC::Equal, mt, 0);
    let acount = bcx.ins().load(types::I32, flags, t, ACOUNT);
    let aprefix32 = bcx.ins().load(types::I32, flags, t, APREFIX);
    let dense = bcx.ins().icmp(IntCC::Equal, acount, aprefix32);
    let aprefix = bcx.ins().uextend(types::I64, aprefix32);
    let asize = load_asize(bcx, t);
    let short = bcx.ins().icmp(IntCC::UnsignedLessThan, aprefix, asize);
    let ok = bcx.ins().band(no_mt, dense);
    let ok = bcx.ins().band(ok, short);
    if rules == TableRules::Pre54 {
        bcx.ins().brif(ok, hit, &[aprefix.into()], miss, &[]);
        return;
    }
    if rules == TableRules::V54 {
        // `alimit` already at the result (a table grown by `t[#t + 1]`
        // stores is so) stays there; anything else is worked out
        let state_blk = bcx.create_block();
        let check_blk = bcx.create_block();
        bcx.ins().brif(ok, check_blk, &[], miss, &[]);
        bcx.switch_to_block(check_blk);
        bcx.seal_block(check_blk);
        let l = bcx.ins().load(types::I32, flags, t, ALIMIT);
        let same = bcx.ins().icmp(IntCC::Equal, l, aprefix32);
        bcx.ins().brif(same, hit, &[aprefix.into()], state_blk, &[]);
        bcx.switch_to_block(state_blk);
        bcx.seal_block(state_blk);
        emit_alimit_after_len(bcx, rules, t, aprefix32, asize);
        bcx.ins().jump(hit, &[aprefix.into()]);
        return;
    }
    let state_blk = bcx.create_block();
    bcx.ins().brif(ok, state_blk, &[], miss, &[]);
    bcx.switch_to_block(state_blk);
    bcx.seal_block(state_blk);
    if rules == TableRules::Any {
        emit_alimit_after_len(bcx, rules, t, aprefix32, asize);
    }
    bcx.ins().store(flags, aprefix32, t, LENHINT);
    bcx.ins().jump(hit, &[aprefix.into()]);
}

/// Where 5.4's `#t` leaves `alimit` (`l`) when the array part holds exactly
/// a leading run of `p` values, `p` below the size `a` (`Table::len`): at
/// `p` when `l < p`; when `l > p`, at `p` if `a` is a power of two and
/// either `l - 1 == p` with `p` not a power of two, or `l - 1 != p` with
/// `p` past half of `a`; else where it was. With `TableRules::Any` only a
/// 5.4 table's moves.
fn emit_alimit_after_len<E: Emit>(
    bcx: &mut E,
    rules: TableRules,
    t: Value,
    p: Value,
    asize: Value,
) {
    let flags = MemFlagsData::trusted();
    let l = bcx.ins().load(types::I32, flags, t, ALIMIT);
    let a = bcx.ins().ireduce(types::I32, asize);
    let lt = bcx.ins().icmp(IntCC::UnsignedLessThan, l, p);
    let gt = bcx.ins().icmp(IntCC::UnsignedGreaterThan, l, p);
    // PUC `ispow2`: `x & (x - 1) == 0`
    let am1 = bcx.ins().iadd_imm_s(a, -1);
    let a_and = bcx.ins().band(a, am1);
    let pow2a = bcx.ins().icmp_imm_u(IntCC::Equal, a_and, 0);
    let lm1 = bcx.ins().iadd_imm_s(l, -1);
    let adjacent = bcx.ins().icmp(IntCC::Equal, lm1, p);
    let pm1 = bcx.ins().iadd_imm_s(p, -1);
    let p_and = bcx.ins().band(p, pm1);
    let not_pow2p = bcx.ins().icmp_imm_u(IntCC::NotEqual, p_and, 0);
    let half = bcx.ins().ushr_imm_u(a, 1);
    let past_half = bcx.ins().icmp(IntCC::UnsignedGreaterThan, p, half);
    let inner = bcx.ins().select(adjacent, not_pow2p, past_half);
    let lowered = bcx.ins().band(gt, pow2a);
    let lowered = bcx.ins().band(lowered, inner);
    let mut moved = bcx.ins().bor(lt, lowered);
    if rules == TableRules::Any {
        let dialect = bcx.ins().uload8(types::I64, flags, t, DIALECT);
        let is_54 = bcx.ins().icmp_imm_u(
            IntCC::Equal,
            dialect,
            i64::from(luna_core::runtime::table::jit_layout::TABLE_DIALECT_54),
        );
        moved = bcx.ins().band(moved, is_54);
    }
    let new_l = bcx.ins().select(moved, p, l);
    bcx.ins().store(flags, new_l, t, ALIMIT);
}
