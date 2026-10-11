//! Fast-loop arms for the numeric `for` loop's preparation and the
//! numeric and generic `for` loop back-edges.
//! The generator defines one macro per opcode inside `run_fast`; it takes
//! the loop's locals and label as arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_loop_arms {
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
                            put_int(ra, init.wrapping_sub(step));
                            Some(bx - 1)
                        } else if step == 0 {
                            None
                        } else if let Some(count) = int_for_count(init, raw_int(ra.add(1)), step) {
                            put_int(ra.add(1), count as i64);
                            ra.add(3).write(Value::Int(init));
                            Some(0)
                        } else {
                            Some(bx)
                        }
                    } else if is_num_tag(t0) && is_num_tag(t1) && is_num_tag(t2) {
                        let init = raw_num(ra, t0);
                        let limit = raw_num(ra.add(1), t1);
                        let step = raw_num(ra.add(2), t2);
                        if $pre53 {
                            ra.write(Value::Float(init - step));
                            ra.add(1).write(Value::Float(limit));
                            ra.add(2).write(Value::Float(step));
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
                            ra.write(Value::Float(init));
                            ra.add(1).write(Value::Float(limit));
                            ra.add(2).write(Value::Float(step));
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
                            put_int(ra, count as i64);
                            put_int(ra.add(1), step);
                            put_int(ra.add(2), init);
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
                            ra.write(Value::Float(limit));
                            ra.add(1).write(Value::Float(step));
                            ra.add(2).write(Value::Float(init));
                            Some(0)
                        }
                    } else {
                        None
                    }
                };
                for_prep_tail!(off)
            }};
        }
        // One step of a numeric `for` whose control registers at `$d ra`
        // all hold integers (`for_int_step`) or all floats
        // (`for_flt_step`), as `Vm::for_loop` steps it: 5.4 on count down,
        // 5.1-5.3 step and compare with the limit. Jumps to `$d back` while
        // the loop goes on. The index and the count or limit keep their
        // tags (the control variable is the body's to change).
        macro_rules! for_int_step {
            ($d ra:ident, $d back:expr) => {{
                let (cur, x, st) = (raw_int($d ra), raw_int($d ra.add(1)), raw_int($d ra.add(2)));
                if !$pre53 {
                    if x != 0 {
                        $npc = $d back;
                        let next = cur.wrapping_add(st);
                        put_int($d ra, next);
                        put_int($d ra.add(1), x.wrapping_sub(1));
                        $d ra.add(3).write(Value::Int(next));
                    }
                } else {
                    let next = cur.wrapping_add(st);
                    // as in `for_flt_step`
                    if st > 0 {
                        if next <= x {
                            put_int($d ra, next);
                            $d ra.add(3).write(Value::Int(next));
                            $npc = $d back;
                        }
                    } else if next >= x {
                        put_int($d ra, next);
                        $d ra.add(3).write(Value::Int(next));
                        $npc = $d back;
                    }
                }
            }};
        }
        // One step of a float loop as PUC `floatforloop` takes it:
        // `0 < step ? next <= limit : limit <= next`. The two directions
        // are separate branches, so that the compiler does not merge them
        // into a select of both tests. `$d put` stores the next index.
        macro_rules! flt_step {
            ($d next:ident, $d lim:ident, $d st:ident, $d back:expr, $d put:expr) => {
                if $d st > 0.0 {
                    if $d next <= $d lim {
                        $npc = $d back;
                        $d put;
                    }
                } else if $d lim <= $d next {
                    $npc = $d back;
                    $d put;
                }
            };
        }
        macro_rules! for_flt_step {
            ($d ra:ident, $d back:expr) => {{
                let (cur, lim, st) = (raw_flt($d ra), raw_flt($d ra.add(1)), raw_flt($d ra.add(2)));
                let next = cur + st;
                flt_step!(next, lim, st, $d back, {
                    put_flt($d ra, next);
                    $d ra.add(3).write(Value::Float(next));
                })
            }};
        }
        // `for_prep` leaves the three registers all integers or all floats;
        // anything else (`debug.setlocal`, crafted bytecode) goes to
        // `Vm::for_loop`, which raises. 5.1/5.2 loops are float loops.
        // True after running `$d hot` when the three control registers at
        // `$d ra` all have the tag `$d h`, or `$d other` when they all have
        // `$d o`; false (the slow path) otherwise. On x86-64 each tag is
        // read once on each path, so that the tests compare memory: the
        // other kind reads the first tag again rather than reuse the hot
        // test's read. Elsewhere the three tags are read once and tested
        // together, which aarch64 does with conditional compares.
        macro_rules! for_kinds {
            ($d ra:ident, $d h:expr, $d o:expr, $d hot:block, $d other:block) => {
                if cfg!(target_arch = "x86_64") {
                    if raw_tag($d ra) == $d h {
                        if raw_tag($d ra.add(1)) == $d h && raw_tag($d ra.add(2)) == $d h {
                            $d hot;
                            true
                        } else {
                            false
                        }
                    } else {
                        cold_path();
                        if raw_tag_again($d ra) == $d o
                            && raw_tag($d ra.add(1)) == $d o
                            && raw_tag($d ra.add(2)) == $d o
                        {
                            $d other;
                            true
                        } else {
                            false
                        }
                    }
                } else {
                    let (t0, t1, t2) = (raw_tag($d ra), raw_tag($d ra.add(1)), raw_tag($d ra.add(2)));
                    if (t0 == $d h) & (t1 == $d h) & (t2 == $d h) {
                        $d hot;
                        true
                    } else {
                        cold_path();
                        if (t0 == $d o) & (t1 == $d o) & (t2 == $d o) {
                            $d other;
                            true
                        } else {
                            false
                        }
                    }
                }
            };
        }
        // `$d back` computed first, or on x86-64 again where it is used
        macro_rules! back_at {
            ($d back:ident) => {
                if cfg!(target_arch = "x86_64") { $npc.wrapping_sub($inst.bx()) } else { $d back }
            };
        }
        macro_rules! op_for_loop {
            () => {{
                let ra = reg_a($regs, $inst);
                // the back-edge's target: on aarch64 the next fetch waits for
                // it, and computed first it is ready by then (fl_empty 5.5
                // 9.7 -> 8.2 cycles per iteration on a Cortex-A76); x86-64
                // computes it where it is used, one instruction less
                let back = $npc.wrapping_sub($inst.bx());
                // SAFETY: the loop's four registers are in the frame (the
                // verifier checks the run); payloads are read as the type
                // their tags name
                let fast = unsafe {
                    // the dialect's loops first, the other kind out of line
                    let (hot, other) = if DBL { (tag::FLOAT, tag::INT) } else { (tag::INT, tag::FLOAT) };
                    for_kinds!(ra, hot, other, {
                        if DBL {
                            for_flt_step!(ra, back_at!(back))
                        } else {
                            for_int_step!(ra, back_at!(back))
                        }
                    }, {
                        if DBL {
                            for_int_step!(ra, back_at!(back))
                        } else {
                            for_flt_step!(ra, back_at!(back))
                        }
                    })
                };
                for_loop_tail!(fast)
            }};
        }
        // the end of a numeric `for` step: the slow path for anything the
        // fast one did not take, and the trace JIT's back-edge count
        macro_rules! for_loop_tail {
            ($d fast:ident) => {{
                let fast = $d fast;
                let mut slow = false;
                if !fast {
                    // `for_loop` is the reference: 5.1–5.3 step and compare
                    // with the limit, 5.4+ count down; anything else it
                    // raises on
                    save!();
                    $vm.for_loop($inst, base!())?;
                    $npc = $vm.top_frame().pc;
                    slow = true;
                }
                // The trace JIT counts the back-edges taken and starts
                // recording at the body once the count reaches the
                // threshold.
                if $trace_on && $npc != $pc + 1 {
                    let proto = cl!().proto;
                    let c = proto.trace_hot_count.get();
                    if c < u32::MAX / 2 {
                        proto.trace_hot_count.set(c + 1);
                    }
                    // the back-edge target is the body's first op
                    let target = ($pc as i32 + 1 - $inst.bx() as i32).max(0) as u32;
                    if $vm.jit.loop_hot_tick(&proto, target) {
                        save!();
                        slow |= $vm.trace_start_at_back_edge(cl!(), base!(), target, None);
                    }
                }
                if slow {
                    store_pc!();
                    return Ok(FastExit::Reload);
                }
                next_jumped!()
            }};
        }
        // 5.5: `R[A]` count (or limit), `R[A+1]` step, `R[A+2]` the
        // variable, which is the index
        macro_rules! op_for_loop55 {
            () => {{
                let ra = reg_a($regs, $inst);
                // as in `op_for_loop`
                let back = $npc.wrapping_sub($inst.bx());
                // SAFETY: the loop's three registers are in the frame (the
                // verifier checks the run); payloads are read as the type
                // their tags name
                let fast = unsafe {
                    for_kinds!(ra, tag::INT, tag::FLOAT, {
                        let count = raw_int(ra);
                        if count != 0 {
                            $npc = back_at!(back);
                            put_int(ra, count.wrapping_sub(1));
                            // the variable is the index here, its tag tested
                            // above
                            put_int(ra.add(2), raw_int(ra.add(2)).wrapping_add(raw_int(ra.add(1))));
                        }
                    }, {
                        let (lim, st, cur) = (raw_flt(ra), raw_flt(ra.add(1)), raw_flt(ra.add(2)));
                        let next = cur + st;
                        flt_step!(next, lim, st, back_at!(back), put_flt(ra.add(2), next));
                    })
                };
                for_loop_tail!(fast)
            }};
        }
        // `$var`: the first variable's register from the base; `$copy`:
        // it is copied into the control register (`ForLayout`)
        macro_rules! op_t_for_loop {
            ($d var:expr, $d copy:expr) => {{
                let a = $inst.a();
                let pc4 = $regs.wrapping_add(a as usize + $d var);
                // SAFETY: the loop's registers are in the frame
                if unsafe { raw_tag(pc4) } != tag::NIL {
                    // the control variable takes the new key first: a
                    // recording started below snapshots the registers
                    // as the body will see them
                    if $d copy {
                        // SAFETY: as above
                        unsafe { Value::copy_whole($regs.add(a as usize + 2), pc4) };
                    }
                    // the generic-for's back-edge, counted like a
                    // numeric one; an iterator that returned nothing
                    // takes no back-edge
                    if $trace_on {
                        let proto = cl!().proto;
                        let c = proto.trace_hot_count.get();
                        if c < u32::MAX / 2 {
                            proto.trace_hot_count.set(c + 1);
                        }
                        // the body's first op, right after TForPrep
                        let target = ($pc as i32 + 1 - $inst.bx() as i32).max(0) as u32;
                        if $vm.jit.loop_hot_tick(&proto, target) {
                            save!();
                            $vm.trace_start_at_back_edge(cl!(), base!(), target, Some((a, $d var as u32)));
                        }
                    }
                    $npc = $npc.wrapping_sub($inst.bx());
                    // a recording that just started must see the next
                    // instruction from the loop head
                    if $trace_on && $vm.jit.active_trace.is_some() {
                        store_pc!();
                        return Ok(FastExit::Reload);
                    }
                }
                next_jumped!()
            }};
        }
    };
}
pub(super) use fast_loop_arms;
