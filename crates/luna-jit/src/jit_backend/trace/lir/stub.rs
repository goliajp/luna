//! The baseline tier's call stub: one piece of code, shared by every trace
//! of every Vm in the process, that runs the side trace a calling exit's
//! cell holds (see `lower::side_call` for what the optimizing tier's call
//! block does in the trace itself).
//!
//! A calling exit whose cell holds a side trace's entry stores the
//! registers as any exit does, leaves the trace's frame, and goes on at the
//! stub with [`Masm::STUB_ARGS`] holding `reg_state`, the cell, the exit's
//! return, the note for the dispatcher (low half) with the trace's head pc
//! (high half), the address of the word the trace notes a side trace in
//! (`luna_core::__private::trace_ran_side_slot`), and the address of the
//! trace's relink block. The stub returns to the trace's caller: the exit's
//! return when the side trace may not run here, else what the side trace
//! (or the trace it links to) returned. A side trace that ended at the
//! trace's head goes on in the trace's loop: the stub leaves whether it may
//! go on in the optimizing tier's loop in the noting word and goes on at the
//! relink block, which the trace's code enters as a function of its own
//! with `reg_state` alone.
//!
//! The stub holds no address of a Vm or a trace: the helper it calls is the
//! same in every Vm, and everything else comes from the exit (the Vm's run
//! words through the cell).

#[cfg(all(
    test,
    any(target_arch = "x86_64", all(target_arch = "aarch64", not(windows)))
))]
pub(in crate::jit_backend::trace) mod alone;
pub(in crate::jit_backend::trace) mod link;

use super::cg::{Alu, Bufs, Cond, Label, Masm, Width};
use luna_core::__private::{CHECK_COUNT, CHECK_ITEMS, EXIT_RELINK, RunWords, SideCell};

/// The stub's address, placed on first use. A process has one target, so
/// one [`Masm`]: the stub is shared by every trace's code and every Vm's,
/// and never freed. Placing it may fail (no code memory); the next trace
/// tries again.
pub(crate) fn address<M: Masm>() -> Result<usize, &'static str> {
    static AT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    static PLACING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Some(&at) = AT.get() {
        return Ok(at);
    }
    let _placing = PLACING.lock().map_err(|_| "placing the call stub")?;
    if let Some(&at) = AT.get() {
        return Ok(at);
    }
    // an arena dropped leaves its code mapped
    let at = super::CodeArena::default().place(&generate::<M>())? as usize;
    Ok(*AT.get_or_init(|| at))
}

/// Where the side trace's return is kept across the call into a linked
/// trace, at the stack pointer.
fn ret_at<M: Masm>() -> i32 {
    M::CALL_SHADOW as i32
}

/// The stub's code (see the module comment). It keeps what the exit hands
/// it in the first six callee-saved registers, `k`.
fn generate<M: Masm>() -> Vec<u8> {
    let mut m = M::new(Bufs::default());
    let k = &M::INT.callee[..6];
    m.prologue(k, &[], (M::CALL_SHADOW + 8).next_multiple_of(16));
    for (&d, &s) in k.iter().zip(&M::STUB_ARGS) {
        m.mov(d, s);
    }
    let (out, other, back) = (m.new_label(), m.new_label(), m.new_label());
    emit_call(&mut m, k, out);
    emit_on_return(&mut m, k, other, back);
    // returned for the dispatcher, the exit noted
    m.bind(other);
    m.zext(M::SCRATCH[0], k[3], 32);
    m.store(Width::B8, M::SCRATCH[0], k[4], 0);
    m.load(Width::B8, M::RET, M::SP, ret_at::<M>());
    m.epilogue_ret();
    // left to the dispatcher: the exit's return
    m.bind(out);
    m.mov(M::RET, k[2]);
    m.epilogue_ret();
    m.bind(back);
    emit_back(&mut m, k);
    m.finish().bytes
}

/// The side trace called when it may run, its return at [`ret_at`] and in
/// [`Masm::RET`]; else on to `out`: its move to the optimizing tier is due
/// (counted here; the dispatcher makes it), or one of the dispatcher's
/// checks before it runs a side trace fails.
fn emit_call<M: Masm>(m: &mut M, k: &[u8], out: super::cg::Label) {
    let (a, at) = (M::INT_ARGS, SideCell::offsets());
    let (t, u) = (M::SCRATCH[0], M::SCRATCH[1]);
    m.load(Width::B8, t, k[1], at.count);
    m.load(Width::B4, u, t, 0);
    m.alu_imm(Alu::Add, false, u, u, 1);
    m.store(Width::B4, u, t, 0);
    m.load(Width::B4, t, k[1], at.at);
    m.cmp(false, u, t);
    m.jcc(Cond::Uge, out);
    // the dispatcher's checks: the run words, then the slots on the stack
    let checked = m.new_label();
    m.load(Width::B8, a[1], k[1], at.words);
    m.load(Width::B4, t, a[1], RunWords::offsets().blocked);
    m.branch_reg(t, true, out);
    m.load(Width::B8, a[0], k[1], at.check_fast);
    m.branch_reg(a[0], false, checked);
    if cfg!(debug_assertions) {
        emit_checks_agreeing(m, k, checked, out);
    } else {
        link::emit_slot_checks(m, out, (CHECK_COUNT, CHECK_ITEMS));
    }
    m.bind(checked);
    m.load(Width::B8, M::CALL_TARGET, k[1], at.entry);
    m.mov(a[0], k[0]);
    m.call_reg(M::CALL_TARGET);
    m.store(Width::B8, M::RET, M::SP, ret_at::<M>());
}

