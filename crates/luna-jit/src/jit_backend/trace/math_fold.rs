//! Recognizing `math.<fn>(...)` call windows in a recorded trace that
//! the lowerer folds into a libm call or an inline min / max.

use super::*;

/// Recognised single-arg libm math functions. Each entry maps the
/// Lua-side const-pool name (bytes) to the libm symbol cranelift
/// imports. Signature is uniform across the table: `(f64) -> f64`.
/// `math.log(x, base)` / `math.atan(y, x)` / `math.max(...)` use a
/// different bytecode window (B≠2) so the pattern matcher rejects
/// them.
///
/// `pre53` cannot tell 5.3 from 5.2, and two of these differ between
/// them: 5.3+ `atan(y)` is `atan2(y, 1)`, which libm does not round like
/// `atan(y)`, and 5.3+ `floor` / `ceil` return integers. Those two fold
/// only on 5.4+ (see the emit).
pub(super) const MATH_LIBM_FNS: &[(&[u8], &str)] = &[
    (b"sin", "sin"),
    (b"cos", "cos"),
    (b"tan", "tan"),
    (b"asin", "asin"),
    (b"acos", "acos"),
    (b"atan", "atan"),
    (b"exp", "exp"),
    (b"log", "log"),
    (b"sqrt", "sqrt"),
    (b"floor", "floor"),
    (b"ceil", "ceil"),
];

/// Kind of a recognised `math.<fn>(args...)` fold. `Libm1` is the
/// single-arg fold over libm (sin/cos/sqrt/floor/...) — emits a
/// libm call. `Min2` / `Max2` are 2-arg `math.min` / `math.max`
/// folds — emit Cranelift's native `fmin` / `fmax` (single mcode
/// insn on ARM64 and x86_64).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum FoldKind {
    Libm1,
    Min2,
    Max2,
    /// `string.sub(s, i [, j])`, a split window like `Min2`, run as a
    /// direct helper call
    StrSub,
}

impl FoldKind {
    /// Whether the fold leaves the arg-prep ops alone and emits at its
    /// `Call` (the GetTabUp / GetField emit nothing).
    pub(super) fn split(self) -> bool {
        !matches!(self, FoldKind::Libm1)
    }
}

/// Source of a Libm1 fold's argument register slot. Only `Move`
/// is accepted (existing v1.2 contract). The 2-arg `Min2 / Max2`
/// folds skip the [`FoldArgSrc`] decoding entirely — they emit at
/// the Call's position and read `R[A+1] / R[A+2]` directly, so
/// arg-prep ops can be arbitrarily-shaped (LoadI / Move / GetField
/// / Add / etc.) and execute normally before the Call's fold emit.
#[derive(Clone, Copy, Debug)]
pub(super) enum FoldArgSrc {
    /// `Move R[A+k] := R[reg]`. Libm1 emit reads the variable at `reg`.
    Reg { reg: u32 },
}

/// A single recognised `math.<fn>(arg...)` fold.
///
/// Two shape kinds:
///
/// * **`Libm1`** — 4-op contiguous window: `GetTabUp + GetField +
///   Move + Call(B=2,C=2)`. The 4 indices `start_idx..start_idx+4`
///   are flagged in `folded_ops`. Emit fires at `start_idx` (the
///   `GetTabUp`) and produces one libm call.
///
/// * **`Min2 / Max2`** — *split-window* fold: `GetTabUp` at
///   `start_idx`, `GetField "min" / "max"` at `start_idx + 1`, and
///   `Call(B=3,C=2)` at `call_idx` (typically `start_idx + 4` for
///   `Move + Move` arg-prep, but can be `start_idx + 5` or more
///   when arg2 is computed by an in-place `GetField + Add` chain
///   like `bucket.tokens + refill`). Only `start_idx`,
///   `start_idx + 1`, and `call_idx` are flagged in `folded_ops`
///   — the arg-prep ops between them execute normally so the
///   Call's `R[A+1] / R[A+2]` arrive at their natural Lua-frame
///   slots. Emit fires at `call_idx` (the `Call`) and produces
///   `fmin / fmax` reading from `R[A+1] / R[A+2]`. The GetTabUp
///   and GetField emit positions are silent (their semantic result
///   — the resolved `math.min` function pointer — is discarded;
///   the trace knows the call target statically).
#[derive(Clone, Copy, Debug)]
pub(super) struct TraceMathFold {
    pub(super) start_idx: usize,
    /// Libm name ("sin", "cos", ...) for Libm1; "min" / "max"
    /// (diagnostic only) for Min2 / Max2.
    pub(super) fn_name: &'static str,
    pub(super) kind: FoldKind,
    /// Libm1 only: source of the single arg. `None` for Min2/Max2
    /// (their args live at R[A+1] / R[A+2] by Call ABI).
    pub(super) arg_src: Option<FoldArgSrc>,
    /// Trace-index of the Op::Call this fold collapses. For
    /// `Libm1` it's `start_idx + 3`; for `Min2 / Max2` it's the
    /// recognised Call op's index. Used by `folded_ops` indexing
    /// and the emit-site `start_idx == i` lookup.
    pub(super) call_idx: usize,
    /// `R[A]` of the Call — the destination of the fold's result.
    /// For `Libm1` this is also `start_idx`'s GetTabUp A; for
    /// `Min2 / Max2` it's identically `start_idx`'s GetTabUp A
    /// (and the Call's A).
    pub(super) dst_reg: u32,
    /// `R[A+1]` of the Call — only meaningful for `Min2 / Max2`.
    pub(super) arg1_reg: u32,
    /// `R[A+2]` of the Call — only meaningful for `Min2 / Max2`.
    pub(super) arg2_reg: u32,
    /// Arguments the Call passes (`B - 1`).
    pub(super) nargs: u32,
}

