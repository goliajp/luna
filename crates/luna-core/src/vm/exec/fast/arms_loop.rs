//! Fast-loop arms for the numeric and generic `for` loop back-edges.
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
        macro_rules! op_for_loop {
            () => {{
                let ra = $regs.wrapping_add($inst.a() as usize);
                let back = $npc.wrapping_sub($inst.bx());
                let mut slow = false;
                // SAFETY: the loop's four registers are in the frame
                // (the verifier checks the run)
                let (t0, t1, t2) =
                    unsafe { (raw_tag(ra), raw_tag(ra.add(1)), raw_tag(ra.add(2))) };
                if t0 == tag::INT && t1 == tag::INT && t2 == tag::INT {
                    // SAFETY: three integers; the index and the count
                    // or limit keep their tags, the control variable
                    // is the body's to change
                    unsafe {
                        let (cur, x, st) =
                            (raw_int(ra), raw_int(ra.add(1)), raw_int(ra.add(2)));
                        if !$pre53 {
                            if x != 0 {
                                let next = cur.wrapping_add(st);
                                put_int(ra, next);
                                put_int(ra.add(1), x.wrapping_sub(1));
                                ra.add(3).write(Value::Int(next));
                                $npc = back;
                            }
                        } else {
                            let next = cur.wrapping_add(st);
                            if if st > 0 { next <= x } else { next >= x } {
                                put_int(ra, next);
                                ra.add(3).write(Value::Int(next));
                                $npc = back;
                            }
                        }
                    }
                } else {
                    cold_path();
                    if t0 == tag::FLOAT && t1 == tag::FLOAT && t2 == tag::FLOAT {
                        // SAFETY: three floats
                        unsafe {
                            let (cur, lim, st) =
                                (raw_flt(ra), raw_flt(ra.add(1)), raw_flt(ra.add(2)));
                            let next = cur + st;
                            if if st > 0.0 { next <= lim } else { next >= lim } {
                                ra.write(Value::Float(next));
                                ra.add(3).write(Value::Float(next));
                                $npc = back;
                            }
                        }
                    } else {
                        // `for_loop` is the reference: 5.1–5.3 step and
                        // compare with the limit, 5.4+ count down;
                        // anything else it raises on
                        save!();
                        $vm.for_loop($inst, base!())?;
                        $npc = $vm.top_frame().pc;
                        slow = true;
                    }
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
                    if c == $vm.jit.trace_hot_threshold && $vm.jit.active_trace.is_none()
                    {
                        // the back-edge target is the body's first op
                        let target = ($pc as i32 + 1 - $inst.bx() as i32).max(0) as u32;
                        save!();
                        $vm.trace_start_at_loop(cl!(), base!(), target, None);
                        slow = true;
                    }
                }
                if slow {
                    // SAFETY: see `next!`
                    unsafe { (*$fr).pc = $npc };
                    return Ok(FastExit::Reload);
                }
                next!()
            }};
        }
        macro_rules! op_t_for_loop {
            () => {{
                let a = $inst.a();
                let pc4 = $regs.wrapping_add(a as usize + 4);
                // SAFETY: the loop's registers are in the frame
                if unsafe { raw_tag(pc4) } != tag::NIL {
                    // the generic-for's back-edge, counted like a
                    // numeric one; an iterator that returned nothing
                    // takes no back-edge
                    if $trace_on {
                        let proto = cl!().proto;
                        let c = proto.trace_hot_count.get();
                        if c < u32::MAX / 2 {
                            proto.trace_hot_count.set(c + 1);
                        }
                        if c == $vm.jit.trace_hot_threshold
                            && $vm.jit.active_trace.is_none()
                        {
                            // the body's first op, right after TForPrep
                            let target = ($pc as i32 + 1 - $inst.bx() as i32).max(0) as u32;
                            save!();
                            $vm.trace_start_at_loop(cl!(), base!(), target, Some(a));
                        }
                    }
                    // SAFETY: as above
                    unsafe { Value::copy_whole($regs.add(a as usize + 2), pc4) };
                    $npc = $npc.wrapping_sub($inst.bx());
                    // a recording that just started must see the next
                    // instruction from the loop head
                    if $trace_on && $vm.jit.active_trace.is_some() {
                        // SAFETY: see `next!`
                        unsafe { (*$fr).pc = $npc };
                        return Ok(FastExit::Reload);
                    }
                }
                next!()
            }};
        }
    };
}
pub(super) use fast_loop_arms;
