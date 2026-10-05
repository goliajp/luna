use super::*;

/// per-op register window offset.
///
/// For a recorded trace with inline self-recursive `Op::Call`s, each
/// `RecordedOp` at depth `d` views its `R[k]` as the trace's
/// `reg_state_buf[offset_per_op[i] + k]`. The offset accumulates
/// across self-recursive calls:
/// - depth 0 ops: offset = 0
/// - When `Op::Call A B C` (self-recursive) at depth d is followed
///   by an op at depth d+1: that callee's offset = `offset[d] + A + 1`.
///   (Lua call ABI: callee's `R[0]` lives at caller's `R[A+1]`, since
///   `R[A]` is the function value being called.)
/// - On `Op::Return*` at depth d>0: depth drops to d-1, offset
///   reverts to the saved `offset[d-1]`.
///
/// Used by the body emit to address registers across inlined
/// frames. The companion `enclosing_call_a` Vec gives the matching
/// caller `Op::Call`'s A field for any depth>0 op (None at depth 0),
/// which the `Op::Return*` emit consumes to compute the
/// caller's destination slot for return-value copy-back.
/// pure-function depth invariant verifier.
///
/// Returns `true` iff the recorded op sequence's inline-depth
/// trail is well-formed for `compute_op_offsets` to consume:
///
/// 1. The first op (if any) is at depth 0.
/// 2. Any depth bump (`d_curr > d_prev`) is exactly `d_prev + 1`
///    AND the previous op is an `Op::Call` (the only legitimate
///    frame-push trigger in Lua bytecode under the recorder's
///    self-rec inline contract).
/// 3. Depth never exceeds `MAX_INLINE_DEPTH` (the lowerer's
///    window-size cap).
///
/// Depth descents (`d_curr < d_prev`) are unconstrained — the
/// interpreter can unwind many frames via consecutive
/// `Op::Return*` / tail-call exits, and the offset stack `pop()`
/// loop in `compute_op_offsets` handles arbitrary descent.
///
/// The function operates on `(depth, is_call)` tuples instead of
/// the heavier `RecordedOp` so the lib unit tests can construct
/// synthetic input without a real `Gc<Proto>`.
pub(crate) fn verify_depth_invariant(items: &[(u8, bool)]) -> bool {
    if items.is_empty() {
        return true;
    }
    if items[0].0 != 0 {
        return false;
    }
    let mut prev_depth = items[0].0;
    let mut prev_was_call = items[0].1;
    for &(d, is_call) in &items[1..] {
        if d > prev_depth {
            if d != prev_depth + 1 {
                return false;
            }
            if !prev_was_call {
                return false;
            }
        }
        if d > MAX_INLINE_DEPTH {
            return false;
        }
        prev_depth = d;
        prev_was_call = is_call;
    }
    true
}

/// Where the plain terminator scan ends a trace: the first depth-0
/// `Call` that is not self-recursive, `ForLoop` / `TForLoop`, depth-0
/// return, or an op the inline path cannot hold (`InlineAbort`).
/// Self-recursive calls are walked past; the callee's ops follow at
/// depth + 1. `None` when the whole record is body.
pub(super) fn plain_trace_end(
    record: &TraceRecord,
    folded_ops: &[bool],
) -> Option<(usize, TraceEnd)> {
    let mut found: Option<(usize, TraceEnd)> = None;
    let (calls, _) = inline_calls(record);
    for (i, r) in record.ops.iter().enumerate() {
        if folded_ops[i] {
            continue;
        }
        let depth = r.inline_depth as usize;
        if depth > MAX_INLINE_DEPTH as usize {
            found = Some((i, TraceEnd::InlineAbort));
            break;
        }
        match r.inst.op() {
            Op::Call => {
                if call_inlinable(record, &calls, i) {
                    // Continue walking — Op::Call emits nothing in
                    // the inline path and op_offsets handles the
                    // window shift for the callee's subsequent ops.
                    continue;
                }
                if depth == 0 {
                    found = Some((i, TraceEnd::Call));
                } else {
                    found = Some((i, TraceEnd::InlineAbort));
                }
                break;
            }
            Op::ForLoop => {
                if depth == 0 {
                    found = Some((i, TraceEnd::ForLoop));
                } else {
                    found = Some((i, TraceEnd::InlineAbort));
                }
                break;
            }
            // generic-for back-edge. Same tail
            // emit slot as Op::ForLoop (TraceEnd::ForLoop); the
            // tail emit branches on `record.ops[idx].inst.op()`
            // to pick the right side-exit predicate (count>0 vs
            // R[A+4] tag check).
            Op::TForLoop => {
                if depth == 0 {
                    found = Some((i, TraceEnd::ForLoop));
                } else {
                    found = Some((i, TraceEnd::InlineAbort));
                }
                break;
            }
            Op::Return0 | Op::Return1 if depth == 0 => {
                found = Some((i, TraceEnd::Return));
                break;
            }
            // depth>0 Returns are inline-path unwinds; the
            // body emit loop handles them (Return0 no-op,
            // Return1 copy-back). Don't terminate.
            _ => {}
        }
    }
    found
}

