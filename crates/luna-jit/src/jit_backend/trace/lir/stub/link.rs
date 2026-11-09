//! The stub taking a link itself: what `Vm::link_take` and
//! `Vm::trace_entry_counted` do, from the words the cell's `link_fast`
//! points at (`luna_core::__private::LinkFastOffsets`). Everything that can
//! leave the link to the Vm is checked before anything is written, so that
//! the Vm's way, taken then, finds nothing half done.

use super::super::cg::{Alu, Cond, Label, Masm, Width};
use luna_core::__private::{
    LINK_DEPTH, RunWords, STACK_PTR_OFFSET, STORE_BOOL, SideCell, VALUE_PAYLOAD, link_fast_offsets,
};

/// Why the stub hands the Vm a link, as `JitCounters::links_left_by_stub`
/// counts it: the label each check goes on at.
struct Bails {
    blocked: Label,
    quiet: Label,
    deep: Label,
    full: Label,
    check: Label,
    due: Label,
}

/// The link of cell `k[1]` taken, the linked trace's return returned; on to
/// `slow` (the Vm's way) when the cell has no words for it or a check fails.
/// `k` as in the stub; the argument registers and scratch are free.
pub(super) fn emit<M: Masm>(m: &mut M, k: &[u8], slow: Label) {
    let (a, at) = (M::INT_ARGS, SideCell::offsets());
    let b = Bails {
        blocked: m.new_label(),
        quiet: m.new_label(),
        deep: m.new_label(),
        full: m.new_label(),
        check: m.new_label(),
        due: m.new_label(),
    };
    m.load(Width::B8, a[0], k[1], at.link_fast);
    m.branch_reg(a[0], false, slow);
    emit_run_checks(m, k, &b);
    emit_slot_checks(m, b.check);
    emit_move_due(m, k, b.due);
    emit_trail(m, k);
    emit_stores(m, k);
    emit_counts(m, k);
    emit_enter(m, k);
    let why = [b.blocked, b.quiet, b.deep, b.full, b.check, b.due];
    for (n, l) in (1..).zip(why) {
        m.bind(l);
        m.load(Width::B8, a[1], k[1], at.words);
        m.mov_imm(M::SCRATCH[0], n);
        m.store(
            Width::B4,
            M::SCRATCH[0],
            a[1],
            RunWords::offsets().stub_left,
        );
        m.jmp(slow);
    }
}

/// The frame's first value on the stack into `f`, from the run words `w`.
fn frame<M: Masm>(m: &mut M, w: u8, f: u8, t: u8) {
    let rw = RunWords::offsets();
    m.load(Width::B8, f, w, rw.stack);
    m.load(Width::B8, f, f, STACK_PTR_OFFSET);
    m.load(Width::B4, t, w, rw.base);
    m.alu(Alu::Add, true, f, f, t);
}

/// What `link_take` checks of the run words before it takes the link,
/// with the run words left in the second argument register.
fn emit_run_checks<M: Masm>(m: &mut M, k: &[u8], b: &Bails) {
    let (at, rw) = (SideCell::offsets(), RunWords::offsets());
    let (t, u, w) = (M::SCRATCH[0], M::SCRATCH[1], M::INT_ARGS[1]);
    m.load(Width::B8, w, k[1], at.words);
    m.load(Width::B4, t, w, rw.blocked);
    m.branch_reg(t, true, b.blocked);
    m.load(Width::B4, t, w, rw.quiet);
    m.branch_reg(t, true, b.quiet);
    m.load(Width::B4, t, w, rw.trail_len);
    m.load(Width::B4, u, w, rw.link_base);
    m.alu(Alu::Sub, false, u, t, u);
    m.cmp_imm(false, u, LINK_DEPTH as i64);
    m.jcc(Cond::Uge, b.deep);
    m.load(Width::B4, u, w, rw.trail_cap);
    m.cmp(false, t, u);
    m.jcc(Cond::Uge, b.full);
}

/// The stack's slots the linked trace checks on entry, each holding the
/// byte it checks: else on to `fail`. The words in the first argument
/// register, the run words in the second.
fn emit_slot_checks<M: Masm>(m: &mut M, fail: Label) {
    let (a, lf) = (M::INT_ARGS, link_fast_offsets());
    let (t, u, p, w, f, e) = (M::SCRATCH[0], M::SCRATCH[1], a[0], a[1], a[2], a[3]);
    let (top, done) = (m.new_label(), m.new_label());
    m.load(Width::B4, e, p, lf.checks);
    m.branch_reg(e, false, done);
    frame(m, w, f, t);
    m.bind(top);
    m.load(Width::B4, t, p, lf.items);
    m.alu(Alu::Add, true, t, t, f);
    m.load(Width::B1, t, t, 0);
    m.load(Width::B4, u, p, lf.items + 4);
    m.cmp(false, t, u);
    m.jcc(Cond::Ne, fail);
    m.alu_imm(Alu::Add, true, p, p, 8);
    m.alu_imm(Alu::Sub, false, e, e, 1);
    m.branch_reg(e, true, top);
    m.bind(done);
}

