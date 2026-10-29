//! The math library's C functions as PUC's opening names them.

use super::*;
use crate::runtime::Builtin;
use crate::vm::opening::{Ctor, Natives, Opener, run_open};

use crate::runtime::puc::api::PucCount;
use Ctor::F;

pub(crate) const PUC_FNS: Natives = &[
    ("luaopen_math", Ctor::Open(luaopen_math)),
    ("math_abs", F(m_abs, Builtin::None)),
    ("math_acos", F(m_acos, Builtin::MathAcos)),
    ("math_asin", F(m_asin, Builtin::MathAsin)),
    ("math_atan", F(m_atan, Builtin::MathAtan)),
    ("math_atan2", F(m_atan2, Builtin::None)),
    ("math_ceil", F(m_ceil, Builtin::MathCeil)),
    ("math_cos", F(m_cos, Builtin::MathCos)),
    ("math_cosh", F(m_cosh, Builtin::None)),
    ("math_deg", F(m_deg, Builtin::None)),
    ("math_exp", F(m_exp, Builtin::MathExp)),
    ("math_floor", F(m_floor, Builtin::MathFloor)),
    ("math_fmod", F(m_fmod, Builtin::MathFmod)),
    ("math_frexp", F(m_frexp, Builtin::None)),
    ("math_ldexp", F(m_ldexp, Builtin::None)),
    ("math_log", F(m_log, Builtin::MathLog)),
    ("math_log10", F(m_log10, Builtin::None)),
    ("math_max", F(m_max, Builtin::MathMax)),
    ("math_min", F(m_min, Builtin::MathMin)),
    ("math_modf", F(m_modf, Builtin::None)),
    ("math_pow", F(m_pow, Builtin::None)),
    ("math_rad", F(m_rad, Builtin::None)),
    ("math_random", F(m_random, Builtin::None)),
    ("math_randomseed", F(m_randomseed, Builtin::None)),
    ("math_sin", F(m_sin, Builtin::MathSin)),
    ("math_sinh", F(m_sinh, Builtin::None)),
    ("math_sqrt", F(m_sqrt, Builtin::MathSqrt)),
    ("math_tan", F(m_tan, Builtin::MathTan)),
    ("math_tanh", F(m_tanh, Builtin::None)),
    ("math_toint", F(m_tointeger, Builtin::None)),
    ("math_type", Ctor::R(by_rules!(m_type), Builtin::None)),
    ("math_ult", F(m_ult, Builtin::None)),
];

fn luaopen_math(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    run_open(vm, fs, nargs, "luaopen_math", 1)
}

/// PUC's `setrandfunc` (5.4 on): the generator's state, a userdata, made
/// the one upvalue of `random` and `randomseed` (luna's generator is its
/// own; the state stands for PUC's).
pub(crate) fn puc_setrandfunc(vm: &mut Vm, op: &mut Opener) -> Result<(), LuaError> {
    op.before(vm)?;
    let size = vm.heap.puc_size(crate::runtime::puc::PucObj::RandState);
    let u = vm.lib_userdata(crate::runtime::UserdataPayload::Empty, false, size)?;
    op.push(vm, Value::Userdata(u))?;
    op.after(vm)?;
    for name in [&b"random"[..], b"randomseed"] {
        op.pushvalue(vm, -1)?;
        let f = crate::vm::opening::fn_number(if name == b"random" {
            "math_random"
        } else {
            "math_randomseed"
        });
        op.pushcclosure(vm, f, 1)?;
        let ix = op.ix(vm, -3);
        op.setfield(vm, ix, name)?;
    }
    op.pop(vm, 1);
    Ok(())
}
