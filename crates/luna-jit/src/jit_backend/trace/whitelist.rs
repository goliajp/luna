//! The ops a trace may contain.

use super::*;

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
            | Op::ForLoop55
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
            // 5.2 / 5.3 closing jumps: the close as `Op::Close`, the
            // jump as `Op::Jmp`
            | Op::JmpClose
            | Op::JmpCloseBack
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
            | Op::TForPrep53
            | Op::TForCall53
            | Op::TForLoop53
            | Op::TForPrep55
            | Op::TForCall55
            | Op::TForLoop55
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
            // booleans: `LFalseSkip` / `LTrueSkip` write a boolean and skip
            // the next op, which the recording already did not follow
            | Op::LoadFalse
            | Op::LoadTrue
            | Op::LFalseSkip
            | Op::LTrueSkip
            | Op::Not
    )
}
