use super::*;

/// The emit-pass state at the loop head.
#[allow(clippy::too_many_arguments)]
pub(super) fn begin_body<E: Emit>(
    bcx: E,
    pl: &Plan<'_>,
    h: Helpers,
    head: EntryRegs,
    escape: EscapeAnalysis,
    defined_aot_data: std::collections::HashSet<DataId>,
    sunk: (Vec<Option<Vec<Variable>>>, Vec<Option<Vec<RegKind>>>, u32),
    flush_ctx: Option<FlushCtx>,
    blocks: (Option<Block>, Block),
) -> Lower<E> {
    let EntryRegs {
        reg_state,
        trace_fn_sig_ref,
        global_side_trace_box,
        regs_full,
        tforcall_tag_var,
        tforcall_val_tag_var,
    } = head;
    let (virt_vars, virt_kinds, sunk_alloc_seen) = sunk;
    let (precheck, body_loop) = blocks;
    let Plan {
        record,
        max_stack,
        window_size_us,
        ..
    } = *pl;
    let head_live = &pl.head_live;
    // What reg_state holds for each register at the loop head: on entry
    // the values the prelude loaded (caller window) or the zeroes the
    // dispatcher filled it with (inline frames); on the back-edge what
    // `sync_reg_state` wrote before the jump. Read at the loop head below.
    let stored: Vec<Option<Value>> = Vec::new();

    // Per-reg current kind: the recorded entry tag for a head-frame
    // register the trace reads before writing (the dispatcher checks
    // it), held on the stack for the other head-frame registers, and
    // Unset past the head frame (the dispatcher zero-initialises those
    // reg_state slots and trace IR fills them via writers). Sized to
    // `window_size_us` (mirrors `regs_full`).
    let current_kinds: Vec<RegKind> = (0..window_size_us)
        .map(|i| match head_live.get(i) {
            Some(true) => record
                .entry_tags
                .get(i)
                .and_then(|&t| RegKind::from_entry_tag(t))
                .unwrap_or(RegKind::Unset),
            Some(false) => RegKind::StackHeld,
            None => RegKind::Unset,
        })
        .collect();
    // The kinds the body is lowered for. A back-edge may run it again
    // only when the caller window holds these same kinds (see
    // `loop_kinds_match`); otherwise
    // the next pass would read (and hand to an exit) a register with
    // bits of one kind as another, e.g. an Int as a Float.
    let head_kinds: Vec<RegKind> = current_kinds[..max_stack].to_vec();
    let dispatchable: bool = true;
    // the first emit-pass site that flips
    // dispatchable to false wins this label; CompiledTrace
    // exposes it via `dispatch_off_reason` for probe diagnostics.
    let dispatch_off_reason: Option<&'static str> = None;
    // per-side-exit RegKind snapshot. Pushed at each
    // true side-exit emit site (Lt/Le/Eq + Jmp) so later writers
    // (e.g. `Op::GetUpval` whose result we infer as `Closure`) don't
    // pollute the side-exit's restore with a tag the slot hasn't
    // actually become at that exit. The clean-tail and call-truncation
    // paths reuse the final `current_kinds` via `ct.exit_tags`.
    // 3rd element is the per-entry `Box<Cell<*const
    // u8>>` whose heap address is baked into the corresponding
    // emit_store_back_and_return_pc callsite. Allocated at each push
    // site BEFORE the helper call so the IR's `iconst`-baked address
    // exists. Transported through into `tags_side_trace_ptrs` at the
    // end of emit (the cell never moves).
    let per_exit_kinds: Vec<(u32, Vec<RegKind>, Box<TCellPtr>)> = Vec::new();
    // per inline cmp@d>0 side-exit. Each entry
    // is built at the cmp emit site and includes the side-exit PC,
    // a window-sized exit-tag snapshot, and the frame-mat chain. The
    // IR encodes `(site_idx + 1)` in the upper 32 bits of the
    // returned i64 so the dispatcher can pick the right entry
    // without colliding on shared cont_pc values (fib's cmp@d=0
    // through cmp@d=4 all side-exit to the same PC).
    // 5th element is the per-site `Box<Cell<*const
    // u8>>` whose heap address is baked into the IR's
    // `emit_store_back_and_return_site` gate. Allocated at each push
    // site BEFORE the helper call (address is
    // stable across `Vec → Rc<[]>` moves because Box transfers
    // ownership without moving the heap cell).
    let per_exit_inline_vec: Vec<(
        u32,
        u32,
        Vec<RegKind>,
        TArc<[FrameMaterializeInfo]>,
        Box<TCellPtr>,
    )> = Vec::new();
    // Live call stack mirror — push on self-recursive `Op::Call`,
    // pop on `Op::Return0/1` at depth>0. Each frame's `base_offset`
    // and `pc` (= caller's Call.pc + 1) are stamped at push time;
    // when snapshotting at a cmp@d>0 site, the innermost frame's
    // `pc` is overwritten with the actual side-exit PC so the helper
    // pushes the right resume point without needing a dispatcher
    // post-hoc fix-up.
    let call_chain: Vec<FrameMaterializeInfo> = Vec::new();

    // --- emit body
    //
    // Cranelift's `FunctionBuilder` tracks the "current" block
    // internally; every `bcx.ins()` emits into whichever block was
    // last `switch_to_block`'d. A cmp's `brif` forks the current
    // block to a `continue_blk` and a `side_exit_blk`; after
    // emitting the side-exit and switching back to `continue_blk`,
    // subsequent ops land there. By the end of the loop the
    // "current" block is whatever the last cmp's continue branch
    // pointed at (or the entry block if no cmps fired).
    //
    // Only the *normal* range (`record.ops[..effective_end]`) is
    // emitted. If `Op::Call` truncates the trace, the tail emits
    // a side-exit at the Call's PC instead of the head_pc close.
    // Memoize GetUpval(idx) per dispatch.
    // For self-recursive traces (fib, factorial, etc.), the trace head
    // is entered with one closure and `JIT_CL` stays pinned to it for
    // the entire dispatch; all inlined-depth GetUpval(idx) calls return
    // the same value. Hoist the helper call to the first occurrence and
    // reuse the cached SSA value at later sites (in cranelift-dominated
    // blocks). For fib's 3-deep inline trace, this cuts 4 helper calls
    // to 1 per dispatch (~60 cycles saved × 163k dispatches ≈ 3-5 ms,
    // ~10-15% win).
    //
    // SAFETY of memoization:
    // - Cache invalidation: none required within a single trace
    //   dispatch — `JIT_CL` is pinned at entry and unchanged through
    //   the entire trace body. The first GetUpval(idx) call materializes
    //   the value; subsequent reads of the same idx are exact duplicates.
    // - Cross-block validity: cached values are stored in a Variable
    //   (via def_var / use_var); cranelift's FunctionBuilder inserts
    //   phis as needed for cross-block reads.
    // - Side-exit safety: the first occurrence may be in a block reached
    //   only on the recursive path (e.g. block2 in fib). If a side-exit
    //   fires BEFORE that block (e.g. head-fail base case in block3),
    //   the cache is never populated and reuse never happens — correct.
    let upval_cache: std::collections::HashMap<u32, Variable> = std::collections::HashMap::new();
    // the entry closure, fetched at the first inlined call; the trace
    // is linear, so that fetch dominates every later call
    let head_closure_var: Option<Variable> = None;
    // No iconst memoization: the arm64 backend folds
    // `iconst+isub`/`iconst+icmp` into immediate-form instructions
    // at codegen, so it would add little.
    // incremented at each cmp side-exit emit point that
    // materialises ≥1 live Sinkable site. Telemetry only; the
    // dispatcher's runtime materialise calls are not counted here
    // (this is a per-trace static count of emit sites that emit
    // the helper call).
    let materialize_emit_count: u32 = 0;
    let closure_seen: u32 = 0;
    // Integer constants the registers hold at this point of the trace
    // (from LoadI / LoadK earlier in the same pass), so a `//`, `%` or shift
    // by a constant needs no runtime guard.
    let known_int: Vec<Option<i64>> = vec![None; window_size_us];
    Lower {
        bcx,
        h,
        reg_state,
        trace_fn_sig_ref,
        global_side_trace_box,
        regs_full,
        tforcall_tag_var,
        tforcall_val_tag_var,
        precheck,
        body_loop,
        head_kinds,
        defined_aot_data,
        escape,
        flush_ctx,
        virt_vars,
        virt_kinds,
        sunk_alloc_seen,
        materialize_emit_count,
        closure_seen,
        stored,
        current_kinds,
        dispatchable,
        dispatch_off_reason,
        per_exit_kinds,
        per_exit_inline_vec,
        call_chain,
        upval_cache,
        upval_checked: std::collections::HashMap::new(),
        upval_check_done: Vec::new(),
        head_closure_var,
        known_int,
        alt_joins: std::collections::HashMap::new(),
        tier_count: None,
    }
}
