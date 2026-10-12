//! Fast-loop arms for the numeric `for` loop's back-edge on aarch64, which
//! reads the control values as word pairs (see `num_words`). The generator
//! defines one macro per opcode inside `run_fast`, after `fast_loop_arms`,
//! whose step macros it uses; it takes the loop's locals and label as
//! arguments.

// rustfmt does not keep the nested macro bodies stable
#[rustfmt::skip]
macro_rules! fast_loop_a64_arms {
    (
        $d:tt, $vm:ident, $fr:ident, $regs:ident, $npc:ident, $inst:ident, $pc:ident,
        $code:ident, $kptr:ident, $trace_on:ident, $pre53:ident, $entry_depth:ident,
        $frames:lifetime
    ) => {
        // aarch64: each control register's first word and payload come in
        // one load, and the first words are tested whole (see `tag_word`):
        // three loads where the tags and payloads took six. A register
        // whose padding is not zero takes `Vm::for_loop` once.
        // A loop the compiler or the dialect makes a float loop: its first
        // words only (its payloads are read as floats where they are used,
        // not with them), and anything else to `Vm::for_loop`. The limit at
        // offset `$d lim` is only read, and its tag not tested: whatever
        // changes it outside the loop's own code unseals the step (see
        // `num_words::unseal`), whose whole first word is tested.
        #[cfg(all(target_arch = "aarch64", not(miri)))]
        macro_rules! float_first {
            ($d ra:ident, $d lim:expr, $d step:expr) => {{
                let w = raw_tag_words3($d ra);
                let f = tag_word(tag::FLOAT);
                let (a, b) = if $d lim == 0 { (w[1], w[2]) } else { (w[0], w[2]) };
                if (a == f) & (b == f) {
                    $d step;
                    true
                } else {
                    cold_path();
                    false
                }
            }};
        }
        #[cfg(all(target_arch = "aarch64", not(miri)))]
        macro_rules! op_for_loop {
            ($d flt:expr) => {{
                let ra = reg_a($regs, $inst);
                // as in the other `op_for_loop`
                // not where the trace JIT counts back-edges, whose reads of its
                // counters the barrier would keep from sharing
                let back = if $trace_on { $npc.wrapping_sub($inst.bx()) } else { first($npc.wrapping_sub($inst.bx())) };
                // SAFETY: the loop's four registers are in the frame (the
                // verifier checks the run); payloads are read as the type
                // their tags name
                let fast = unsafe {
                    if DBL {
                        // 5.1/5.2: the index's tag byte and the step's word
                        // (their loops mix numbers of both kinds; a limit of
                        // the other kind leaves the step unsealed, see
                        // `Vm::pad_for_state`), not three tag words, which
                        // cost a call-heavy loop backend stalls (`nc_select`
                        // 5.1 with the JIT on: 103.3 / 104.9 cycles per
                        // iteration)
                        if (raw_tag(ra) == tag::FLOAT) & (raw_word(ra.add(2)) == tag_word(tag::FLOAT)) {
                            for_flt_step!(ra, back);
                            true
                        } else {
                            cold_path();
                            false
                        }
                    } else if $d flt {
                        float_first!(ra, 1, for_flt_step!(ra, back))
                    } else {
                        let ([w0, w1, w2], [v0, v1, v2]) = raw_words3(ra);
                        let (i, f) = (tag_word(tag::INT), tag_word(tag::FLOAT));
                        if (w0 == i) & (w1 == i) & (w2 == i) {
                            for_int_step!(ra, back, v0 as i64, v1 as i64, v2 as i64);
                            true
                        } else {
                            cold_path();
                            // the limit's tag as in `float_first`
                            if (w0 == f) & (w2 == f) {
                                for_flt_step!(ra, back);
                                true
                            } else {
                                false
                            }
                        }
                    }
                };
                for_loop_tail!(fast)
            }};
        }
        #[cfg(all(target_arch = "aarch64", not(miri)))]
        macro_rules! op_for_loop55 {
            ($d flt:expr) => {{
                let ra = reg_a($regs, $inst);
                // not where the trace JIT counts back-edges, whose reads of its
                // counters the barrier would keep from sharing
                let back = if $trace_on { $npc.wrapping_sub($inst.bx()) } else { first($npc.wrapping_sub($inst.bx())) };
                // SAFETY: as for `op_for_loop`
                let fast = unsafe {
                    if $d flt {
                        float_first!(ra, 0, flt55_step!(ra, back))
                    } else {
                        let ([w0, w1, w2], [v0, v1, v2]) = raw_words3(ra);
                        let (i, f) = (tag_word(tag::INT), tag_word(tag::FLOAT));
                        // all three tags: with the step's untested, the
                        // branch no longer waited for its load, the next
                        // round's loads went ahead of this round's stores, and
                        // some code addresses made the Cortex-A76 replay them
                        // (`s = s + i`: 13.3 or 34.3 cycles per iteration by
                        // function order, 14.0 with the test)
                        if (w0 == i) & (w1 == i) & (w2 == i) {
                            int55_step!(ra, back, v0 as i64, v1, v2);
                            true
                        } else {
                            cold_path();
                            // the limit's tag as in `float_first`
                            if (w1 == f) & (w2 == f) {
                                flt55_step!(ra, back);
                                true
                            } else {
                                false
                            }
                        }
                    }
                };
                for_loop_tail!(fast)
            }};
        }
    };
}
pub(super) use fast_loop_a64_arms;
