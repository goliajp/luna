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
                    slow(l, r) => { save!(); $vm.arith_slow($inst.a(), base!(), ArithOp::$d aop, l, r) })
                {
                    next!()
                }
                resume_same!()
            }};
        }
        // `R[A] := R[B] op K[C]` of a commutative operator; `k`: the
        // constant was on the left, which only the metamethod sees
        macro_rules! arith_rk {
            ($d aop:ident, int($d ia:ident, $d ib:ident) => $d iv:expr, float($d fa:ident, $d fb:ident) => $d fv:expr) => {{
                if arith_arm!($regs, $inst, $regs.wrapping_add($inst.b() as usize), $kptr.wrapping_add($inst.c() as usize),
                    int($d ia, $d ib) => $d iv, float($d fa, $d fb) => $d fv,
                    slow(x, c) => {
                        save!();
                        let (l, r) = if $inst.k() { (c, x) } else { (x, c) };
                        $vm.arith_slow($inst.a(), base!(), ArithOp::$d aop, l, r)
                    })
                {
                    next!()
                }
                resume_same!()
            }};
        }
        // `R[A] := R[B] op K[C]`, or `K[C] op R[B]` with `k` set
        macro_rules! arith_rk_ordered {
            ($d aop:ident, int($d ia:ident, $d ib:ident) => $d iv:expr, float($d fa:ident, $d fb:ident) => $d fv:expr) => {{
                let (pb, pk): (*const Value, *const Value) = ($regs.wrapping_add($inst.b() as usize), $kptr.wrapping_add($inst.c() as usize));
                let (pl, pr) = if $inst.k() { (pk, pb) } else { (pb, pk) };
                if arith_arm!($regs, $inst, pl, pr,
                    int($d ia, $d ib) => $d iv, float($d fa, $d fb) => $d fv,
                    slow(l, r) => { save!(); $vm.arith_slow($inst.a(), base!(), ArithOp::$d aop, l, r) })
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
                        $vm.arith_slow($inst.a(), base!(), ArithOp::$d aop, l, r)
                    })
                {
                    next!()
                }
                resume_same!()
            }};
        }
        // the rest of a `GetI` / `GetTabUp` read and of a table write,
        // past the probe in their arm
        macro_rules! index_op_miss {
            () => {
                // SAFETY: `fr`, `regs` and `kptr` are the running frame's
                unsafe { $vm.index_op_miss($inst, $regs, $kptr, $fr) }
            };
        }
        macro_rules! newindex_op_miss {
            () => {
                // SAFETY: `fr` is the running frame
                unsafe { $vm.newindex_op_miss($inst, $fr) }
            };
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
        // the value a table write stores: `K[C]` with `k` set, else `R[C]`
        macro_rules! store_value {
            () => {
                if $inst.k() {
                    $kptr.wrapping_add($inst.c() as usize)
                } else {
                    $regs.wrapping_add($inst.c() as usize)
                }
            };
        }
        // `R[A][*pk] := R[C]/K[C]` for a key in a register or a constant
        macro_rules! set_arm {
            ($d pk:expr, $d probe:ident) => {{
                let pt = $regs.wrapping_add($inst.a() as usize);
                let pk: *const Value = $d pk;
                let pv = store_value!();
                // SAFETY: registers and a constant of the running frame;
                // the value is read where it is (see `Value::copy_raw`)
                if unsafe { $vm.$d probe(pt, pk, pv) } {
                    next!()
                }
                save!();
                newindex_op_miss!()?;
                resume_same!()
            }};
        }
        // `R[A] < R[B]` / `<=` (PUC `op_order`): two integers or two
        // floats here, the rest (mixed numbers, strings, `__lt` / `__le`)
        // by `less_step`
        macro_rules! order_arm {
            ($d op:tt, $d or_eq:expr) => {
                order_arm!($d op, $d or_eq, $regs.wrapping_add($inst.a() as usize), $regs.wrapping_add($inst.b() as usize))
            };
            ($d op:tt, $d or_eq:expr, $d pl:expr, $d pr:expr) => {{
                let (pl, pr): (*const Value, *const Value) = ($d pl, $d pr);
                // SAFETY: registers or constants of the running frame, so initialised
                // values; a payload is read as the type its tag names
                let res = unsafe {
                    let (tl, tr) = (raw_tag(pl), raw_tag(pr));
                    if tl == tag::INT && tr == tag::INT {
                        Some(raw_int(pl) $d op raw_int(pr))
                    } else {
                        cold_path();
                        if tl == tag::FLOAT && tr == tag::FLOAT {
                            Some(raw_flt(pl) $d op raw_flt(pr))
                        } else {
                            None
                        }
                    }
                };
                let res = match res {
                    Some(res) => res,
                    None => {
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
                // SAFETY: a register of the running frame, so an
                // initialised value; its payload is read as the type its
                // tag names
                let res = unsafe {
                    let t = raw_tag(px);
                    if t == tag::INT {
                        Some(raw_int(px) $d op (im as i64))
                    } else {
                        cold_path();
                        if t == tag::FLOAT {
                            Some(raw_flt(px) $d op (im as f64))
                        } else {
                            None
                        }
                    }
                };
                let res = match res {
                    Some(res) => res,
                    None => {
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
