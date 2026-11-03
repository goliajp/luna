//! A numeric `for` loop's `ForPrep` inside a side trace linked to the
//! compiled trace it reached (`TraceRecord::link_pc`): the outer body
//! enters an inner loop. The trace prepares the loop as the interpreter's
//! `OP_FORPREP` does and goes on the way the recording went; it leaves at
//! the `ForPrep` itself, the registers untouched, wherever the interpreter
//! would do something else (raise, skip the loop where the recording ran
//! it, or the other way round).

use super::*;

/// The preparation the dialect and the kinds of the loop's state select
/// (`for_operands`): integer bounds and step make an integer loop from 5.3
/// on, anything else numeric a float loop. An integer loop with a float
/// limit (`forlimit`'s clamping) and a numeric string are left to the
/// interpreter.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Prep {
    /// 5.1 / 5.2 (every loop steps in doubles) and a 5.3 float loop: the
    /// index one step back.
    Doubles,
    /// 5.3: the index starts one step back, the limit is kept.
    IntLimit,
    /// 5.4+: the unsigned count of iterations.
    IntCount,
    Float,
}

pub(super) fn emit_mid_for_prep<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
) -> Option<()> {
    let rop = oc.rop;
    if rop.inline_depth != 0 || pl.record.link_pc.is_none() {
        checkpoint("bail:mid-for-prep");
        return None;
    }
    let a = rop.inst.a() as usize;
    let v55 = rop.inst.op() == Op::ForPrep55;
    let pre53 = pl.opts.pre53 && !v55;
    let kinds = [a, a + 1, a + 2].map(|r| lw.current_kinds[r]);
    if !kinds
        .iter()
        .all(|&k| matches!(k, RegKind::Int | RegKind::Float))
    {
        checkpoint("bail:mid-for-prep-kinds");
        return None;
    }
    let int_loop = kinds[0] == RegKind::Int && kinds[2] == RegKind::Int;
    let prep = if (pl.float_only && !v55) || (pre53 && !int_loop) {
        Prep::Doubles
    } else if int_loop && kinds[1] != RegKind::Int {
        checkpoint("bail:mid-for-prep-float-limit");
        return None;
    } else if int_loop && pre53 {
        Prep::IntLimit
    } else if int_loop {
        Prep::IntCount
    } else {
        Prep::Float
    };
    let next = pl
        .record
        .ops
        .get(oc.i + 1)
        .map(|r| r.pc)
        .or(pl.record.link_pc)?;
    // 5.1–5.3 jump to the `ForLoop`; 5.4+ fall into the body or skip
    // past the loop
    let (to_loop, body, past) = (
        rop.pc + rop.inst.bx(),
        rop.pc + 1,
        rop.pc + 1 + rop.inst.bx(),
    );
    let runs = if pre53 || prep == Prep::Doubles {
        if next != to_loop {
            checkpoint("bail:mid-for-prep-next");
            return None;
        }
        None
    } else if next == body {
        Some(true)
    } else if next == past {
        Some(false)
    } else {
        checkpoint("bail:mid-for-prep-next");
        return None;
    };
    let vals = [a, a + 1, a + 2].map(|r| lw.bcx.use_var(lw.entry.regs_full[r]));
    let [init, limit, step] = vals;
    let out = match prep {
        Prep::Doubles => {
            let [x0, lim, st] = as_doubles(lw, vals, kinds);
            let pre = lw.bcx.ins().fsub(x0, st);
            bits(lw, &[pre, lim, st])
        }
        Prep::IntLimit => {
            let pre = lw.bcx.ins().isub(init, step);
            vec![pre, limit, step]
        }
        Prep::IntCount => int_count(lw, pl, oc, runs?, v55, init, limit, step),
        Prep::Float => {
            let f = as_doubles(lw, vals, kinds);
            float_count(lw, pl, oc, runs?, v55, f)
        }
    };
    let kind = match prep {
        Prep::IntLimit | Prep::IntCount => RegKind::Int,
        Prep::Doubles | Prep::Float => RegKind::Float,
    };
    // a skipped loop writes nothing
    if runs != Some(false) {
        for (k, v) in out.into_iter().enumerate() {
            lw.bcx.def_var(lw.entry.regs_full[a + k], v);
            lw.current_kinds[a + k] = kind;
        }
    }
    Some(())
}