/// Whether the `Op::Call` at `i` is lowered inline: the recorder followed
/// it into a Lua function (the next op is one level deeper) and
/// [`inline_calls`] could lay out the callee's frame: the argument count
/// is fixed or the recording fixes the stack top it comes from, the callee
/// returns a count the recording fixes, and it does not need the
/// arguments as a table. A vararg callee's extra arguments move below its
/// registers as `push_frame` moves them, in the trace's registers and in
/// the frames the frame-materialise helper rebuilds at an exit.
///
/// Any other call ends the trace there, as a call the trace leaves to the
/// interpreter.
pub(super) fn call_inlinable(record: &TraceRecord, calls: &[Option<InlineCall>], i: usize) -> bool {
    let depth = record.ops[i].inline_depth as usize;
    calls.get(i).copied().flatten().is_some()
        && record
            .ops
            .get(i + 1)
            .is_some_and(|next| next.inline_depth as usize == depth + 1)
        && depth < MAX_INLINE_DEPTH as usize
}

pub(super) fn compute_op_offsets(record: &TraceRecord) -> (Vec<u32>, Vec<Option<u8>>) {
    let n = record.ops.len();
    let (calls, _) = inline_calls(record);
    let mut offsets = Vec::with_capacity(n);
    let mut enclosing_call_a = Vec::with_capacity(n);
    // `offset_stack[d]` = the register-window offset for depth d.
    // `call_a_stack[d]` = the Op::Call A field that entered depth d.
    //   `call_a_stack[0]` is unused (depth 0 has no enclosing call).
    let mut offset_stack: Vec<u32> = vec![0u32];
    let mut call_a_stack: Vec<u8> = vec![0u8];
    for (i, rop) in record.ops.iter().enumerate() {
        let d = rop.inline_depth as usize;
        if d >= offset_stack.len() {
            // Depth increased — the previous op must be Op::Call
            // (recorder invariant for self-recursive entry).
            debug_assert!(i > 0, "first recorded op cannot be at depth > 0");
            debug_assert!(
                d == offset_stack.len(),
                "depth jumped more than 1 in a single transition"
            );
            let caller_idx = i - 1;
            let caller = &record.ops[caller_idx];
            debug_assert!(
                matches!(caller.inst.op(), Op::Call),
                "depth bump must follow Op::Call"
            );
            let caller_offset = offset_stack[offset_stack.len() - 1];
            let caller_a = caller.inst.a();
            // a vararg callee's extra arguments sit below its registers
            let extras = calls[caller_idx].map_or(0, |c| c.n_varargs);
            let new_offset = caller_offset + caller_a + 1 + extras;
            offset_stack.push(new_offset);
            // Lua register indices fit in u8 by VM design; this
            // cast is lossless for any valid bytecode.
            call_a_stack.push(caller_a as u8);
        } else {
            // Depth decreased (or stayed equal). Pop down to d.
            while offset_stack.len() > d + 1 {
                offset_stack.pop();
                call_a_stack.pop();
            }
        }
        offsets.push(offset_stack[d]);
        enclosing_call_a.push(if d == 0 { None } else { Some(call_a_stack[d]) });
    }
    (offsets, enclosing_call_a)
}

