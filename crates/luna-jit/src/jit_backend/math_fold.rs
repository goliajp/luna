//! Recognizing `math.<fn>(arg)` call windows the method JIT folds into a
//! libm call.

use super::*;

/// try to recognize the 4-op `<env>.math.<fn>(R[arg])` window
/// starting at `start_pc`. Returns `Some(MathFold)` on match, `None`
/// otherwise. Pure inspection — no side effects, no whitelist
/// promotion. Caller (`try_compile_int_chunk`'s pre-scan) marks the
/// participating PCs in `folded_math[]` and pushes the fold to
/// `math_folds`.
pub(super) fn try_match_math_fold(
    proto: &Proto,
    code: &[Inst],
    start_pc: usize,
    float_only: bool,
) -> Option<MathFold> {
    let i0 = *code.get(start_pc)?;
    let i1 = *code.get(start_pc + 1)?;
    let i2 = *code.get(start_pc + 2)?;
    let i3 = *code.get(start_pc + 3)?;

    if !matches!(i0.op(), Op::GetTabUp) {
        return None;
    }
    if !matches!(i1.op(), Op::GetField) {
        return None;
    }
    if !matches!(i2.op(), Op::Move) {
        return None;
    }
    if !matches!(i3.op(), Op::Call) {
        return None;
    }

    let a = i0.a();
    // GetTabUp reads upvals[B] indexed by consts[C]. We pin B=0
    // (env upvalue). The frontend invariant (`env_upval_present`
    // check in `try_compile_int_chunk`) guarantees upvals[0].name
    // == "_ENV".
    if i0.b() != 0 {
        return None;
    }
    let k_math = proto.consts.get(i0.c() as usize).copied()?;
    let LuaValue::Str(s) = k_math else {
        return None;
    };
    if s.as_bytes() != b"math" {
        return None;
    }

    // GetField R[A] = R[A].<key>. Same dest as source — the GetTabUp
    // result is consumed in place.
    if i1.a() != a || i1.b() != a {
        return None;
    }
    let k_fn = proto.consts.get(i1.c() as usize).copied()?;
    let LuaValue::Str(fname) = k_fn else {
        return None;
    };
    let fn_name = MATH_LIBM_FNS
        .iter()
        .find_map(|&(needle, name)| (needle == fname.as_bytes()).then_some(name))?;

    // Move R[A+1] = R[arg]. The destination must be the Call's arg
    // slot.
    if i2.a() != a + 1 {
        return None;
    }
    let arg_reg = i2.b();

    // Call R[A], B=2 (1 arg), C=2 (1 return).
    if i3.a() != a || i3.b() != 2 || i3.c() != 2 {
        return None;
    }

    Some(MathFold {
        start_pc,
        fn_name,
        arg_reg,
        dst_reg: a,
        int_result: !float_only && is_rounding(fn_name),
        math_key: s,
        name_key: fname,
    })
}
