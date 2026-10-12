//! Per-opcode helpers of the lowerer: the type a table read's result is
//! used as.

use super::*;

/// Single-op classifier — the per-op decision logic used by the
/// look-ahead walker [`infer_getx_exit_lookahead`].
///
/// The helpers returning a GetX value (`Op::GetI` / `Op::GetTable` /
/// `Op::GetField` / `Op::GetTabUp`) hand back the table cell's raw
/// 8-byte payload — Int, Table, Float, or anything else. A static
/// prediction of the payload's tag needs context from the use site.
///
/// - Arithmetic / numeric cmp operand → must be Int.
/// - Table-base operand (Get / Set / Len) → must be Table.
/// - Anything else (`Move`, `Eq`, trace tail, literal
///   materialisation) → unknown; return `None`. The walker treats
///   `LoadI / LoadF / LoadK` as transparent and walks past them.
///
/// `Op::Eq` is *not* a tag indicator: values of any two types can
/// be compared, so the cmp itself doesn't pin the result's tag.
pub(super) fn infer_getx_exit_inst(getx_a: u32, next: Inst) -> Option<ExitTag> {
    let na = next.a();
    let nb = next.b();
    let nc = next.c();
    match next.op() {
        // Arith: A := B op C. The result's reg is Int; if a
        // GetX output is consumed here it's an Int operand.
        Op::Add | Op::Sub | Op::Mul => {
            if nb == getx_a || nc == getx_a {
                Some(ExitTag::Int)
            } else {
                None
            }
        }
        // Lt / Le compare ordering — operand must be numeric.
        Op::Lt | Op::Le => {
            if na == getx_a || nb == getx_a {
                Some(ExitTag::Int)
            } else {
                None
            }
        }
        // Table-base reads: A := B[*]. The B operand must be a
        // table; if GetX's output feeds it, we know it's a Table.
        Op::GetI | Op::GetTable | Op::GetField => {
            if nb == getx_a {
                Some(ExitTag::Table)
            } else {
                None
            }
        }
        // Table-base writes: A[*] := *. The A operand must be a
        // table.
        Op::SetI | Op::SetTable | Op::SetList | Op::SetField => {
            if na == getx_a {
                Some(ExitTag::Table)
            } else {
                None
            }
        }
        // `#R[B]` requires R[B] to be a table.
        Op::Len => {
            if nb == getx_a {
                Some(ExitTag::Table)
            } else {
                None
            }
        }
        // `Op::Eq` compares values of any two types, so it tells
        // us nothing about the operand's tag.
        _ => None,
    }
}

/// The type the table read at `ops[i]` is typed as: the tag it was seen to
/// produce while recording, when the trace can hold a value of it, else
/// what the ops after it (up to `end`) use it as. The read is checked
/// against it either way.
pub(super) fn infer_getx_exit(record: &TraceRecord, i: usize, end: usize) -> Option<ExitTag> {
    use luna_core::runtime::value::raw;
    let seen = record.result_tag(i).and_then(|t| match t {
        raw::INT => Some(ExitTag::Int),
        raw::FLOAT => Some(ExitTag::Float),
        raw::TABLE => Some(ExitTag::Table),
        raw::STR => Some(ExitTag::Str),
        raw::CLOSURE => Some(ExitTag::Closure),
        raw::FALSE | raw::TRUE => Some(ExitTag::Bool),
        _ => None,
    });
    seen.or_else(|| {
        (i + 1 < end)
            .then(|| infer_getx_exit_lookahead(record.ops[i].inst.a(), &record.ops[i + 1..end]))
            .flatten()
    })
}

