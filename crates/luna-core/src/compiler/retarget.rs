//! Opcodes whose destination register may be rewritten in place.

use super::*;

/// Closed set of opcodes whose A field is a pure destination produced by an
/// `Exp::Reloc(pc)` discharge AND whose runtime body does NOT trigger a GC
/// step keyed off `base + A + 1` as the live-stack-top. The Reloc-
/// landing peephole at `assign_name` retargets one of these in place to a
/// local register, skipping the otherwise-required Move.
///
/// Excluded opcodes and why:
///   - `Concat`: `R[A] := R[A] .. ... .. R[A+B-1]` — A is the operand range
///     base, retargeting would reread from a different start.
///   - `SelfOp`: writes both `R[A+1]` (object copy) and `R[A]` (method
///     handle); A is part of a pair, not a free destination.
///   - `Move` / `LoadK` / `LoadI` / `LoadF` / `LoadNil` / `LoadTrue` /
///     `LoadFalse` / `LFalseSkip`: discharged to their final register
///     directly by `exp_to_reg`'s typed arms, so a Move-from-temp shape
///     never appears for them.
///   - `NewTable` / `Closure`: BOTH allocate and then call
///     `maybe_collect_garbage(base + A + 1)` immediately after writing to
///     `R[A]` (see `vm/exec.rs` Op::NewTable / Op::Closure arms). The
///     `base + A + 1` is interpreted as the live-stack-top for GC roots:
///     retargeting A to a register BELOW some other still-live local
///     would let GC sweep the higher local during that step. Excluded
///     unconditionally — PUC sidesteps this because `L->top` is bumped
///     past every live local before luaC_step, but luna's per-op live_top
///     computation does not have that headroom.
///   - `GetUpval`: A is destination, B is upvalue idx, no GC side effect
///     — included.
///   - `Not`: `R[A] := not R[B]` — A is destination, no GC — included.
///   - Arithmetic / bitwise / Len / Get* may dispatch to Lua metamethods,
///     but the call-out is routed through `begin_meta_call` which captures
///     `self.top` as the stack high-water, not `base + A + 1`. GC steps
///     during the metamethod run use that captured `top`, so retargeting
///     A is safe.
///   - Comparison / call / store ops are never produced via Exp::Reloc
///     in luna's compiler (`Cmp`-shape goes through `Exp::Cmp`, calls via
///     `Exp::Open`, stores never become a "value" expression).
pub(super) fn is_retargetable_op(op: Op) -> bool {
    use Op::*;
    matches!(
        op,
        Add | Sub
            | Mul
            | Mod
            | Pow
            | Div
            | IDiv
            | BAnd
            | BOr
            | BXor
            | Shl
            | Shr
            | AddI
            | SubI
            | AddK
            | SubK
            | MulK
            | ModK
            | PowK
            | DivK
            | IDivK
            | BAndK
            | BOrK
            | BXorK
            | ShrI
            | ShlI
            | Unm
            | BNot
            | Not
            | Len
            | GetUpval
            | GetTabUp
            | GetTable
            | GetI
            | GetField
    )
}
