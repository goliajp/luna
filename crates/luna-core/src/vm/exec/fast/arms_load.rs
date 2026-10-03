//! Fast-loop arms for moves, loads, upvalues and table access.
//! The generator defines one macro per opcode inside `run_fast`; it takes
//! the loop's locals and label as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_load_arms {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $pc:ident,
        $code:ident, $kptr:ident, $trace_on:ident, $pre53:ident, $entry_depth:ident,
        $frames:lifetime
    ) => {
        macro_rules! op_move {
            () => {{
                // SAFETY: registers of the running frame
                unsafe {
                    Value::copy_raw(
                        $regs.add($inst.a() as usize),
                        $regs.add($inst.b() as usize),
                    )
                };
                next!()
            }};
        }
        macro_rules! op_load_i {
            () => {{
                set_reg!($inst.a(), Value::Int($inst.sbx() as i64));
                next!()
            }};
        }
        macro_rules! op_load_f {
            () => {{
                set_reg!($inst.a(), Value::Float($inst.sbx() as f64));
                next!()
            }};
        }
        macro_rules! op_load_k {
            () => {{
                // SAFETY: a register and a constant of the running
                // frame (see `konst!`)
                unsafe {
                    Value::copy_raw(
                        $regs.add($inst.a() as usize),
                        $kptr.add($inst.bx() as usize),
                    )
                };
                next!()
            }};
        }
        macro_rules! op_load_false {
            () => {{
                set_reg!($inst.a(), Value::Bool(false));
                next!()
            }};
        }
        macro_rules! op_l_false_skip {
            () => {{
                set_reg!($inst.a(), Value::Bool(false));
                $npc += 1;
                next!()
            }};
        }
        macro_rules! op_load_true {
            () => {{
                set_reg!($inst.a(), Value::Bool(true));
                next!()
            }};
        }
        macro_rules! op_load_nil {
            () => {{
                let a = $inst.a();
                for i in 0..=$inst.b() {
                    set_reg!(a + i, Value::Nil);
                }
                next!()
            }};
        }
        macro_rules! op_get_upval {
            () => {{
                let v = $vm.upval_get(cl!(), $inst.b());
                set_reg!($inst.a(), v);
                next!()
            }};
        }
        macro_rules! op_set_upval {
            () => {{
                let v = reg!($inst.a());
                $vm.upval_set(cl!(), $inst.b(), v);
                // the write may have gone through `self.stack`
                let base = base!();
                $regs = $vm.regs_at(base);
                next!()
            }};
        }
        macro_rules! op_get_tab_up {
            () => {{
                let t = $vm.upval_get(cl!(), $inst.b());
                let pk = $kptr.wrapping_add($inst.c() as usize);
                // SAFETY: a constant and a register of the running frame
                if unsafe { Vm::index_raw_kstr_key_at(t, pk, $regs.add($inst.a() as usize)) }
                {
                    next!()
                }
                save!();
                index_op_miss!()?;
                resume_same!()
            }};
        }
        macro_rules! op_get_field {
            () => {{
                get_arm!($kptr.wrapping_add($inst.c() as usize), index_raw_kstr_at)
            }};
        }
        macro_rules! op_get_i {
            () => {{
                let pt = $regs.wrapping_add($inst.b() as usize);
                let key = Value::Int($inst.c() as i64);
                // SAFETY: registers of the running frame
                if unsafe { Vm::index_raw_at(pt, &key, $regs.add($inst.a() as usize)) } {
                    next!()
                }
                save!();
                index_op_miss!()?;
                resume_same!()
            }};
        }
        macro_rules! op_set_tab_up {
            () => {{
                let t = $vm.upval_get(cl!(), $inst.a());
                let pk = $kptr.wrapping_add($inst.b() as usize);
                let pv = $regs.wrapping_add($inst.c() as usize);
                // SAFETY: a constant and a register of the running frame
                if unsafe { $vm.newindex_raw_key_at(t, pk, pv) } {
                    next!()
                }
                save!();
                newindex_op_miss!()?;
                resume_same!()
            }};
        }
        macro_rules! op_set_field {
            () => {{
                set_arm!($kptr.wrapping_add($inst.b() as usize), newindex_raw_kstr_at)
            }};
        }
        macro_rules! op_set_i {
            () => {{
                let pt = $regs.wrapping_add($inst.a() as usize);
                let key = Value::Int($inst.b() as i64);
                let pv = $regs.wrapping_add($inst.c() as usize);
                // SAFETY: registers of the running frame
                if unsafe { $vm.newindex_raw_at(pt, &key, pv) } {
                    next!()
                }
                save!();
                newindex_op_miss!()?;
                resume_same!()
            }};
        }
        macro_rules! op_self_op {
            () => {{
                let pb = $regs.wrapping_add($inst.b() as usize);
                let po = $regs.wrapping_add($inst.a() as usize + 1);
                // SAFETY: registers of the running frame
                unsafe { Value::copy_whole(po, pb) };
                // SAFETY: registers and constants of the running frame;
                // the object is read from its copy, `R[A]` is written last
                if unsafe { Vm::self_probe($regs, $kptr, $inst) } {
                    next!()
                }
                save!();
                let dst = base!() + $inst.a();
                // SAFETY: as above, worked out again (see `get_arm!`)
                unsafe {
                    let pk = self_key($regs, $kptr, $inst);
                    $vm.index_miss_at($regs.wrapping_add($inst.a() as usize + 1), pk, dst)
                }?;
                resume_same!()
            }};
        }
    };
}
pub(super) use fast_load_arms;