/// The loop's state as doubles, an integer converted (`luaV_tonumber`).
fn as_doubles<E: Emit>(lw: &mut Lower<E>, vals: [Value; 3], kinds: [RegKind; 3]) -> [Value; 3] {
    let mut out = vals;
    for (o, (v, k)) in out.iter_mut().zip(vals.into_iter().zip(kinds)) {
        *o = match k {
            RegKind::Int => lw.bcx.ins().fcvt_from_sint(types::F64, v),
            _ => lw.bcx.ins().bitcast(types::F64, MemFlagsData::new(), v),
        };
    }
    out
}

/// Doubles as the registers hold them.
fn bits<E: Emit>(lw: &mut Lower<E>, vals: &[Value]) -> Vec<Value> {
    vals.iter()
        .map(|&v| lw.bcx.ins().bitcast(types::I64, MemFlagsData::new(), v))
        .collect()
}

/// Leaves at the `ForPrep` unless `ok`, then goes on.
fn stay_if<E: Emit>(lw: &mut Lower<E>, pl: &Plan<'_>, oc: &OpCx<'_>, ok: Value) {
    let (stay, leave) = (lw.bcx.create_block(), lw.bcx.create_block());
    lw.bcx.ins().brif(ok, stay, &[], leave, &[]);
    lw.bcx.switch_to_block(leave);
    lw.bcx.seal_block(leave);
    guard_exit(lw, pl, oc.rop.pc, oc.i);
    lw.bcx.switch_to_block(stay);
    lw.bcx.seal_block(stay);
}

/// 5.4+ integer loop (`forprep` with `forlimit` on an integer limit): a
/// zero step raises, an empty range skips the loop; else the unsigned
/// count `(limit - init) / step` (`(init - limit) / -step` downward).
#[allow(clippy::too_many_arguments)]
fn int_count<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    runs: bool,
    v55: bool,
    init: Value,
    limit: Value,
    step: Value,
) -> Vec<Value> {
    let nonzero = lw.bcx.ins().icmp_imm_s(IntCC::NotEqual, step, 0);
    stay_if(lw, pl, oc, nonzero);
    let up = lw.bcx.ins().icmp_imm_s(IntCC::SignedGreaterThan, step, 0);
    let above = lw.bcx.ins().icmp(IntCC::SignedGreaterThan, init, limit);
    let below = lw.bcx.ins().icmp(IntCC::SignedLessThan, init, limit);
    let empty = lw.bcx.ins().select(up, above, below);
    let as_recorded = if runs {
        lw.bcx.ins().bxor_imm_u(empty, 1)
    } else {
        empty
    };
    stay_if(lw, pl, oc, as_recorded);
    if !runs {
        return Vec::new();
    }
    let span_up = lw.bcx.ins().isub(limit, init);
    let span_down = lw.bcx.ins().isub(init, limit);
    let span = lw.bcx.ins().select(up, span_up, span_down);
    let neg = lw.bcx.ins().ineg(step);
    let by = lw.bcx.ins().select(up, step, neg);
    let count = lw.bcx.ins().udiv(span, by);
    if v55 {
        vec![count, step, init]
    } else {
        vec![init, count, step, init]
    }
}

/// 5.4+ float loop: a zero step raises; the loop is skipped when
/// `0 < step ? limit < init : init < limit` (a NaN runs it once).
fn float_count<E: Emit>(
    lw: &mut Lower<E>,
    pl: &Plan<'_>,
    oc: &OpCx<'_>,
    runs: bool,
    v55: bool,
    [x0, lim, st]: [Value; 3],
) -> Vec<Value> {
    let zero = lw.bcx.ins().f64const(0.0);
    let nonzero = lw.bcx.ins().fcmp(FloatCC::NotEqual, st, zero);
    stay_if(lw, pl, oc, nonzero);
    let up = lw.bcx.ins().fcmp(FloatCC::LessThan, zero, st);
    let past_up = lw.bcx.ins().fcmp(FloatCC::LessThan, lim, x0);
    let past_down = lw.bcx.ins().fcmp(FloatCC::LessThan, x0, lim);
    let skip = lw.bcx.ins().select(up, past_up, past_down);
    let as_recorded = if runs {
        lw.bcx.ins().bxor_imm_u(skip, 1)
    } else {
        skip
    };
    stay_if(lw, pl, oc, as_recorded);
    match (runs, v55) {
        (false, _) => Vec::new(),
        (true, true) => bits(lw, &[lim, st, x0]),
        (true, false) => bits(lw, &[x0, lim, st, x0]),
    }
}