/// Which terminating op (if any) sits at the trace's effective
/// tail position. See the comment block in
/// [`try_compile_trace_with_options`] for the contracts on each.
#[derive(Clone, Copy, Debug)]
pub(super) enum TraceEnd {
    Call,
    ForLoop,
    /// the trace's inline-recursion path hit something
    /// the lowerer can't continue past (ForLoop@d>0, a non-self
    /// Call@d>0, depth past MAX_INLINE_DEPTH, or a proto mismatch).
    /// emit `ops[..i]` normally, then close the tail with a
    /// store-back + return of `record.ops[i].pc`. Dispatchable is
    /// forced false because the interp can't resume at that PC
    /// without first materialising the depth>0 CallFrames. cmp@d>0
    /// does not land here: it emits a real side-exit via the
    /// frame-mat helper.
    InlineAbort,
    /// `Op::Return0` / `Op::Return1` at depth=0
    /// terminates the trace (the caller frame unwinds). Treat as a
    /// truncation point: emit `ops[..i]` normally, then store back
    /// the caller window + return `record.ops[i].pc`. The interp
    /// re-executes the Return instruction with the correct PC. Same
    /// shape as `TraceEnd::Call` but emitted by a different op, so
    /// kept as a separate variant for the tail dispatch.
    Return,
    /// Recorder detected self-recursion via the cycle catch
    /// (same-proto ancestor count > [`RECUNROLL_THRESHOLD`] at the
    /// head_pc on head_proto). The trace body covers the inlined
    /// recursion levels, but the lowerer's tail is a plain deopt
    /// (store back the caller window, return `head_pc`) and the trace
    /// is not dispatchable: restoring a snapshot across the back-edge
    /// gives wrong results for non-tail self-recursion such as fib.
    SelfLink(SelfRecKind),
    /// recorder detected a down-recursion close
    /// shape: a depth>0 `Op::Return` fired during recording AND the
    /// `rec.retfs` chain showed the trace bouncing in-and-out of the
    /// same caller-proto past [`RECUNROLL_THRESHOLD`]. Mirrors
    /// LuaJIT's `LJ_TRLINK_DOWNREC` close (`lj_record.c:912
    /// lj_trace_err(LJ_TRERR_DOWNREC)` → `lj_trace.c:570
    /// trace_downrec` → restart-at-Return-PC). Routed BEFORE the
    /// `SelfLink` arm in the `end_idx` picker so depth>0 Return
    /// closes win over depth>0 self-link cycles.
    ///
    /// `return_pc` is the PC the inlined frame is unwinding to —
    /// the stitch-entry head_pc the lowerer bakes into the
    /// retf-guard sequence (`asm_retf` equivalent).
    /// `target_proto_id` carries the target proto's `Gc::as_ptr()`
    /// as a raw `usize` so this enum stays `Copy` for the existing
    /// `end_idx_opt: Option<(usize, TraceEnd)>` plumbing. The
    /// matching `Gc<Proto>` lives on `TraceRecord.downrec_close.
    /// target_proto` (not erased); the lowerer cross-references the
    /// two when emitting the guard.
    ///
    /// The lowerer emits the retf-guard + stitch sentinel for this
    /// arm, falling back to the safe deopt-tail on a guard miss.
    DownRec {
        /// PC the Return is unwinding to (caller's resume PC).
        return_pc: u32,
        /// Caller proto's `Gc::as_ptr()` as `usize` (kept opaque so
        /// `TraceEnd` stays `Copy`). The lowerer reads
        /// `TraceRecord.downrec_close.target_proto` for the real
        /// `Gc<Proto>`.
        target_proto_id: usize,
        /// `from_depth - to_depth` at the moment the catch tripped.
        /// Always `1` today.
        depth_delta: u8,
    },
}

/// Direction the cmp/Jmp pair took in the recording. Both
/// directions can compile, but the brif's predicate and the
/// side-exit PC flip between them.
#[derive(Clone, Copy, Debug)]
pub(super) enum CmpDir {
    /// Recorded: cmp matched K → no pc++ → Jmp executed. Next
    /// recorded op is the Jmp at `cmp_pc + 1`; consumed_by_cmp
    /// marks it. Side-exit PC = `cmp_pc + 2` (interp pc++).
    /// Standard repeat-until / `for` exit-cmp shape.
    TookJmp,
    /// Recorded: cmp didn't match K → pc++ → Jmp skipped. Next
    /// recorded op is the body op at `cmp_pc + 2`. Side-exit PC
    /// = the Jmp's target. Standard `while cond do` body-entry
    /// shape.
    SkippedJmp,
}

