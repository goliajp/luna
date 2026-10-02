//! Shapes several fast-loop opcodes share: arithmetic on registers and
//! constants, table reads and writes, ordered comparisons. The generator
//! defines the macros inside `run_fast`; it takes the loop's locals as
//! arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_arm_helper_macros {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $code:ident,
        $kptr:ident, $trace_on:ident, $entry_depth:ident, $frames:lifetime
    ) => {
        // `R[A] := R[B] op R[C]` (see `arith_arm`)
        macro_rules! arith_rr {
            ($d aop:ident, int($d ia:ident, $d ib:ident) => $d iv:expr, float($d fa:ident, $d fb:ident) => $d fv:expr) => {{
                if arith_arm!($regs, $inst, $regs.wrapping_add($inst.b() as usize), $regs.wrapping_add($inst.c() as usize),
                    int($d ia, $d ib) => $d iv, float($d fa, $d fb) => $d fv,
                    slow(l, r) => { save!(); $vm.arith_slow($inst.a(), base!(), ArithOp::$d aop, l, r, $inst.k()) })
                {
                    next!()
                }
                resume_same!()
            }};
        }
        // `R[A] := R[B] op K[C]`; `k`: the constant was on the left
        macro_rules! arith_rk {
            ($d aop:ident, int($d ia:ident, $d ib:ident) => $d iv:expr, float($d fa:ident, $d fb:ident) => $d fv:expr) => {{
                if arith_arm!($regs, $inst, $regs.wrapping_add($inst.b() as usize), $kptr.wrapping_add($inst.c() as usize),
                    int($d ia, $d ib) => $d iv, float($d fa, $d fb) => $d fv,
                    slow(x, c) => {
                        save!();
                        let (l, r) = if $inst.k() { (c, x) } else { (x, c) };
                        $vm.arith_slow($inst.a(), base!(), ArithOp::$d aop, l, r, false)
                    })
                {
                    next!()
                }
                resume_same!()
            }};
        }
        // `R[A] := R[B] op sC`; `k`: the immediate was on the left
        macro_rules! arith_ri {
            ($d aop:ident, int($d ia:ident, $d ib:ident) => $d iv:expr, float($d fa:ident, $d fb:ident) => $d fv:expr) => {{
                if arith_imm_arm!($regs, $inst, $regs.wrapping_add($inst.b() as usize), $inst.sc() as i64,
                    int($d ia, $d ib) => $d iv, float($d fa, $d fb) => $d fv,
                    slow(x, c) => {
                        save!();
                        let (l, r) = if $inst.k() { (c, x) } else { (x, c) };
                        $vm.arith_slow($inst.a(), base!(), ArithOp::$d aop, l, r, false)
                    })
                {
                    next!()
                }
                resume_same!()
            }};
        }
        // `R[A] := R[B][*pk]` for a key in a register or a constant
        macro_rules! get_arm {
            ($d pk:expr, $d probe:ident) => {{
                let pt = $regs.wrapping_add($inst.b() as usize);
                let pk: *const Value = $d pk;
                // SAFETY: a register and a register or constant of the
                // running frame
                if unsafe { Vm::$d probe(pt, pk, $regs.add($inst.a() as usize)) } {
                    next!()
                }
                save!();
                let dst = base!() + $inst.a();
                // SAFETY: as above; the pointers are worked out again
                // here so that none of them stays live across the probe
                unsafe { $vm.index_miss_at($regs.wrapping_add($inst.b() as usize), $d pk, dst) }?;
                resume_same!()
            }};
        }
        // `R[A][*pk] := R[C]` for a key in a register or a constant
        macro_rules! set_arm {
            ($d pk:expr, $d probe:ident) => {{
                let pt = $regs.wrapping_add($inst.a() as usize);
                let pk: *const Value = $d pk;
                let pv = $regs.wrapping_add($inst.c() as usize);
                // SAFETY: registers and a constant of the running frame;
                // the value is read where it is (see `Value::copy_raw`)
                if unsafe { $vm.$d probe(pt, pk, pv) } {
                    next!()
                }
                save!();
                $vm.newindex_op_miss($inst, $fr)?;
                resume_same!()
            }};
        }
        // `R[A] < R[B]` / `<=` (PUC `op_order`): two integers or two
        // floats here, the rest (mixed numbers, strings, `__lt` / `__le`)
        // by `less_step`
        macro_rules! order_arm {
            ($d op:tt, $d or_eq:expr) => {{
                let (pl, pr) = (
                    $regs.wrapping_add($inst.a() as usize),
                    $regs.wrapping_add($inst.b() as usize),
                );
                // SAFETY: registers of the running frame
                let (tl, tr) = unsafe { (raw_tag(pl), raw_tag(pr)) };
                let res = if tl == tag::INT && tr == tag::INT {
                    // SAFETY: two integers
                    (unsafe { raw_int(pl) }) $d op (unsafe { raw_int(pr) })
                } else {
                    cold_path();
                    if tl == tag::FLOAT && tr == tag::FLOAT {
                        // SAFETY: two floats
                        (unsafe { raw_flt(pl) }) $d op (unsafe { raw_flt(pr) })
                    } else {
                        // SAFETY: as above
                        let (l, r) = unsafe { (*pl, *pr) };
                        save!();
                        let step = $vm.less_step(l, r, $d or_eq)?;
                        $vm.op_compare(step, l, r, $inst.k())?;
                        resume!()
                    }
                };
                cond_jump!(res == $inst.k())
            }};
        }
        // `R[A] op sB` (PUC `op_orderI`); `$swap`: the immediate is the
        // left operand of the metamethod (`>` and `>=`), and `C` says it
        // was written as a float
        macro_rules! order_imm_arm {
            ($d op:tt, $d swap:expr, $d or_eq:expr) => {{
                let px = $regs.wrapping_add($inst.a() as usize);
                let im = $inst.sb();
                // SAFETY: a register of the running frame
                let t = unsafe { raw_tag(px) };
                let res = if t == tag::INT {
                    // SAFETY: an integer
                    (unsafe { raw_int(px) }) $d op (im as i64)
                } else {
                    cold_path();
                    if t == tag::FLOAT {
                        // SAFETY: a float
                        (unsafe { raw_flt(px) }) $d op (im as f64)
                    } else {
                        // SAFETY: as above
                        let x = unsafe { *px };
                        let imv = if $inst.c() != 0 {
                            Value::Float(im as f64)
                        } else {
                            Value::Int(im as i64)
                        };
                        let (l, r) = if $d swap { (imv, x) } else { (x, imv) };
                        save!();
                        let step = $vm.less_step(l, r, $d or_eq)?;
                        $vm.op_compare(step, l, r, $inst.k())?;
                        resume!()
                    }
                };
                cond_jump!(res == $inst.k())
            }};
        }
    };
}
pub(super) use fast_arm_helper_macros;
