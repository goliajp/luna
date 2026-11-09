//! Pieces of the stub as functions of their own, for tests of their code.

use super::super::cg::{Bufs, Masm};

#[cfg(target_arch = "x86_64")]
type Host = super::super::x64::X64;
#[cfg(target_arch = "aarch64")]
type Host = super::super::a64::A64;

/// What the stub's link taking returns when it leaves the link to the Vm.
pub(in crate::jit_backend::trace) const LEFT: i64 = 0x1ef7;

/// `body` as a function of the `TraceFn` ABI whose `reg_state` is the
/// buffer it is handed and whose cell is at `cell`, with the stub's
/// registers; it returns what `body` leaves in the return register.
fn alone(cell: usize, body: impl FnOnce(&mut Host, &[u8])) -> Vec<u8> {
    let mut m = Host::new(Bufs::default());
    let k = &<Host as Masm>::INT.callee[..6];
    m.prologue(k, &[], 16);
    m.mov(k[0], <Host as Masm>::INT_ARGS[0]);
    m.mov_imm(k[1], cell as i64);
    body(&mut m, k);
    m.epilogue_ret();
    m.finish().bytes
}

/// The stub's stores alone; it returns 0. For tests of the stores against
/// the exit's write-back.
pub(in crate::jit_backend::trace) fn stores_alone(cell: usize) -> Vec<u8> {
    alone(cell, |m, k| {
        super::link::emit_stores(m, k);
        m.mov_imm(<Host as Masm>::RET, 0);
    })
}

/// The stub's taking of the link alone: [`LEFT`] when it leaves the link
/// to the Vm, else what the linked trace's entry returns. For tests of
/// the checks before it takes the link.
pub(in crate::jit_backend::trace) fn link_alone(cell: usize) -> Vec<u8> {
    alone(cell, |m, k| {
        let slow = m.new_label();
        super::link::emit(m, k, slow);
        m.bind(slow);
        m.mov_imm(<Host as Masm>::RET, LEFT);
    })
}
