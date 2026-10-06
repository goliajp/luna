//! `#t` inline: the array part's counts, or 5.4's `alimit`.

use super::*;

/// `#t` from the array part's counts: branch to `hit` with the length when
/// the table has no metatable and its non-nil slots are exactly a prefix
/// shorter than the array part (`Table::len`'s first case), to `miss`
/// otherwise; 5.5's search sets its length hint to that result, done here
/// too. A 5.4 table goes by `alimit` instead, as PUC 5.4's `luaH_getn`
/// does in its cases that move nothing (see [`emit_len_check_54`]).
pub(in super::super) fn emit_len_check<E: Emit>(
    bcx: &mut E,
    rules: TableRules,
    t: Value,
    hit: Block,
    miss: Block,
) {
    if rules == TableRules::V54 {
        emit_len_check_54(bcx, t, hit, miss);
        return;
    }
    let flags = MemFlagsData::trusted();
    let len_flags = bcx.len_state_flags();
    let mt = bcx.ins().load(
        types::I64,
        flags,
        t,
        super::super::super::TABLE_METATABLE_OFFSET as i32,
    );
    let no_mt = bcx.ins().icmp_imm_u(IntCC::Equal, mt, 0);
    let acount = bcx.ins().load(types::I32, flags, t, ACOUNT);
    let aprefix32 = bcx.ins().load(types::I32, flags, t, APREFIX);
    let dense = bcx.ins().icmp(IntCC::Equal, acount, aprefix32);
    let aprefix = bcx.ins().uextend(types::I64, aprefix32);
    let asize = load_asize(bcx, t);
    let short = bcx.ins().icmp(IntCC::UnsignedLessThan, aprefix, asize);
    let ok = bcx.ins().band(no_mt, dense);
    let mut ok = bcx.ins().band(ok, short);
    if rules == TableRules::Any {
        // a 5.4 table is left to the helper
        let dialect = bcx.ins().uload8(types::I64, flags, t, DIALECT);
        let not_54 = bcx.ins().icmp_imm_u(
            IntCC::NotEqual,
            dialect,
            i64::from(luna_core::runtime::table::jit_layout::TABLE_DIALECT_54),
        );
        ok = bcx.ins().band(ok, not_54);
    }
    if rules == TableRules::Pre54 {
        bcx.ins().brif(ok, hit, &[aprefix.into()], miss, &[]);
        return;
    }
    let hint_blk = bcx.create_block();
    bcx.ins().brif(ok, hint_blk, &[], miss, &[]);
    bcx.switch_to_block(hint_blk);
    bcx.seal_block(hint_blk);
    bcx.ins().store(len_flags, aprefix32, t, LENHINT);
    bcx.ins().jump(hit, &[aprefix.into()]);
}

