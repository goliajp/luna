//! A number's tag and payload as two words. The numeric `for` writes its
//! control values with the seven bytes between the tag and the payload
//! zeroed, so that on aarch64 its step reads each one's tag and payload in
//! one load and tests the tag as a whole word.

use super::*;

/// A value's first word, its tag byte and the seven bytes after it, when
/// those seven are zero. The numeric `for` writes its control values so (see
/// [`put_words`]), and the aarch64 loop tests a value's tag and reads its
/// payload with one load of both words (see [`raw_words`]); anything else
/// that writes those registers leaves the seven bytes as they were, and
/// takes the loop once through `Vm::for_loop`, which writes them so again.
#[inline(always)]
pub(super) const fn tag_word(t: u8) -> u64 {
    u64::from_ne_bytes([t, 0, 0, 0, 0, 0, 0, 0])
}

/// Write the number with tag `t` and payload bits `bits` at `p`, the bytes
/// between the tag and the payload zeroed (see [`tag_word`]).
///
/// # Safety
/// `p` points at a `Value`, and `bits` are an integer's or a float's as `t`
/// names.
#[inline(always)]
pub(super) unsafe fn put_words(p: *mut Value, t: u8, bits: u64) {
    // SAFETY: the caller's contract; a `Value` is its tag byte, seven bytes
    // of padding and the payload word, and these two words are a valid one
    unsafe { (p as *mut [u64; 2]).write([tag_word(t), bits]) }
}

/// The first words (see [`tag_word`]) and the payloads of the three values
/// from `p` on, one load for each value's two words.
///
/// # Safety
/// `p` points at three initialised `Value`s.
#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline(always)]
pub(super) unsafe fn raw_words3(p: *const Value) -> ([u64; 3], [u64; 3]) {
    let (w0, v0, w1, v1, w2, v2): (u64, u64, u64, u64, u64, u64);
    // SAFETY: the caller's contract; `asm!` reads the padding bytes as the
    // bytes they are, which Rust code may not
    unsafe {
        core::arch::asm!(
            "ldp {w0}, {v0}, [{p}]",
            "ldp {w1}, {v1}, [{p}, #16]",
            "ldp {w2}, {v2}, [{p}, #32]",
            p = in(reg) p,
            w0 = out(reg) w0,
            v0 = out(reg) v0,
            w1 = out(reg) w1,
            v1 = out(reg) v1,
            w2 = out(reg) w2,
            v2 = out(reg) v2,
            options(pure, readonly, nostack, preserves_flags),
        )
    };
    ([w0, w1, w2], [v0, v1, v2])
}

/// The first words (see [`tag_word`]) of the three values from `p` on,
/// without their payloads: a float loop reads those as floats.
///
/// # Safety
/// `p` points at three initialised `Value`s.
#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline(always)]
pub(super) unsafe fn raw_tag_words3(p: *const Value) -> [u64; 3] {
    let (w0, w1, w2): (u64, u64, u64);
    // SAFETY: as for `raw_words3`
    unsafe {
        core::arch::asm!(
            "ldr {w0}, [{p}]",
            "ldr {w1}, [{p}, #16]",
            "ldr {w2}, [{p}, #32]",
            p = in(reg) p,
            w0 = out(reg) w0,
            w1 = out(reg) w1,
            w2 = out(reg) w2,
            options(pure, readonly, nostack, preserves_flags),
        )
    };
    [w0, w1, w2]
}

/// The first word of the value at `p` (see [`tag_word`]).
///
/// # Safety
/// `p` points at an initialised `Value`.
#[cfg(all(target_arch = "x86_64", not(miri)))]
#[inline(always)]
pub(super) unsafe fn raw_word(p: *const Value) -> u64 {
    let w: u64;
    // SAFETY: as for `raw_words3`
    unsafe {
        core::arch::asm!(
            "mov {w}, qword ptr [{p}]",
            p = in(reg) p,
            w = out(reg) w,
            options(pure, readonly, nostack, preserves_flags),
        )
    };
    w
}