/// The slot checks of the words at `check_fast`, in a debug build: each
/// way they go, the Vm's run of the cell's `check` list must go the same,
/// else the words' list for `luna_jit_side_cell_list` panics. On to
/// `checked` when they pass, to `out` when they fail.
fn emit_checks_agreeing<M: Masm>(m: &mut M, k: &[u8], checked: Label, out: Label) {
    let (a, at) = (M::INT_ARGS, SideCell::offsets());
    let (failed, disagree) = (m.new_label(), m.new_label());
    link::emit_slot_checks(m, failed, (CHECK_COUNT, CHECK_ITEMS));
    let vm_checks = |m: &mut M| {
        m.load(Width::B8, a[0], k[1], at.check);
        m.mov(a[1], k[0]);
        m.call_abs(side_cell_list());
    };
    vm_checks(m);
    m.branch_reg(M::RET, true, checked);
    m.jmp(disagree);
    m.bind(failed);
    vm_checks(m);
    m.branch_reg(M::RET, false, out);
    m.bind(disagree);
    m.load(Width::B8, a[0], k[1], at.check_fast);
    m.mov(a[1], k[0]);
    m.call_abs(side_cell_list());
    m.jmp(out);
}

/// On the side trace's return: to `back` when it ended at the trace's head;
/// into the trace the cell links to when it ended at that one's head and
/// the link may be taken, whose return is returned; to `other` otherwise.
fn emit_on_return<M: Masm>(m: &mut M, k: &[u8], other: super::cg::Label, back: super::cg::Label) {
    let (a, at, t) = (M::INT_ARGS, SideCell::offsets(), M::SCRATCH[0]);
    m.mov_imm(t, EXIT_RELINK as i64);
    m.alu(Alu::And, true, t, t, M::RET);
    m.branch_reg(t, false, other);
    m.alu_imm(Alu::Lshr, true, t, k[3], 32);
    m.cmp(false, M::RET, t);
    m.jcc(Cond::Eq, back);
    m.load(Width::B4, t, k[1], at.link_pc);
    m.cmp(false, M::RET, t);
    m.jcc(Cond::Ne, other);
    let slow = m.new_label();
    link::emit(m, k, slow);
    m.bind(slow);
    m.load(Width::B8, a[0], k[1], at.link);
    m.mov(a[1], k[0]);
    m.call_abs(side_cell_list());
    m.branch_reg(M::RET, false, other);
    m.mov(M::CALL_TARGET, M::RET);
    m.mov(a[0], k[0]);
    m.call_reg(M::CALL_TARGET);
    m.epilogue_ret();
}

/// Back into the trace's loop: the cell's relink count bumped and its stores
/// to the stack made, whether the side trace may go on in the optimizing
/// tier's loop left in the noting word's low half and the note in its high
/// half, and on at the relink block with `reg_state`, the stub's frame left.
fn emit_back<M: Masm>(m: &mut M, k: &[u8]) {
    let (a, at, t) = (M::INT_ARGS, SideCell::offsets(), M::SCRATCH[0]);
    m.load(Width::B4, t, k[1], at.relinks);
    m.alu_imm(Alu::Add, false, t, t, 1);
    m.store(Width::B4, t, k[1], at.relinks);
    let stored = m.new_label();
    m.load(Width::B8, a[0], k[1], at.back);
    m.branch_reg(a[0], false, stored);
    m.mov(a[1], k[0]);
    m.call_abs(side_cell_list());
    m.bind(stored);
    m.load(Width::B4, t, k[1], at.loop_ok);
    let u = M::SCRATCH[1];
    m.alu_imm(Alu::Shl, true, u, k[3], 32);
    m.alu(Alu::Or, true, t, t, u);
    m.store(Width::B8, t, k[4], 0);
    m.mov(a[0], k[0]);
    m.mov(M::SCRATCH[1], k[5]);
    m.epilogue_jmp(M::SCRATCH[1]);
}

/// `luna_jit_side_cell_list`'s address.
fn side_cell_list() -> usize {
    crate::jit_backend::luna_jit_side_cell_list as *const u8 as usize
}
