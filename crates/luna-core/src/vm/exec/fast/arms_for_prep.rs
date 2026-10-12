//! Fast-loop arms for the numeric `for` loop's preparation.
//! The generator defines one macro per opcode inside `run_fast`; it takes
//! the loop's locals and label as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_for_prep_arms {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $pc:ident,
        $code:ident, $kptr:ident, $trace_on:ident, $pre53:ident, $entry_depth:ident,
        $frames:lifetime
    ) => {
        // Control values that need no conversion and cannot raise are
        // prepared here, as `Vm::for_prep` prepares them; the rest (a
        // string or mixed numbers, a zero step from 5.4 on, a non-number)
        // go to it. `$d off` is how far past the instruction after the
        // prepare the loop goes on: 0 into the body, `bx` past the loop,
        // `bx - 1` onto the step, where 5.1-5.3 test the first value. The
        // work stays inline: a call here changed the register allocation of
        // the call and table arms (method_dispatch +2.2% instructions).
        macro_rules! for_prep_tail {
            ($d off:expr) => {{
                match $d off {
                    Some(off) => {
                        $npc += off;
                        next!()
                    }
                    None => {
                        cold_path();
                        save!();
                        $vm.run_frame_op($inst)?;
                        resume!()
                    }
                }
            }};
        }
        // the dialect's own layout: `R[A]` index, `R[A+1]` limit (5.1-5.3)
        // or count, `R[A+2]` step, `R[A+3]` the variable. An integer loop
        // (5.3 on: integer start and step) whose limit is an integer, or
        // any loop over numbers that is a float loop (5.1/5.2 step every
        // loop in floating point).
        macro_rules! op_for_prep {
            () => {{
                let ra = $regs.wrapping_add($inst.a() as usize);
                let bx = $inst.bx();
                // SAFETY: the loop's four registers are in the frame (the
                // verifier checks the run); payloads are read as the type
                // their tags name, and a payload is written only over a
                // register already tagged with its type
                let off = unsafe {
                    let (t0, t1, t2) = (raw_tag(ra), raw_tag(ra.add(1)), raw_tag(ra.add(2)));
                    if !DBL && t0 == tag::INT && t2 == tag::INT {
                        let (init, step) = (raw_int(ra), raw_int(ra.add(2)));
                        if t1 != tag::INT {
                            None
                        } else if $pre53 {
                            put_words(ra, tag::INT, init.wrapping_sub(step) as u64);
                            put_words(ra.add(1), tag::INT, raw_int(ra.add(1)) as u64);
                            put_words(ra.add(2), tag::INT, step as u64);
                            Some(bx - 1)
                        } else if step == 0 {
                            None
                        } else if let Some(count) = int_for_count(init, raw_int(ra.add(1)), step) {
                            put_words(ra, tag::INT, init as u64);
                            put_words(ra.add(1), tag::INT, count);
                            put_words(ra.add(2), tag::INT, step as u64);
                            put_words(ra.add(3), tag::INT, init as u64);
                            Some(0)
                        } else {
                            Some(bx)
                        }
                    } else if is_num_tag(t0) && is_num_tag(t1) && is_num_tag(t2) {
                        let init = raw_num(ra, t0);
                        let limit = raw_num(ra.add(1), t1);
                        let step = raw_num(ra.add(2), t2);
                        if $pre53 {
                            put_words(ra, tag::FLOAT, (init - step).to_bits());
                            put_words(ra.add(1), tag::FLOAT, limit.to_bits());
                            put_words(ra.add(2), tag::FLOAT, step.to_bits());
                            Some(bx - 1)
                        } else if step == 0.0 {
                            None
                        } else if if 0.0 < step {
                            limit < init
                        } else {
                            init < limit
                        } {
                            Some(bx)
                        } else {
                            put_words(ra, tag::FLOAT, init.to_bits());
                            put_words(ra.add(1), tag::FLOAT, limit.to_bits());
                            put_words(ra.add(2), tag::FLOAT, step.to_bits());
                            ra.add(3).write(Value::Float(init));
                            Some(0)
                        }
                    } else {
                        None
                    }
                };
                for_prep_tail!(off)
            }};
        }
        // 5.5's layout, whatever the dialect: `R[A]` count (a float loop:
        // limit), `R[A+1]` step, `R[A+2]` the variable
        macro_rules! op_for_prep55 {
            () => {{
                let ra = $regs.wrapping_add($inst.a() as usize);
                let bx = $inst.bx();
                // SAFETY: as for `op_for_prep`
                let off = unsafe {
                    let (t0, t1, t2) = (raw_tag(ra), raw_tag(ra.add(1)), raw_tag(ra.add(2)));
                    if t0 == tag::INT && t2 == tag::INT {
                        let (init, step) = (raw_int(ra), raw_int(ra.add(2)));
                        if t1 != tag::INT || step == 0 {
                            None
                        } else if let Some(count) = int_for_count(init, raw_int(ra.add(1)), step) {
                            put_words(ra, tag::INT, count);
                            put_words(ra.add(1), tag::INT, step as u64);
                            put_words(ra.add(2), tag::INT, init as u64);
                            Some(0)
                        } else {
                            Some(bx)
                        }
                    } else if is_num_tag(t0) && is_num_tag(t1) && is_num_tag(t2) {
                        let init = raw_num(ra, t0);
                        let limit = raw_num(ra.add(1), t1);
                        let step = raw_num(ra.add(2), t2);
                        if step == 0.0 {
                            None
                        } else if if 0.0 < step {
                            limit < init
                        } else {
                            init < limit
                        } {
                            Some(bx)
                        } else {
                            put_words(ra, tag::FLOAT, limit.to_bits());
                            put_words(ra.add(1), tag::FLOAT, step.to_bits());
                            put_words(ra.add(2), tag::FLOAT, init.to_bits());
                            Some(0)
                        }
                    } else {
                        None
                    }
                };
                for_prep_tail!(off)
            }};
        }
    };
}
pub(super) use fast_for_prep_arms;
