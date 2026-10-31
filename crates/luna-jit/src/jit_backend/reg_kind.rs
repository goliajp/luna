//! The kinds the method JIT gives a register.

/// per-Lua-register type lattice. `Unset` is the bottom;
/// `Int` and `Float` are incomparable monotypes. A register that's
/// pinned to both Int and Float in the same Proto causes the lowerer
/// to bail (`unify_kind` returns false). `Unset` registers that
/// stay Unset after the scan default to Int at emit time (they're
/// only used by Cranelift's SSA in unreachable tail slots).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum RegKind {
    Unset,
    Int,
    Float,
    /// `Gc<Table>` raw pointer pun. Represented as I64 at the
    /// Cranelift level (same shape as `RegKind::Int`) but kept distinct
    /// in the lattice so a register pinned to a table can't unify with
    /// one pinned to an integer; the whitelist gates on Table where it
    /// expects a table operand (e.g. `SetTable.A`, `Len.B`).
    Table,
}

impl RegKind {
    #[inline]
    pub(super) fn unify(slot: &mut RegKind, incoming: RegKind) -> bool {
        match (*slot, incoming) {
            (_, RegKind::Unset) => true,
            (RegKind::Unset, _) => {
                *slot = incoming;
                true
            }
            (a, b) if a == b => true,
            // Int+Table coexist (both I64-shaped at the
            // Cranelift level; `maybe_table[reg]` + Table-bail on
            // arith/cmp/ForPrep keeps the semantic guard).
            (RegKind::Int, RegKind::Table) | (RegKind::Table, RegKind::Int) => true,
            // Float+Table coexist via I64↔F64
            // bitcast. The Variable is declared in whichever shape
            // the first writer pinned (F64 if Float first, I64 if
            // Table first); `aligned_def` handles the writer-side
            // bitcast and the emit-side `use_var` callers for
            // Table operands bitcast F64→I64 on the read side when
            // the declared slot is F64. Bit-pattern reinterpret is
            // lossless for an 8-byte pun (`f64::from_bits(ptr as
            // u64).to_bits()` round-trips exactly). Unlocks the
            // 5.1/5.2 `binary_trees` pattern: `if d == 0` uses
            // LoadF R[1]=0 (Float) in one BB, NewTable R[1]
            // (Table) in another — both safe per-BB, but a stricter
            // `unify` would reject the slot reuse globally.
            (RegKind::Float, RegKind::Table) | (RegKind::Table, RegKind::Float) => true,
            _ => false,
        }
    }
}
