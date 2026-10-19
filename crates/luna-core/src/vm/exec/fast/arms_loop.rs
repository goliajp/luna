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
        // One step of a numeric `for` whose control registers at `$d ra`
        // all hold integers (`for_int_step`) or all floats
        // (`for_flt_step`), as `Vm::for_loop` steps it: 5.4 on count down,
        // 5.1-5.3 step and compare with the limit. Jumps to `$d back` while
        // the loop goes on. The index and the count or limit keep their
        // tags (the control variable is the body's to change).
        macro_rules! for_int_step {
            ($d ra:ident, $d back:expr, $d cur:expr, $d x:expr, $d st:expr) => {{
                let (cur, x, st): (i64, i64, i64) = ($d cur, $d x, $d st);
                if !$pre53 {
                    if x != 0 {
                        $npc = $d back;
                        let next = cur.wrapping_add(st);
                        put_int($d ra, next);
                        put_int($d ra.add(1), x.wrapping_sub(1));
                        put_words($d ra.add(3), tag::INT, next as u64);
                    }
                } else {
                    let next = cur.wrapping_add(st);
                    // as in `for_flt_step`
                    if st > 0 {
                        if next <= x {
                            put_int($d ra, next);
                            put_words($d ra.add(3), tag::INT, next as u64);
                            $npc = $d back;
                        }
                    } else if next >= x {
                        put_int($d ra, next);
                        put_words($d ra.add(3), tag::INT, next as u64);
                        $npc = $d back;
                    }
                }
            }};
        }
        // the payloads of the three control registers at `$d ra` as integers
        #[cfg(not(all(target_arch = "aarch64", not(miri))))]
        macro_rules! int_pays {
            ($d ra:ident) => {
                (raw_int($d ra), raw_int($d ra.add(1)), raw_int($d ra.add(2)))
            };
        }
        // One step of a float loop as PUC `floatforloop` takes it:
        // `0 < step ? next <= limit : limit <= next`. On aarch64 one
        // comparison of the pair the step's sign selects (`fcsel`), so that
        // the step has one branch and one store; elsewhere the two
        // directions are separate branches, which x86-64 does with fewer
        // instructions. `$d put` stores the next index.
        #[cfg(target_arch = "aarch64")]
        macro_rules! flt_step {
            ($d next:ident, $d lim:ident, $d st:ident, $d back:expr, $d put:expr) => {{
                let (lo, hi) = if $d st > 0.0 { ($d next, $d lim) } else { ($d lim, $d next) };
                if lo <= hi {
                    $npc = $d back;
                    $d put;
                }
            }};
        }
        #[cfg(not(target_arch = "aarch64"))]
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
        // True when the control registers at `$d ra`, the limit at offset
        // `$d lim`, hold a loop of kind `$d k`. On x86 and x86-64 a float loop
        // tests its index (or variable) tag and its step's whole first word
        // and not the limit's tag: the step only reads the limit, and
        // whatever changes it outside the loop's own code unseals the step
        // (see `num_words::unseal`). Elsewhere the three tags.
        // `$d tag` reads the first register's tag: the test of the other
        // kind reads it again (`raw_tag_again`), so that both reads stay
        // compares against memory
        #[cfg(all(any(target_arch = "x86_64", target_arch = "x86"), not(miri)))]
        macro_rules! kind_ok {
            ($d ra:ident, $d k:expr, $d lim:expr) => {
                kind_ok!($d ra, $d k, $d lim, raw_tag)
            };
            ($d ra:ident, $d k:expr, $d lim:expr, $d tag:ident) => {
                if $d k == tag::FLOAT {
                    let (idx, st) = if $d lim == 0 { (2, 1) } else { (0, 2) };
                    $d tag($d ra.add(idx)) == tag::FLOAT
                        && raw_word($d ra.add(st)) == tag_word(tag::FLOAT)
                } else {
                    $d tag($d ra) == $d k
                        && raw_tag($d ra.add(1)) == $d k
                        && raw_tag($d ra.add(2)) == $d k
                }
            };
        }
        #[cfg(not(any(target_arch = "aarch64", all(any(target_arch = "x86_64", target_arch = "x86"), not(miri)))))]
        macro_rules! kind_ok {
            ($d ra:ident, $d k:expr, $d lim:expr) => {{
                let _ = $d lim;
                raw_tag($d ra) == $d k && raw_tag($d ra.add(1)) == $d k && raw_tag($d ra.add(2)) == $d k
            }};
        }
        #[cfg(any(all(target_arch = "aarch64", miri), all(any(target_arch = "x86_64", target_arch = "x86"), miri)))]
        macro_rules! kind_ok_again {
            ($d ra:ident, $d k:expr, $d lim:expr) => {
                kind_ok!($d ra, $d k, $d lim)
            };
        }
        #[cfg(all(target_arch = "aarch64", miri))]
        macro_rules! kind_ok {
            ($d ra:ident, $d k:expr, $d lim:expr) => {{
                let _ = $d lim;
                raw_tag($d ra) == $d k && raw_tag($d ra.add(1)) == $d k && raw_tag($d ra.add(2)) == $d k
            }};
        }
        #[cfg(all(any(target_arch = "x86_64", target_arch = "x86"), not(miri)))]
        macro_rules! kind_ok_again {
            ($d ra:ident, $d k:expr, $d lim:expr) => {
                kind_ok!($d ra, $d k, $d lim, raw_tag_again)
            };
        }
        #[cfg(not(any(target_arch = "aarch64", target_arch = "x86_64", target_arch = "x86")))]
        macro_rules! kind_ok_again {
            ($d ra:ident, $d k:expr, $d lim:expr) => {
                kind_ok!($d ra, $d k, $d lim)
            };
        }
        // `$d hot` when the registers hold a loop of kind `$d h`, else
        // `$d other` when they hold one of kind `$d o` (see `kind_ok`);
        // false (the slow path) otherwise
        #[cfg(not(all(target_arch = "aarch64", not(miri))))]
        macro_rules! for_kinds {
            ($d ra:ident, $d lim:expr, $d h:expr, $d o:expr, $d hot:block, $d other:block) => {
                if kind_ok!($d ra, $d h, $d lim) {
                    $d hot;
                    true
                } else {
                    cold_path();
                    if kind_ok_again!($d ra, $d o, $d lim) {
                        $d other;
                        true
                    } else {
                        false
                    }
                }
            };
        }
        // `$d back` computed first, or on x86-64 again where it is used
        #[cfg(not(all(target_arch = "aarch64", not(miri))))]
        macro_rules! back_at {
            ($d back:ident) => {
                if cfg!(target_arch = "x86_64") { $npc.wrapping_sub($inst.bx()) } else { $d back }
            };
        }
        #[cfg(not(all(target_arch = "aarch64", not(miri))))]
        macro_rules! op_for_loop {
            ($d flt:expr) => {{
                // the kind of loop tested first
                let fl = DBL || $d flt;
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
                    let (hot, other) = if fl { (tag::FLOAT, tag::INT) } else { (tag::INT, tag::FLOAT) };
                    for_kinds!(ra, 1, hot, other, {
                        if fl {
                            for_flt_step!(ra, back_at!(back))
                        } else {
                            let (c, x, st) = int_pays!(ra);
                            for_int_step!(ra, back_at!(back), c, x, st)
                        }
                    }, {
                        if fl {
                            let (c, x, st) = int_pays!(ra);
                            for_int_step!(ra, back_at!(back), c, x, st)
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
        // the steps of a 5.5 loop, its registers' payloads read as the
        // kind the tests found
        macro_rules! int55_step {
            ($d ra:ident, $d back:expr, $d count:expr, $d st:expr, $d var:expr) => {{
                let count: i64 = $d count;
                if count != 0 {
                    $npc = $d back;
                    put_int($d ra, count.wrapping_sub(1));
                    // the variable is the index here, its tag tested above
                    put_int($d ra.add(2), ($d var as i64).wrapping_add($d st as i64));
                }
            }};
        }
        macro_rules! flt55_step {
            ($d ra:ident, $d back:expr) => {{
                let (lim, st, cur) = (raw_flt($d ra), raw_flt($d ra.add(1)), raw_flt($d ra.add(2)));
                let next = cur + st;
                flt_step!(next, lim, st, $d back, put_flt($d ra.add(2), next));
            }};
        }
        // 5.5: `R[A]` count (or limit), `R[A+1]` step, `R[A+2]` the
        // variable, which is the index. `$d flt`: floats first.
        #[cfg(not(all(target_arch = "aarch64", not(miri))))]
        macro_rules! op_for_loop55 {
            ($d flt:expr) => {{
                let ra = reg_a($regs, $inst);
                // as in `op_for_loop`
                let back = $npc.wrapping_sub($inst.bx());
                // SAFETY: the loop's three registers are in the frame (the
                // verifier checks the run); payloads are read as the type
                // their tags name
                let fast = unsafe {
                    if $d flt {
                        for_kinds!(ra, 0, tag::FLOAT, tag::INT, {
                            flt55_step!(ra, back_at!(back))
                        }, {
                            int55_step!(ra, back_at!(back), raw_int(ra), raw_int(ra.add(1)), raw_int(ra.add(2)))
                        })
                    } else {
                        for_kinds!(ra, 0, tag::INT, tag::FLOAT, {
                            int55_step!(ra, back_at!(back), raw_int(ra), raw_int(ra.add(1)), raw_int(ra.add(2)))
                        }, {
                            flt55_step!(ra, back_at!(back))
                        })
                    }
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