/// Maximum number of arg-prep ops the `Min2 / Max2` fold scans
/// between the `GetField "min"/"max"` and the closing `Call`.
/// Covers the common patterns (Move+Move = 2; LoadI+Move = 2;
/// LoadI + GetField + Add = 3; Move + GetField + Add + Add = 4) and
/// caps cost on adversarial trace shapes.
const MINMAX_FOLD_ARG_PREP_MAX: usize = 6;

/// Detect a `math.<fn>(arg...)` fold starting at recorded op index `i`.
/// Two arms recognised:
///   * `Libm1` — 4-op window `GetTabUp + GetField + Move + Call(B=2,C=2)`
///     mapped onto a libm fn in [`MATH_LIBM_FNS`].
///   * `Min2 / Max2` — split-window fold: `GetTabUp` at `i`, `GetField
///     "min"|"max"` at `i + 1`, then up to [`MINMAX_FOLD_ARG_PREP_MAX`]
///     arg-prep ops, then the closing `Call(B=3,C=2)`. The arg-prep
///     ops execute normally; only the GetTabUp/GetField/Call indices
///     are flagged in `folded_ops`. This lets `math.min(K, expr)`
///     fold even when `expr` is computed by an in-place `GetField +
///     Add` chain (the canonical Redis-Lua / BullMQ idiom).
///
/// Mirrors method JIT's `try_match_math_fold` but reads from
/// `record.ops[..]` instead of the Proto's code slice.
pub(super) fn try_match_trace_math_fold(
    record: &TraceRecord,
    i: usize,
    head_proto: Gc<Proto>,
    pre53: bool,
) -> Option<TraceMathFold> {
    // Common prefix (GetTabUp + GetField) needs at least 2 ops.
    if i + 1 >= record.ops.len() {
        return None;
    }
    // The first 2 ops must come from `head_proto` at depth 0.
    // Cross-Proto / inlined ops break the fold (would mis-resolve
    // the const-pool slots).
    for k in 0..=1 {
        if !std::ptr::eq(record.ops[i + k].proto.as_ptr(), head_proto.as_ptr()) {
            return None;
        }
        if record.ops[i + k].inline_depth != 0 {
            return None;
        }
    }
    let i0 = record.ops[i].inst;
    let i1 = record.ops[i + 1].inst;

    if !matches!(i0.op(), Op::GetTabUp) {
        return None;
    }
    if !matches!(i1.op(), Op::GetField) {
        return None;
    }

    let a = i0.a();
    // GetTabUp reads upvals[B] indexed by consts[C]. Pin B=0
    // (env upvalue) — frontend invariant.
    if i0.b() != 0 {
        return None;
    }
    let k_math = head_proto.consts.get(i0.c() as usize).copied()?;
    let luna_core::runtime::Value::Str(s) = k_math else {
        return None;
    };
    let lib = s.as_bytes();
    if lib != b"math" && lib != b"string" {
        return None;
    }

    // GetField R[A] = R[A].<key>. Same dest as source.
    if i1.a() != a || i1.b() != a {
        return None;
    }
    let k_fn = head_proto.consts.get(i1.c() as usize).copied()?;
    let luna_core::runtime::Value::Str(fname) = k_fn else {
        return None;
    };
    let fname_bytes = fname.as_bytes();
    if lib == b"string" {
        return match fname_bytes {
            b"sub" => split_window(record, i, head_proto, a, &[3, 4]).map(|(call_idx, nargs)| {
                TraceMathFold {
                    start_idx: i,
                    fn_name: "sub",
                    kind: FoldKind::StrSub,
                    arg_src: None,
                    call_idx,
                    dst_reg: a,
                    arg1_reg: a + 1,
                    arg2_reg: a + 2,
                    nargs,
                }
            }),
            _ => None,
        };
    }

    // ── Libm1 arm (4-op window, B=2 C=2 call) ─────────────────
    if let Some(fn_name) = MATH_LIBM_FNS
        .iter()
        .find_map(|&(needle, name)| (needle == fname_bytes).then_some(name))
    {
        // floor/ceil results are floats on 5.1/5.2 and integers on 5.3,
        // atan is atan(y) on 5.1/5.2 and atan2(y, 1) on 5.3; `pre53`
        // covers both, so leave the call to the interpreter.
        if pre53 && (is_rounding(fn_name) || fn_name == "atan") {
            return None;
        }
        if i + 3 >= record.ops.len() {
            return None;
        }
        // Tail ops i+2, i+3 same head_proto + depth 0 constraint.
        for k in 2..=3 {
            if !std::ptr::eq(record.ops[i + k].proto.as_ptr(), head_proto.as_ptr()) {
                return None;
            }
            if record.ops[i + k].inline_depth != 0 {
                return None;
            }
        }
        let i2 = record.ops[i + 2].inst;
        let i3 = record.ops[i + 3].inst;
        if !matches!(i2.op(), Op::Move) {
            return None;
        }
        if !matches!(i3.op(), Op::Call) {
            return None;
        }
        // Move R[A+1] = R[arg_reg].
        if i2.a() != a + 1 {
            return None;
        }
        let arg_reg = i2.b();
        // Call R[A], B=2 (1 arg), C=2 (1 result).
        if i3.a() != a || i3.b() != 2 || i3.c() != 2 {
            return None;
        }
        return Some(TraceMathFold {
            start_idx: i,
            fn_name,
            kind: FoldKind::Libm1,
            arg_src: Some(FoldArgSrc::Reg { reg: arg_reg }),
            call_idx: i + 3,
            dst_reg: a,
            arg1_reg: 0,
            arg2_reg: 0,
            nargs: 1,
        });
    }

    // ── Min2 / Max2 arm (split-window, B=3 C=2 call) ───────────
    //
    // GetTabUp at `i`, GetField at `i+1`, then scan forward up to
    // MINMAX_FOLD_ARG_PREP_MAX ops looking for the matching
    // `Op::Call A B=3 C=2`. All scanned ops must come from
    // `head_proto` at depth 0. We do NOT constrain the shape of
    // the arg-prep ops — they execute normally and leave the call
    // args at `R[a+1]` / `R[a+2]` by the standard Lua Call ABI.
    let kind = match fname_bytes {
        b"min" => FoldKind::Min2,
        b"max" => FoldKind::Max2,
        _ => return None,
    };
    let (call_idx, _) = split_window(record, i, head_proto, a, &[3])?;
    let diag_name = if matches!(kind, FoldKind::Min2) {
        "min"
    } else {
        "max"
    };
    Some(TraceMathFold {
        start_idx: i,
        fn_name: diag_name,
        kind,
        arg_src: None,
        call_idx,
        dst_reg: a,
        arg1_reg: a + 1,
        arg2_reg: a + 2,
        nargs: 2,
    })
}