/// First filter on a recorded op: an op outside this set makes the
/// lowerer return `None` and the recorder drops the trace. Admission is
/// not compilation: the pre-emit pass of
/// [`try_compile_trace_with_options`] still bails on operand kinds,
/// register bounds and shapes it cannot lower (for example `GetTabUp` /
/// `GetField` outside a math fold).
///
/// - `Move` copies the 8-byte payload whatever its type.
/// - Arithmetic, bitwise and compare ops lower for the operand kinds
///   recorded in the trace.
/// - `Jmp` emits no IR: it is either consumed by the compare before it
///   or the trailing back edge.
/// - Table reads and writes go through the `luna_jit_table_*` helpers,
///   or through the virtual slots of a site `escape_analyze` sank. A
///   helper that meets a metatable reports it and the trace side-exits,
///   so the interpreter runs the metamethod.
/// - `Call`, `ForLoop`, `TForLoop` and returns end the trace (see
///   [`TraceEnd`]), except self-recursive calls, which are inlined.
pub(super) fn is_whitelisted_op(op: Op) -> bool {
    matches!(
        op,
        Op::Move
            | Op::Add
            | Op::Sub
            | Op::Mul
            | Op::Div
            | Op::Pow
            | Op::IDiv
            | Op::Mod
            | Op::BAnd
            | Op::BOr
            | Op::BXor
            | Op::Shl
            | Op::Shr
            | Op::Unm
            | Op::BNot
            | Op::Jmp
            | Op::Lt
            | Op::Le
            | Op::Eq
            | Op::EqK
            | Op::NewTable
            | Op::GetI
            | Op::GetTable
            | Op::SetI
            | Op::SetTable
            | Op::SetList
            | Op::Len
            | Op::Call
            | Op::ForLoop
            | Op::LoadI
            | Op::LoadF
            | Op::LoadK
            // Op::LoadNil writes Nil to R[A..=A+B].
            // Emit: iconst(0) + def_var per slot + current_kinds[slot]
            // = RegKind::Nil. ExitTag::Nil carries the Nil
            // through restore so non-Nil entry slots get repacked
            // as Value::Nil rather than mis-typed.
            | Op::LoadNil
            // Op::Closure creates `R[A] := closure(proto[Bx])`.
            // Emit: call `luna_jit_op_closure(bx)` (shared-upval path
            // only; in_stack upvals bail compile in pre-emit). Result
            // is the Gc<LuaClosure> raw payload; current_kinds =
            // RegKind::Closure → ExitTag::Closure on side-exit restore.
            | Op::Closure
            // Op::Close closes open upvals at slot ≥ A.
            // Emit: pre-Close spill of all live regs ≥ A, then
            // call `luna_jit_op_close(a)` returning 0 (continue) or
            // 1 (deopt). Deopt block writes store_back + returns
            // close_pc so interp redoes the Op::Close. Helper's
            // close_from is idempotent on the deopt path (open
            // upvals already popped).
            | Op::Close
            // Op::GetUpval reads the trace head
            // closure's upvals[idx] via the `luna_jit_upval_get`
            // helper (the dispatcher's enter_jit pins JIT_CL).
            | Op::GetUpval
            // GetTabUp / GetField are admitted ONLY inside a math
            // fold; the pre-emit pass enforces that gate via
            // `folded_math[i]`.
            | Op::GetTabUp
            | Op::GetField
            // Op::SetField writes `R[A][K[B]:string] = R[C]`.
            // Helper-path emit calls luna_jit_table_set_field with the
            // string key's Gc<LuaStr> raw ptr baked into IR.
            | Op::SetField
            // Op::Test gates `if x then ...` branches
            // when x isn't a comparison. Followed by Op::Jmp (taken
            // or skipped depending on R[A] truthiness vs K).
            // Kind-known truthy/falsy via compile-time
            // const fold (no IR — recorded direction is provably
            // stable); RegKind::Unset bails compile.
            | Op::Test
            // Op::TestSet is `if R[B].truthy()==K
            // then R[A]=R[B] else pc++`. Same kind-fold approach
            // as Op::Test (truthy of R[B]); on test-pass branch
            // (TookJmp recorded), emit a Move-style def_var
            // R[A] = R[B].
            | Op::TestSet
            // generic-for ops. TForPrep is a forward
            // pc-bump emitted before the body (head_pc = body_top,
            // so recorder never actually sees TForPrep in record —
            // whitelist only as a defensive arm). TForCall calls
            // the iterator via `luna_jit_op_tforcall` helper.
            // TForLoop terminates the trace at its back-edge,
            // handled in the tail emit (same TraceEnd::ForLoop
            // arm as Op::ForLoop, dispatch branches on inst.op()).
            | Op::TForPrep
            | Op::TForCall
            | Op::TForLoop
            // Op::Concat A B does an N-operand
            // right-associative fold over `R[A..A+B-1]`, writing
            // the resulting string to R[A]. Trace emit spills the
            // operand window to vm.stack and calls
            // `luna_jit_op_concat(A, B, roots)` helper which runs
            // concat_run + detects/deopts on the __concat
            // metamethod path. Helper-path equivalent to interp
            // (perf wash); the perf wins live in the buffered string
            // accumulator path.
            | Op::Concat
            // Op::SelfOp `R[A+1] := R[B]; R[A] := R[B][K[C]]`: a method
            // lookup through the receiver's table-valued `__index` links
            // (`luna_jit_op_self_checked`)
            | Op::SelfOp
            // booleans: `LFalseSkip` writes false and skips the next op,
            // which the recording already did not follow
            | Op::LoadFalse
            | Op::LoadTrue
            | Op::LFalseSkip
            | Op::Not
    )
}