/// [`raw_word`] on x86, as two 32-bit loads.
///
/// # Safety
/// `p` points at an initialised `Value`.
#[cfg(all(target_arch = "x86", not(miri)))]
#[inline(always)]
pub(super) unsafe fn raw_word(p: *const Value) -> u64 {
    let (lo, hi): (u32, u32);
    // SAFETY: as for `raw_words3`
    unsafe {
        core::arch::asm!(
            "mov {lo:e}, dword ptr [{p}]",
            "mov {hi:e}, dword ptr [{p} + 4]",
            p = in(reg) p,
            lo = out(reg) lo,
            hi = out(reg) hi,
            options(pure, readonly, nostack, preserves_flags),
        )
    };
    u64::from(lo) | (u64::from(hi) << 32)
}

/// [`raw_word`] on aarch64.
///
/// # Safety
/// `p` points at an initialised `Value`.
#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline(always)]
pub(super) unsafe fn raw_word(p: *const Value) -> u64 {
    let w: u64;
    // SAFETY: as for `raw_words3`
    unsafe {
        core::arch::asm!(
            "ldr {w}, [{p}]",
            p = in(reg) p,
            w = out(reg) w,
            options(pure, readonly, nostack, preserves_flags),
        )
    };
    w
}

/// `pc`, computed before the memory accesses that follow: the next fetch
/// waits for a loop's back-edge target, and the compiler, left alone, may
/// compute it after the control values' loads (Cortex-A76: 7.9 / 8.9 cycles
/// per iteration of an empty 5.5 integer loop). The empty `asm!` may read
/// and write memory as far as the compiler knows, so the loads stay after it.
#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline(always)]
pub(super) fn first(mut pc: u32) -> u32 {
    // SAFETY: the template is empty: it touches no register but `pc`, which
    // it leaves as it is, and no memory
    unsafe { core::arch::asm!("/* {0:w} */", inout(reg) pc, options(nostack, preserves_flags)) };
    pc
}

/// Make the first word of `v` differ from its tag (see [`tag_word`]): a
/// numeric `for` with `v` among its control values takes `Vm::for_loop` at
/// its next step, which checks all three. On aarch64, x86 and x86-64 the step of
/// a float loop does not test the tag of its limit, which it only reads, only
/// the whole first word of the step after it, so whatever changes a control
/// value outside the loop's own code unseals the one after it.
pub(crate) fn unseal(v: &mut Value) {
    let p: *mut Value = v;
    // SAFETY: the byte after the tag is padding, which any byte may fill
    unsafe { (p as *mut u8).add(1).write(1) }
}

impl Vm {
    /// The control values of the loop `inst` steps or prepares, as the fast
    /// loop's `for` arms write them (see [`tag_word`]) when they are all
    /// integers or all floats; otherwise (5.1/5.2 numbers of both kinds, or
    /// values `Vm::for_loop` raises on) the step unsealed, so that every
    /// step comes back here.
    pub(super) fn pad_for_state(&mut self, base: u32, inst: Inst) {
        use crate::runtime::value::tag;
        let a = inst.a();
        let at = |i: u32| (base + a + i) as usize;
        let tags = [0, 1, 2].map(|i| match self.stack[at(i)] {
            Value::Int(_) => tag::INT,
            Value::Float(_) => tag::FLOAT,
            _ => tag::NIL,
        });
        if tags[0] == tags[1] && tags[1] == tags[2] && tags[0] != tag::NIL {
            for i in 0..3 {
                let bits = match self.stack[at(i)] {
                    Value::Int(x) => x as u64,
                    Value::Float(f) => f.to_bits(),
                    _ => unreachable!("tested above"),
                };
                let p: *mut Value = &mut self.stack[at(i)];
                // SAFETY: `p` points at that register, a number of tag `tags[i]`
                unsafe { put_words(p, tags[i as usize], bits) };
            }
        } else {
            let step = if inst.op().for_layout() == Some(crate::vm::isa::ForLayout::Num55) {
                1
            } else {
                2
            };
            unseal(&mut self.stack[at(step)]);
        }
    }

    /// After a debug write into stack slot `slot` holding local `name`: a
    /// numeric `for`'s hidden state is named `(for ...)`, and the value a
    /// step tests for a control value it does not test comes right after it
    /// in every dialect, so that next slot is unsealed (see [`unseal`]).
    pub(crate) fn unseal_after_local_write(stack: &mut [Value], slot: usize, name: &str) {
        if name.starts_with("(for ")
            && let Some(v) = stack.get_mut(slot + 1)
        {
            unseal(v);
        }
    }
}