/// Look-ahead part of [`infer_getx_exit`]: walks the recorded ops
/// after the GetX, skipping ops that are *transparent* (provably
/// don't read `R[getx_a]` and don't overwrite it). Returns as soon
/// as the first non-transparent op classifies the use, or `None` if
/// nothing in the trail consumes the slot or the slot is overwritten
/// first.
///
/// The transparent set is intentionally tight: only `LoadI`,
/// `LoadF`, `LoadK` (literal materialisation into a register slot ≠
/// getx_a) qualify. These ops never read any register, so they can
/// never consume `R[getx_a]`; if their `A` ≠ `getx_a` they also
/// don't overwrite it. Anything else stops the walk — either we
/// classify (Add / Sub / Mul / Lt / Le / Get* / Set* / Len) or
/// we conservatively return `None`.
///
/// This unblocks the common Lua codegen pattern
///
/// ```text
///   GetField R[a], R[base], "k"   ; read
///   LoadI    R[a+1], <literal>    ; materialise const operand
///   Le       R[?],  R[a], R[a+1]  ; compare R[a] vs literal
/// ```
///
/// where the 1-op-ahead path saw `LoadI` next and bailed.
pub(super) fn infer_getx_exit_lookahead(getx_a: u32, ops_after: &[RecordedOp]) -> Option<ExitTag> {
    // Bound the walk — analysis cost cap on faulty trace shapes.
    // Most use sites are within 1-2 ops; 4 is generous.
    const MAX_LOOKAHEAD: usize = 4;
    for rop in ops_after.iter().take(MAX_LOOKAHEAD) {
        let inst = rop.inst;
        let op = inst.op();
        let a = inst.a();
        // First, a per-op classification attempt — if the use site
        // is right here, that wins.
        if let Some(tag) = infer_getx_exit_inst(getx_a, inst) {
            return Some(tag);
        }
        // Transparent: `LoadI / LoadF / LoadK` materialise a
        // literal into `R[A]`. They read no registers. If `A` is
        // not our slot they don't affect it; walk past. This
        // covers the canonical Lua codegen pattern
        // `GetField R[a]; LoadI R[a+1], K; Le R[?], R[a], R[a+1]`
        // where the 1-op-ahead path would have stopped at LoadI.
        if matches!(op, Op::LoadI | Op::LoadF | Op::LoadK) {
            if a == getx_a {
                // LoadX overwrites our slot before any use — give up.
                return None;
            }
            continue;
        }
        // Any other op stops the walk: either the per-op classifier
        // already pinned a tag (handled above) or we conservatively
        // bail to avoid a wrong static prediction.
        return None;
    }
    None
}

/// The kind a GetX result is typed as, from its inferred use, and the
/// value tag its checked read demands (`None`: the use is not typed).
pub(super) fn getx_want(tag: Option<ExitTag>) -> Option<(RegKind, u8)> {
    use luna_core::runtime::value::raw;
    match tag {
        Some(ExitTag::Int) => Some((RegKind::Int, raw::INT)),
        Some(ExitTag::Table) => Some((RegKind::Table, raw::TABLE)),
        Some(ExitTag::Float) => Some((RegKind::Float, raw::FLOAT)),
        Some(ExitTag::Str) => Some((RegKind::Str, raw::STR)),
        Some(ExitTag::Closure) => Some((RegKind::Closure, raw::CLOSURE)),
        // either boolean: the checked helpers hand back 0 or 1
        Some(ExitTag::Bool) => Some((RegKind::Bool, raw::FALSE)),
        _ => None,
    }
}

/// forward-look exit-tag inference for `Op::GetUpval`.
/// Walks the recorded ops following the GetUpval until either:
/// - an `Op::Call` with `A == getupval_a` is found → the upval is
///   that Call's function target → `Some(ExitTag::Closure)`.
/// - an op that writes `R[getupval_a]` is found before the Call →
///   the upval is overwritten in this slot → `None`.
/// - the walk runs out → `None`.
pub(super) fn infer_upval_exit(getupval_a: u32, ops_after: &[RecordedOp]) -> Option<ExitTag> {
    for rop in ops_after {
        let next = rop.inst;
        if next.op() == Op::Call && next.a() == getupval_a {
            return Some(ExitTag::Closure);
        }
        // Conservative writes-A detection: ops that don't write A
        // are control / cmp / store ops. Everything else writes A.
        let writes_a = !matches!(
            next.op(),
            Op::Lt
                | Op::Le
                | Op::Eq
                | Op::EqK
                | Op::Jmp
                | Op::JmpClose
                | Op::JmpCloseBack
                | Op::SetI
                | Op::SetTable
                | Op::SetList
                | Op::Return0
                | Op::Return1
                | Op::Return
        );
        if writes_a && next.a() == getupval_a {
            return None;
        }
    }
    None
}
