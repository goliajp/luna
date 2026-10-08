//! Fast-loop arms for jumps, equality and tests.
//! The generator defines one macro per opcode inside `run_fast`; it takes
//! the loop's locals and label as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_cmp_arms {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $pc:ident,
        $code:ident, $kptr:ident, $trace_on:ident, $pre53:ident, $entry_depth:ident,
        $frames:lifetime
    ) => {
        macro_rules! op_jmp {
            () => {{
                let off = $inst.sj();
                $npc = ($npc as i64 + off as i64) as u32;
                // a backward jump is a loop's back-edge: the trace JIT
                // counts them, and from the threshold on looks whether
                // to record from the target
                if $trace_on && off < 0 {
                    let proto = cl!().proto;
                    let c = proto.trace_hot_count.get();
                    if c < u32::MAX / 2 {
                        proto.trace_hot_count.set(c + 1);
                    }
                    let target = ($pc as i32 + 1 + off).max(0) as u32;
                    if $vm.jit.loop_hot_tick(&proto, target) && {
                        save!();
                        $vm.trace_start_at_back_edge(cl!(), base!(), target, None)
                    } {
                        // the recording sees the next instruction from
                        // the loop head
                        store_pc!();
                        return Ok(FastExit::Reload);
                    }
                }
                next_jumped!()
            }};
        }
        macro_rules! op_eq {
            () => {{
                let (pl, pr) = (
                    $regs.wrapping_add($inst.a() as usize),
                    $regs.wrapping_add($inst.b() as usize),
                );
                // SAFETY: registers of the running frame
                let (tl, tr) = unsafe { (raw_tag(pl), raw_tag(pr)) };
                // `__eq` is looked for only between two tables or two
                // full userdata
                let eq = if tl == tag::INT && tr == tag::INT {
                    // SAFETY: two integers
                    unsafe { raw_int(pl) == raw_int(pr) }
                } else if tl != tr || tl != tag::TABLE && tl != tag::USERDATA {
                    // SAFETY: as above
                    unsafe { (*pl).raw_eq(*pr) }
                } else {
                    // SAFETY: as above
                    let (l, r) = unsafe { (*pl, *pr) };
                    save!();
                    let step = $vm.eq_step(l, r);
                    $vm.op_compare(step, l, r, $inst.k())?;
                    resume!()
                };
                cond_jump!(eq == $inst.k())
            }};
        }
        // a constant is never a table or a userdata: no `__eq`
        macro_rules! op_eq_k {
            () => {{
                let (pl, pk) = (
                    $regs.wrapping_add($inst.a() as usize),
                    $kptr.wrapping_add($inst.b() as usize),
                );
                // SAFETY: a register and a constant of the running frame
                let eq = unsafe {
                    if raw_tag(pl) == tag::INT && raw_tag(pk) == tag::INT {
                        raw_int(pl) == raw_int(pk)
                    } else {
                        (*pl).raw_eq(*pk)
                    }
                };
                cond_jump!(eq == $inst.k())
            }};
        }
        // raw equality with a number: no metamethod can be involved
        macro_rules! op_eq_i {
            () => {{
                let px = $regs.wrapping_add($inst.a() as usize);
                let im = $inst.sb();
                // SAFETY: a register of the running frame
                let eq = unsafe {
                    match raw_tag(px) {
                        tag::INT => raw_int(px) == im as i64,
                        tag::FLOAT => raw_flt(px) == im as f64,
                        _ => false,
                    }
                };
                cond_jump!(eq == $inst.k())
            }};
        }
        // `R[A] op K[B]`, or `K[B] op R[A]` with `C` set
        macro_rules! order_k_arm {
            ($d op:tt, $d or_eq:expr) => {{
                let (pr, pk): (*const Value, *const Value) = (
                    $regs.wrapping_add($inst.a() as usize),
                    $kptr.wrapping_add($inst.b() as usize),
                );
                let (pl, pr) = if $inst.c() != 0 { (pk, pr) } else { (pr, pk) };
                order_arm!($d op, $d or_eq, pl, pr)
            }};
        }
        // two constants: `EqKK`, `LtKK`, `LeKK`
        macro_rules! order_kk_arm {
            ($d op:tt, $d or_eq:expr) => {{
                order_arm!($d op, $d or_eq, $kptr.wrapping_add($inst.a() as usize), $kptr.wrapping_add($inst.b() as usize))
            }};
        }
        macro_rules! op_eq_kk {
            () => {{
                // SAFETY: constants of the running frame, never a table or
                // a userdata
                let eq = unsafe {
                    (*$kptr.add($inst.a() as usize)).raw_eq(*$kptr.add($inst.b() as usize))
                };
                cond_jump!(eq == $inst.k())
            }};
        }
        macro_rules! op_test {
            () => {{
                // the JMP that follows runs when the condition equals k
                let t = reg_truthy!($inst.a());
                cond_jump!(t == $inst.k())
            }};
        }
        macro_rules! op_test_set {
            () => {{
                let pb = $regs.wrapping_add($inst.b() as usize);
                // SAFETY: registers of the running frame
                let t = unsafe {
                    let t = raw_truthy(pb) == $inst.k();
                    if t {
                        Value::copy_whole($regs.add($inst.a() as usize), pb);
                    }
                    t
                };
                cond_jump!(t)
            }};
        }
    };
}
pub(super) use fast_cmp_arms;
