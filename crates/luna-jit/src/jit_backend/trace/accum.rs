use super::*;

/// scan a trace for the `s = s .. v` 4-op idiom
/// emitted by Lua's frontend, then run an escape sweep over the
/// REAL accumulator slot.
///
/// The idiom:
/// ```text
///   pc i:   Move A=tmp   B=s_slot        ; load accumulator into temp
///   pc i+1: Move A=tmp+1 B=v_slot        ; load piece into temp+1
///   pc i+2: Concat A=tmp B=2             ; R[tmp] = R[tmp] .. R[tmp+1]
///   pc i+3: Move A=s_slot B=tmp          ; store accumulated back
/// ```
///
/// The `tmp` slot is local to the idiom; the REAL accumulator is
/// `s_slot` (the source of the first Move AND the destination of
/// the last Move). The escape sweep checks that `s_slot` has NO
/// uses outside this idiom in the trace body.
///
/// Algorithm:
/// 1. Walk `record.ops[..end]` for Op::Concat with `b() == 2` at
///    `inline_depth == 0`. For each, check the 3 surrounding ops
///    match the idiom.
/// 2. Filter call-triggered traces (need back-edge closure for the
///    accumulator-across-iterations semantic).
/// 3. For each idiom match, run an escape sweep over the trace body
///    — every reference to `s_slot` MUST be within the idiom (the
///    pre-Move src or post-Move dst) or the slot escapes.
/// 4. Return matched idioms as `AccumSite` entries.
pub(super) fn detect_accumulators(
    record: &TraceRecord,
    end: usize,
    _head_proto: Gc<Proto>,
) -> Vec<AccumSite> {
    use luna_core::vm::isa::Op;

    let upper = end.min(record.ops.len());
    // Need at least 4 ops to form the idiom.
    if upper < 4 {
        return Vec::new();
    }

    let candidates = accum_candidates(record, upper);
    if candidates.is_empty() {
        return Vec::new();
    }

    // Step 2: require the trace to be a back-edge loop (closed AND
    // not call-triggered) so the accumulator semantic ("survives
    // across iterations") holds. Call-triggered traces close on
    // re-entry of head_pc, which doesn't guarantee a loop semantic.
    if !record.closed || record.is_call_triggered {
        return Vec::new();
    }

    // Step 3: per-candidate escape sweep over body ops outside the
    // idiom. The 4 idiom op indices (pre1/pre2/concat/post) are
    // skipped — every other op must NOT reference the accumulator
    // slot, or it's NonBuffered.
    let mut out: Vec<AccumSite> = Vec::with_capacity(candidates.len());
    for (mut site, pre1, pre2, concat, post) in candidates {
        for i in 0..upper {
            if i == pre1 || i == pre2 || i == concat || i == post {
                continue;
            }
            let rop = &record.ops[i];
            if rop.inline_depth != 0 {
                continue;
            }
            let ins = rop.inst;
            let op = ins.op();
            let a = ins.a();
            let b = ins.b();
            let c = ins.c();
            let reads_slot = match op {
                Op::Move => b == site.accum_slot,
                Op::Add
                | Op::Sub
                | Op::Mul
                | Op::Div
                | Op::Mod
                | Op::Pow
                | Op::IDiv
                | Op::BAnd
                | Op::BOr
                | Op::BXor
                | Op::Shl
                | Op::Shr => b == site.accum_slot || c == site.accum_slot,
                Op::Unm | Op::BNot | Op::Not | Op::Len => b == site.accum_slot,
                Op::Eq | Op::Lt | Op::Le => a == site.accum_slot || b == site.accum_slot,
                Op::EqK | Op::Test | Op::TestSet => a == site.accum_slot,
                Op::Concat => {
                    let n = ins.b();
                    let end_op = a.saturating_add(n);
                    (a..end_op).any(|r| r == site.accum_slot)
                }
                Op::Call | Op::TailCall => {
                    let lo = a;
                    let hi = lo.saturating_add(b.max(1));
                    site.accum_slot >= lo && site.accum_slot < hi
                }
                Op::Return | Op::Return1 => a == site.accum_slot,
                Op::Return0 => false,
                Op::SetI | Op::SetTable | Op::SetField | Op::SetUpval | Op::SetTabUp => {
                    a == site.accum_slot || c == site.accum_slot
                }
                Op::GetI | Op::GetTable | Op::GetField | Op::GetTabUp | Op::GetUpval => {
                    b == site.accum_slot
                }
                Op::SetList => {
                    let lo = a;
                    let hi = lo.saturating_add(b.max(1));
                    site.accum_slot >= lo && site.accum_slot < hi
                }
                _ => false,
            };
            let writes_slot = match op {
                Op::LoadNil => {
                    let lo = a;
                    let hi = lo.saturating_add(b);
                    site.accum_slot >= lo && site.accum_slot <= hi
                }
                Op::Move
                | Op::LoadI
                | Op::LoadF
                | Op::LoadK
                | Op::LoadKx
                | Op::LoadFalse
                | Op::LFalseSkip
                | Op::LoadTrue
                | Op::GetUpval
                | Op::GetTabUp
                | Op::GetTable
                | Op::GetI
                | Op::GetField
                | Op::NewTable
                | Op::Add
                | Op::Sub
                | Op::Mul
                | Op::Div
                | Op::Mod
                | Op::Pow
                | Op::IDiv
                | Op::BAnd
                | Op::BOr
                | Op::BXor
                | Op::Shl
                | Op::Shr
                | Op::Unm
                | Op::BNot
                | Op::Not
                | Op::Len
                | Op::Concat
                | Op::Call
                | Op::TailCall
                | Op::TestSet
                | Op::ForLoop
                | Op::ForPrep
                | Op::TForCall => a == site.accum_slot,
                _ => false,
            };
            if reads_slot || writes_slot {
                site.state = BufferState::NonBuffered;
                break;
            }
        }
        out.push(site);
    }
    out
}