/// On to `due` when the count `trace_entry_counted` adds makes the linked
/// trace's move (`count_towards_tier_up`'s test): the Vm makes it.
fn emit_move_due<M: Masm>(m: &mut M, k: &[u8], due: Label) {
    let (at, lf) = (SideCell::offsets(), link_fast_offsets());
    let (t, u, p) = (M::SCRATCH[0], M::SCRATCH[1], M::INT_ARGS[0]);
    let counted = m.new_label();
    m.load(Width::B8, t, k[1], at.link_tried);
    m.branch_reg(t, false, counted);
    m.load(Width::B1, t, t, 0);
    m.branch_reg(t, true, counted);
    m.load(Width::B8, t, k[1], at.link_tier_count);
    m.load(Width::B4, t, t, 0);
    m.alu_imm(Alu::Add, false, t, t, 1);
    m.load(Width::B8, p, k[1], at.link_fast);
    m.load(Width::B4, u, p, lf.at);
    m.cmp(false, t, u);
    m.jcc(Cond::Uge, due);
    m.load(Width::B4, u, p, lf.reused_at);
    m.cmp(false, t, u);
    m.jcc(Cond::Ult, counted);
    // past reused_at it moves once its head function was called again
    m.load(Width::B8, t, k[1], at.link_calls);
    m.load(Width::B4, t, t, 0);
    m.load(Width::B4, u, p, lf.calls_at);
    m.cmp(false, t, u);
    m.jcc(Cond::Ne, due);
    m.bind(counted);
}

/// The link noted on the trail.
fn emit_trail<M: Masm>(m: &mut M, k: &[u8]) {
    let (a, at, rw, lf) = (
        M::INT_ARGS,
        SideCell::offsets(),
        RunWords::offsets(),
        link_fast_offsets(),
    );
    let (t, u, p, w, e) = (M::SCRATCH[0], M::SCRATCH[1], a[0], a[1], a[3]);
    m.load(Width::B8, p, k[1], at.link_fast);
    m.load(Width::B8, w, k[1], at.words);
    m.load(Width::B4, t, w, rw.trail_len);
    m.load(Width::B8, u, w, rw.trail_at);
    m.alu_imm(Alu::Shl, true, e, t, 1);
    m.alu(Alu::Add, true, e, e, t);
    m.alu_imm(Alu::Shl, true, e, e, 2);
    m.alu(Alu::Add, true, u, u, e);
    for i in 0..3 {
        m.load(Width::B4, e, p, lf.trail + 4 * i);
        m.store(Width::B4, e, u, 4 * i);
    }
    m.alu_imm(Alu::Add, false, t, t, 1);
    m.store(Width::B4, t, w, rw.trail_len);
    m.load(Width::B4, t, w, rw.links_stub);
    m.alu_imm(Alu::Add, false, t, t, 1);
    m.store(Width::B4, t, w, rw.links_stub);
}

/// The registers stored to the frame's slots on the stack, as the exit's
/// write-back writes them.
fn emit_stores<M: Masm>(m: &mut M, k: &[u8]) {
    let (a, at, lf) = (M::INT_ARGS, SideCell::offsets(), link_fast_offsets());
    let (t, u, p, w, f, e) = (M::SCRATCH[0], M::SCRATCH[1], a[0], a[1], a[2], a[3]);
    let (top, next, word, done) = (m.new_label(), m.new_label(), m.new_label(), m.new_label());
    m.load(Width::B8, p, k[1], at.link_fast);
    m.load(Width::B4, e, p, lf.stores);
    m.branch_reg(e, false, done);
    m.load(Width::B4, t, p, lf.checks);
    m.alu_imm(Alu::Shl, true, t, t, 3);
    m.alu(Alu::Add, true, p, p, t);
    m.load(Width::B8, w, k[1], at.words);
    frame(m, w, f, t);
    m.bind(top);
    m.load(Width::B4, t, p, lf.items);
    m.alu(Alu::Add, true, t, t, f);
    m.load(Width::B4, u, p, lf.items + 8);
    m.store(Width::B1, u, t, 0);
    m.alu_imm(Alu::Lshr, false, u, u, 8);
    m.branch_reg(u, false, next);
    m.load(Width::B4, w, p, lf.items + 4);
    m.alu(Alu::Add, true, w, w, k[0]);
    m.load(Width::B8, w, w, 0);
    m.cmp_imm(false, u, i64::from(STORE_BOOL));
    m.jcc(Cond::Ne, word);
    m.alu_imm(Alu::And, true, w, w, 1);
    m.bind(word);
    m.store(Width::B8, w, t, VALUE_PAYLOAD);
    m.bind(next);
    m.alu_imm(Alu::Add, true, p, p, 12);
    m.alu_imm(Alu::Sub, false, e, e, 1);
    m.branch_reg(e, true, top);
    m.bind(done);
}