/// The `Call A B C=2` with `B` one of `bs` that closes a split-window fold
/// whose GetField is at `i + 1`, within [`MINMAX_FOLD_ARG_PREP_MAX`] ops,
/// and the number of arguments it passes. All scanned ops must come from
/// `head_proto` at depth 0. The arg-prep ops between are not constrained:
/// they execute normally and leave the call args at `R[A+1..]` by the
/// standard Lua Call ABI; they only must not overwrite the function slot.
fn split_window(
    record: &TraceRecord,
    i: usize,
    head_proto: Gc<Proto>,
    a: u32,
    bs: &[u32],
) -> Option<(usize, u32)> {
    let search_limit = (i + 2 + MINMAX_FOLD_ARG_PREP_MAX + 1).min(record.ops.len());
    for j in (i + 2)..search_limit {
        let rop_j = &record.ops[j];
        if !std::ptr::eq(rop_j.proto.as_ptr(), head_proto.as_ptr()) || rop_j.inline_depth != 0 {
            return None;
        }
        let inst_j = rop_j.inst;
        if matches!(inst_j.op(), Op::Call) {
            if inst_j.a() != a || !bs.contains(&inst_j.b()) || inst_j.c() != 2 {
                return None;
            }
            return Some((j, inst_j.b() - 1));
        }
        if inst_j.a() == a {
            return None;
        }
    }
    None
}

/// `math.floor` / `math.ceil`: on 5.3+ they keep integers integral and
/// turn a float into an integer when it fits.
pub(super) fn is_rounding(fn_name: &str) -> bool {
    matches!(fn_name, "floor" | "ceil")
}