/// Step 1 of [`detect_accumulators`]: the idiom matches, each with its
/// four op indices (pre1, pre2, concat, post).
fn accum_candidates(
    record: &TraceRecord,
    upper: usize,
) -> Vec<(AccumSite, usize, usize, usize, usize)> {
    use luna_core::vm::isa::Op;

    // Step 1: idiom scan. For each Concat at index `ci`, check the
    // 3 surrounding ops match.
    let mut candidates: Vec<(AccumSite, usize, usize, usize, usize)> = Vec::new();
    for ci in 2..upper.saturating_sub(1) {
        let concat_rop = &record.ops[ci];
        if !matches!(concat_rop.inst.op(), Op::Concat) {
            continue;
        }
        if concat_rop.inline_depth != 0 {
            continue;
        }
        if concat_rop.inst.b() != 2 {
            continue;
        }
        let tmp = concat_rop.inst.a();

        // Pre-Move 1: Move A=tmp B=s_slot
        let pre1 = &record.ops[ci - 2];
        if pre1.inline_depth != 0 || !matches!(pre1.inst.op(), Op::Move) || pre1.inst.a() != tmp {
            continue;
        }
        let s_slot = pre1.inst.b();

        // Pre-Move 2: Move A=tmp+1 B=v_slot
        let pre2 = &record.ops[ci - 1];
        if pre2.inline_depth != 0 || !matches!(pre2.inst.op(), Op::Move) || pre2.inst.a() != tmp + 1
        {
            continue;
        }
        let v_slot = pre2.inst.b();

        // Post-Move: Move A=s_slot B=tmp
        let post = &record.ops[ci + 1];
        if post.inline_depth != 0
            || !matches!(post.inst.op(), Op::Move)
            || post.inst.a() != s_slot
            || post.inst.b() != tmp
        {
            continue;
        }

        // both the accumulator slot and the
        // piece slot must be Str at recorder-fire time for the
        // buffered emit to be sound. `luna_jit_str_buf_extend`
        // unconditionally interprets the raw bits as a
        // `*const LuaStr`; a non-Str payload (e.g. Int(1) =
        // raw=1) would dereference address 0x1 → SIGSEGV. The
        // dispatcher's entry-tag guard (`src/vm/exec.rs:~5124`)
        // ensures runtime tags match `record.entry_tags`, so
        // gating on Str-at-recorder-fire is sufficient. Covered by
        // `trace_ipairs_val_tag_guard::ipairs_mixed_tag_array_deopts
        // _no_garbage` with `{'a', 1, 'c'}`.
        let entry_tags = &record.entry_tags;
        let s_tag = entry_tags.get(s_slot as usize).copied();
        let v_tag = entry_tags.get(v_slot as usize).copied();
        if s_tag != Some(luna_core::runtime::value::raw::STR)
            || v_tag != Some(luna_core::runtime::value::raw::STR)
        {
            continue;
        }
        candidates.push((
            AccumSite {
                op_idx: ci,
                pc: concat_rop.pc,
                accum_slot: s_slot,
                piece_slot: v_slot,
                inline_depth: 0,
                state: BufferState::Bufferable,
            },
            ci - 2, // pre1 idx
            ci - 1, // pre2 idx
            ci,     // concat idx
            ci + 1, // post idx
        ));
    }
    candidates
}
