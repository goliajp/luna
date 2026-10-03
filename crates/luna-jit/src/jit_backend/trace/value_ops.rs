use super::*;

/// Whether the float `r` (an integral value or NaN / ±inf) converts to
/// an i64: `-2^63 <= r < 2^63`. NaN fails both comparisons.
pub(in crate::jit_backend) fn emit_f64_fits_i64<E: Ins>(bcx: &mut E, r: Value) -> Value {
    let lo = bcx.ins().f64const(-9_223_372_036_854_775_808.0);
    let hi = bcx.ins().f64const(9_223_372_036_854_775_808.0);
    let ge_lo = bcx.ins().fcmp(FloatCC::GreaterThanOrEqual, r, lo);
    let lt_hi = bcx.ins().fcmp(FloatCC::LessThan, r, hi);
    bcx.ins().band(ge_lo, lt_hi)
}

/// `i < f` for an integer and a float, exactly (lvm.c `LTintfloat`):
/// `i < f` iff `i < ceil(f)`, with a NaN `f` false and an `f` beyond the
/// integer range deciding by its sign.
pub(super) fn emit_lt_int_float<E: Ins>(bcx: &mut E, i: Value, f: Value) -> Value {
    let c = bcx.ins().ceil(f);
    let ci = bcx.ins().fcvt_to_sint_sat(types::I64, c);
    let in_range = emit_f64_fits_i64(bcx, c);
    let lt = bcx.ins().icmp(IntCC::SignedLessThan, i, ci);
    let zero = bcx.ins().f64const(0.0);
    // Out of range (or NaN): true iff f is above every integer.
    let above = bcx.ins().fcmp(FloatCC::GreaterThan, f, zero);
    bcx.ins().select(in_range, lt, above)
}

/// `f < i` for a float and an integer, exactly (lvm.c `LTfloatint`):
/// `f < i` iff `floor(f) < i`, with a NaN `f` false and an `f` beyond the
/// integer range deciding by its sign.
pub(super) fn emit_lt_float_int<E: Ins>(bcx: &mut E, f: Value, i: Value) -> Value {
    let fl = bcx.ins().floor(f);
    let fi = bcx.ins().fcvt_to_sint_sat(types::I64, fl);
    let in_range = emit_f64_fits_i64(bcx, fl);
    let lt = bcx.ins().icmp(IntCC::SignedLessThan, fi, i);
    let zero = bcx.ins().f64const(0.0);
    let below = bcx.ins().fcmp(FloatCC::LessThan, f, zero);
    bcx.ins().select(in_range, lt, below)
}

/// How a trace lowers `==` of two registers of the given kinds, neither
/// of them Float (those take the fcmp path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum EqLowering {
    /// equal exactly when the payloads are
    Payload,
    /// different types: never equal (nil and integer 0 share payload 0)
    Unequal,
    /// equal payloads mean equal; different ones need a guard
    Identity(RegKind),
    /// a value of unknown type: the payload compare is only a guess,
    /// so the trace must not be dispatched
    Unknown,
}

pub(super) fn eq_lowering(a: RegKind, b: RegKind) -> EqLowering {
    use RegKind::*;
    match (a, b) {
        (Unset | Unknown | StackHeld, _) | (_, Unset | Unknown | StackHeld) => EqLowering::Unknown,
        (Int, Int) | (Nil, Nil) | (Closure, Closure) | (Bool, Bool) => EqLowering::Payload,
        (Table, Table) | (Str, Str) => EqLowering::Identity(a),
        _ => EqLowering::Unequal,
    }
}

/// Cast a Variable's i64 payload into f64 if its kind is Float.
pub(super) fn use_var_f64<E: Ins>(bcx: &mut E, regs: &[Variable], reg: u32) -> Value {
    let raw = bcx.use_var(regs[reg as usize]);
    bcx.ins().bitcast(types::F64, MemFlagsData::new(), raw)
}

/// Read a number register of kind `kind` (Int or Float) as an f64.
pub(super) fn use_var_as_f64<E: Ins>(
    bcx: &mut E,
    regs: &[Variable],
    reg: u32,
    kind: RegKind,
) -> Value {
    if matches!(kind, RegKind::Float) {
        use_var_f64(bcx, regs, reg)
    } else {
        let raw = bcx.use_var(regs[reg as usize]);
        bcx.ins().fcvt_from_sint(types::F64, raw)
    }
}

/// Store an f64 SSA value into a Variable as i64 bits.
pub(super) fn def_var_f64<E: Ins>(bcx: &mut E, var: Variable, val_f64: Value) {
    let bits = bcx.ins().bitcast(types::I64, MemFlagsData::new(), val_f64);
    bcx.def_var(var, bits);
}