/// The words of the two traces' calls (`count_out`, `count_in`, the
/// side-trace note), the running trace, and the linked trace's count of
/// its move, in the order the Vm writes them.
fn emit_counts<M: Masm>(m: &mut M, k: &[u8]) {
    let (a, at, rw, lf) = (
        M::INT_ARGS,
        SideCell::offsets(),
        RunWords::offsets(),
        link_fast_offsets(),
    );
    let (t, u, p, e) = (M::SCRATCH[0], M::SCRATCH[1], a[0], a[3]);
    m.load(Width::B8, p, k[1], at.link_fast);
    let (no_out, no_in, no_note, no_count) =
        (m.new_label(), m.new_label(), m.new_label(), m.new_label());
    m.load(Width::B8, t, k[1], at.link_counts);
    m.branch_reg(t, false, no_out);
    m.load(Width::B4, u, p, lf.count_out);
    m.alu(Alu::Add, true, u, u, k[0]);
    m.load(Width::B4, u, u, 0);
    m.store(Width::B4, u, t, 0);
    m.bind(no_out);
    m.load(Width::B8, t, k[1], at.link_counts + 8);
    m.branch_reg(t, false, no_in);
    m.load(Width::B4, u, t, 0);
    m.alu_imm(Alu::Shl, true, e, u, 32);
    m.alu(Alu::Or, true, e, e, u);
    m.load(Width::B4, t, p, lf.count_in);
    m.alu(Alu::Add, true, t, t, k[0]);
    m.store(Width::B8, e, t, 0);
    m.mov_imm(e, 0);
    m.store(Width::B8, e, t, 8);
    m.bind(no_in);
    m.load(Width::B4, t, p, lf.has_cells);
    m.branch_reg(t, false, no_note);
    m.load(Width::B4, t, p, lf.ran_side);
    m.alu(Alu::Add, true, t, t, k[0]);
    m.mov_imm(u, 0);
    m.store(Width::B8, u, t, 0);
    m.bind(no_note);
    m.load(Width::B8, t, k[1], at.words);
    m.load(Width::B8, u, k[1], at.link_target);
    m.store(Width::B8, u, t, rw.running);
    m.load(Width::B8, t, k[1], at.link_tried);
    m.branch_reg(t, false, no_count);
    m.load(Width::B1, t, t, 0);
    m.branch_reg(t, true, no_count);
    m.load(Width::B8, t, k[1], at.link_tier_count);
    m.load(Width::B4, u, t, 0);
    m.alu_imm(Alu::Add, false, u, u, 1);
    m.store(Width::B4, u, t, 0);
    m.bind(no_count);
}

/// The linked trace's entry called now (the optimizing tier's once it
/// moved, else its own), its return returned.
fn emit_enter<M: Masm>(m: &mut M, k: &[u8]) {
    let (a, at, t) = (M::INT_ARGS, SideCell::offsets(), M::SCRATCH[0]);
    if cfg!(debug_assertions) {
        m.load(Width::B8, a[0], k[1], at.link_fast);
        m.mov(a[1], k[0]);
        m.call_abs(super::side_cell_list());
    }
    let have = m.new_label();
    m.load(Width::B8, t, k[1], at.link_entry);
    m.load(Width::B8, M::CALL_TARGET, t, 0);
    m.branch_reg(M::CALL_TARGET, true, have);
    m.load(Width::B8, t, k[1], at.link_base_entry);
    m.load(Width::B8, M::CALL_TARGET, t, 0);
    m.bind(have);
    m.mov(a[0], k[0]);
    m.call_reg(M::CALL_TARGET);
    m.epilogue_ret();
}

/// The stub's stores alone, as a function of the `TraceFn` ABI whose
/// `reg_state` is the buffer it is handed and whose cell is at `cell`; it
/// returns 0. For tests of the stores against the exit's write-back.
#[cfg(all(
    test,
    any(target_arch = "x86_64", all(target_arch = "aarch64", not(windows)))
))]
pub(in crate::jit_backend::trace) fn stores_alone(cell: usize) -> Vec<u8> {
    #[cfg(target_arch = "x86_64")]
    type Host = super::super::x64::X64;
    #[cfg(target_arch = "aarch64")]
    type Host = super::super::a64::A64;
    let mut m = Host::new(super::super::cg::Bufs::default());
    let k = &<Host as Masm>::INT.callee[..6];
    m.prologue(k, &[], 16);
    m.mov(k[0], <Host as Masm>::INT_ARGS[0]);
    m.mov_imm(k[1], cell as i64);
    emit_stores(&mut m, k);
    m.mov_imm(<Host as Masm>::RET, 0);
    m.epilogue_ret();
    m.finish().bytes
}