/// 5.4's `#t`: first the cases where PUC's `luaH_getn` leaves `alimit`
/// as it is (with `l` the limit and `a` the array size, `0 < l < a` gives
/// `l` when `t[l]` is present and `t[l + 1]` nil; `l == a` gives `a` when
/// `t[a]` is present, or `a` is 0, and the hash part is empty), then a
/// leading run with nothing after it
/// (`Table::len`'s 5.4 shortcut), which gives the run's end and moves
/// `alimit` as [`emit_alimit_after_len`] says. Anything else goes to
/// `miss` (the helper).
fn emit_len_check_54<E: Emit>(bcx: &mut E, t: Value, hit: Block, miss: Block) {
    use luna_core::runtime::value::raw;
    let flags = MemFlagsData::trusted();
    let mt = bcx.ins().load(
        types::I64,
        flags,
        t,
        super::super::super::TABLE_METATABLE_OFFSET as i32,
    );
    let no_mt = bcx.ins().icmp_imm_u(IntCC::Equal, mt, 0);
    let l = load_alimit(bcx, t);
    let asize = load_asize(bcx, t);
    // `0 < l < a`, unsigned: `l - 1 < a - 1`; `l` 0 below a non-empty
    // array part is rare and left to the other cases
    let lm1 = bcx.ins().iadd_imm_s(l, -1);
    let am1 = bcx.ins().iadd_imm_s(asize, -1);
    let below = bcx.ins().icmp(IntCC::UnsignedLessThan, lm1, am1);
    let inner_blk = bcx.create_block();
    let full_blk = bcx.create_block();
    let dense_blk = bcx.create_block();
    let go = bcx.ins().band(no_mt, below);
    let other_blk = bcx.create_block();
    bcx.ins().brif(go, inner_blk, &[], other_blk, &[]);
    bcx.switch_to_block(other_blk);
    bcx.seal_block(other_blk);
    let at_full = bcx.ins().icmp(IntCC::Equal, l, asize);
    let go_full = bcx.ins().band(no_mt, at_full);
    bcx.ins().brif(go_full, full_blk, &[], miss, &[]);

    // `0 < l < a`: t[l] present and t[l + 1] nil
    bcx.switch_to_block(inner_blk);
    bcx.seal_block(inner_blk);
    let (_, atags) = array_part(bcx, t, asize);
    let tag_at = bcx.ins().iadd(atags, l);
    let ok = if cfg!(target_endian = "little") && raw::NIL == 0 {
        // both tags in one load, t[l]'s in the low byte: a set low byte
        // and a clear high one is 1..=255
        let pair = bcx
            .ins()
            .load(types::I16, MemFlagsData::new().with_notrap(), tag_at, -1);
        let pair = bcx.ins().uextend(types::I64, pair);
        let pm1 = bcx.ins().iadd_imm_s(pair, -1);
        bcx.ins().icmp_imm_u(IntCC::UnsignedLessThan, pm1, 255)
    } else {
        let next_tag = bcx.ins().uload8(types::I64, flags, tag_at, 0);
        let next_nil = bcx
            .ins()
            .icmp_imm_u(IntCC::Equal, next_tag, i64::from(raw::NIL));
        let prev_tag = bcx.ins().uload8(types::I64, flags, tag_at, -1);
        let prev_set = bcx
            .ins()
            .icmp_imm_u(IntCC::NotEqual, prev_tag, i64::from(raw::NIL));
        bcx.ins().band(prev_set, next_nil)
    };
    bcx.ins().brif(ok, hit, &[l.into()], dense_blk, &[]);

    // `l == a`: t[a] present (or a 0) and no hash part
    bcx.switch_to_block(full_blk);
    bcx.seal_block(full_blk);
    let mask = bcx.ins().load(
        types::I32,
        flags,
        t,
        super::super::super::TABLE_NODE_MASK_OFFSET as i32,
    );
    let no_hash = bcx
        .ins()
        .icmp_imm_u(IntCC::Equal, mask, i64::from(u32::MAX));
    let l_zero = bcx.ins().icmp_imm_u(IntCC::Equal, l, 0);
    let (_, atags) = array_part(bcx, t, asize);
    let last = bcx.ins().iadd(atags, l);
    let last_tag = bcx.ins().uload8(types::I64, flags, last, -1);
    let last_set = bcx
        .ins()
        .icmp_imm_u(IntCC::NotEqual, last_tag, i64::from(raw::NIL));
    let present = bcx.ins().select(l_zero, l_zero, last_set);
    let ok = bcx.ins().band(present, no_hash);
    bcx.ins().brif(ok, hit, &[l.into()], miss, &[]);

    // a leading run of p < a values and nothing after it
    bcx.switch_to_block(dense_blk);
    bcx.seal_block(dense_blk);
    let acount = bcx.ins().load(types::I32, flags, t, ACOUNT);
    let p32 = bcx.ins().load(types::I32, flags, t, APREFIX);
    let dense = bcx.ins().icmp(IntCC::Equal, acount, p32);
    let p = bcx.ins().uextend(types::I64, p32);
    let short = bcx.ins().icmp(IntCC::UnsignedLessThan, p, asize);
    let ok = bcx.ins().band(dense, short);
    let move_blk = bcx.create_block();
    bcx.ins().brif(ok, move_blk, &[], miss, &[]);
    bcx.switch_to_block(move_blk);
    bcx.seal_block(move_blk);
    emit_alimit_after_len(bcx, t, p32, asize);
    bcx.ins().jump(hit, &[p.into()]);
}

/// Where 5.4's `#t` leaves `alimit` (`l`) when the array part holds exactly
/// a leading run of `p` values, `p` below the size `a`
/// (`table::alimit_after_dense_len`): at `p` when `l < p`; when `l > p`, at
/// `p` if `a` is a power of two and either `l - 1 == p` with `p` not a
/// power of two, or `l - 1 != p` with `p` past half of `a`; else where it
/// was.
fn emit_alimit_after_len<E: Emit>(bcx: &mut E, t: Value, p: Value, asize: Value) {
    let len_flags = bcx.len_state_flags();
    let l = bcx.ins().load(types::I32, len_flags, t, ALIMIT);
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
    let moved = bcx.ins().bor(lt, lowered);
    let new_l = bcx.ins().select(moved, p, l);
    bcx.ins().store(len_flags, new_l, t, ALIMIT);
}
