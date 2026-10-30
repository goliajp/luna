//! Where compiled code checks for a collection: like PUC's `luaC_checkGC`
//! it compares the count with the point a step falls due at itself, and
//! calls `luna_jit_gc_due` (which notes the check for the exit) only once
//! the count has reached that point.

use super::*;
use luna_core::version::LuaVersion;

/// Whether some op of the trace may check for a collection: an allocating
/// op; in 5.1 an iterator call and the loop back to it; in 5.2 an inlined
/// call. A folded `string.sub` (from 5.3 on) learns it from its helper.
fn may_check(pl: &Plan<'_>) -> bool {
    let d = pl.opts.dialect;
    let v51 = d == Some(LuaVersion::Lua51);
    let v52 = d == Some(LuaVersion::Lua52);
    pl.record.ops.iter().any(|r| match r.inst.op() {
        Op::NewTable | Op::Closure | Op::Concat => true,
        Op::TForCall | Op::TForCall53 | Op::TForCall55 => v51,
        Op::TForLoop | Op::TForLoop53 | Op::TForLoop55 => v51,
        Op::Call => v52,
        _ => false,
    })
}

/// In the entry block of a trace that may check: where the count and the
/// due point lie (`luna_jit_due_words`), for every check to read. The
/// outer `None` when the helper cannot be declared.
pub(super) fn emit_due_words<E: Emit>(bcx: &mut E, pl: &Plan<'_>) -> Option<Option<Value>> {
    if !may_check(pl) {
        return Some(None);
    }
    let mut sig = bcx.make_signature();
    sig.returns.push(AbiParam::new(types::I64));
    let id = bcx
        .declare_function("luna_jit_due_words", Linkage::Import, &sig)
        .ok()?;
    let f = bcx.import_func(id);
    let call = bcx.ins().call(f, &[]);
    Some(Some(bcx.inst_results(call)[0]))
}

/// Whether the count has reached the point a step falls due at: no step
/// is due before then, and none of the exit's bookkeeping is needed.
fn reached<E: Emit>(lw: &mut Lower<E>) -> Value {
    let w = lw.due_words.expect("may_check covers every op that checks");
    let bytes = lw.bcx.ins().load(types::I64, MemFlagsData::new(), w, 0);
    let limit = lw.bcx.ins().load(
        types::I64,
        MemFlagsData::new(),
        w,
        luna_core::jit::JIT_GC_LIMIT_OFFSET,
    );
    lw.bcx
        .ins()
        .icmp(IntCC::UnsignedGreaterThanOrEqual, bytes, limit)
}

/// Runs what `slow` emits only once the count has reached the due point,
/// then goes on in a block both ways reach.
fn when_reached<E: Emit>(lw: &mut Lower<E>, slow: impl FnOnce(&mut Lower<E>)) {
    let r = reached(lw);
    let (slow_blk, cont) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(r, slow_blk, &[], cont, &[]);
    lw.bcx.switch_to_block(slow_blk);
    lw.bcx.seal_block(slow_blk);
    slow(lw);
    lw.bcx.ins().jump(cont, &[]);
    lw.bcx.switch_to_block(cont);
    lw.bcx.seal_block(cont);
}

/// `luna_jit_gc_due(check)`: whether a step is due, noted for the exit.
fn call_gc_due<E: Emit>(lw: &mut Lower<E>, check: u32) -> Value {
    let fref = lw.bcx.import_func(lw.h.rt.gc_due_id);
    let check = lw.bcx.ins().iconst(types::I64, i64::from(check));
    let call = lw.bcx.ins().call(fref, &[check]);
    lw.bcx.inst_results(call)[0]
}

/// After op `i`, which allocated through a helper, its result defined:
/// when a collection step is due, leave for `next_pc`, where the
/// interpreter checks for it as after the op (PUC's `luaC_checkGC`
/// there).
pub(super) fn gc_due_exit<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, i: usize, next_pc: u32) {
    gc_due_exit_as(lw, pl, i, next_pc, pl.record.ops[i].pc);
}

/// [`gc_due_exit`] with the check `check` names (`Vm::checkpoint_after_jit`).
pub(super) fn gc_due_exit_as<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    i: usize,
    next_pc: u32,
    check: u32,
) {
    when_reached(lw, |lw| {
        let due = call_gc_due(lw, check);
        due_exit(lw, pl, due, i, next_pc);
    });
}

/// [`gc_due_exit`] where the count has reached the due point when `reached`
/// is not 0, after `slow` emits what that way needs first.
pub(super) fn gc_due_exit_if<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    reached: Value,
    i: usize,
    next_pc: u32,
    slow: impl FnOnce(&mut Lower<E>),
) {
    let (slow_blk, cont) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(reached, slow_blk, &[], cont, &[]);
    lw.bcx.switch_to_block(slow_blk);
    lw.bcx.seal_block(slow_blk);
    slow(lw);
    let due = call_gc_due(lw, pl.record.ops[i].pc);
    due_exit(lw, pl, due, i, next_pc);
    lw.bcx.ins().jump(cont, &[]);
    lw.bcx.switch_to_block(cont);
    lw.bcx.seal_block(cont);
}

/// Notes a step due at the check `check` names for whichever exit the
/// trace takes next, without leaving.
pub(super) fn note_gc_due<E: Emit>(lw: &mut Lower<E>, check: u32) {
    when_reached(lw, |lw| {
        call_gc_due(lw, check);
    });
}

/// Whether lowering `rop` leaves a check made before it as true as it
/// was: the op allocates nothing, and the only helpers it calls read
/// tables, upvalues or the stack, or compute (no metamethod, no
/// collection). A store into a table may grow it, so it is not one.
fn keeps_gc_quiet(rop: &RecordedOp) -> bool {
    let ins = rop.inst;
    match ins.op() {
        Op::Move
        | Op::LoadI
        | Op::LoadF
        | Op::LoadK
        | Op::LoadNil
        | Op::LoadFalse
        | Op::LoadTrue
        | Op::LFalseSkip
        | Op::LTrueSkip
        | Op::Not
        | Op::Jmp
        | Op::Add
        | Op::Sub
        | Op::Mul
        | Op::Div
        | Op::Pow
        | Op::IDiv
        | Op::Mod
        | Op::BAnd
        | Op::BOr
        | Op::BXor
        | Op::Shl
        | Op::Shr
        | Op::Unm
        | Op::BNot
        | Op::EqK
        | Op::Test
        | Op::TestSet
        | Op::Lt
        | Op::Le
        | Op::Eq
        | Op::GetI
        | Op::GetTable
        | Op::GetField
        | Op::SelfOp
        | Op::GetTabUp
        | Op::GetUpval
        | Op::Call
        | Op::Vararg => true,
        // closing the frame's upvalues is not
        Op::Return0 | Op::Return1 | Op::Return => !ins.k(),
        _ => false,
    }
}

/// Before op `i` is lowered: forgets the last check unless every way to
/// the op passed it with nothing since that may allocate. A skip joining
/// here comes from before it, and a folded call runs a library function.
pub(super) fn gc_quiet_before<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, i: usize) {
    if lw.alt_joins.contains_key(&i) || pl.folded_ops[i] || !keeps_gc_quiet(&pl.record.ops[i]) {
        lw.gc_quiet = false;
    }
}

/// 5.2's check at inlined call `i`: none when the count and the due point
/// are what the last check found them (only it could have found a step
/// due, and then the trace left; `luna_jit_gc_due` changes nothing when
/// it finds none).
pub(super) fn call_gc_check<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, i: usize) {
    if !lw.gc_quiet {
        gc_due_exit_as(lw, pl, i, 0, luna_core::jit::JIT_GC_CALL);
        lw.gc_quiet = true;
    }
}
