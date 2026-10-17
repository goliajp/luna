//! math library, following each dialect's `lmathlib.c`. ≤5.2 has no integer
//! subtype, so results there are floats and integer arguments go through
//! `luaL_checkint`; 5.3+ keeps integers integral (`pushnumint`). The RNG is
//! xoshiro256** (PUC 5.4+'s algorithm), state per VM.

use crate::cerrno::{self, MathFn};
use crate::numeric::Num;
use crate::runtime::value::f2i_exact;
use crate::runtime::{Builtin, Value};
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::arg_error;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

mod frexp;
mod minmax;
mod random;
use frexp::{frexp, ldexp};
use minmax::minmax;
use random::{m_random, m_randomseed};

type Native = fn(&mut Vm, u32, u32) -> Result<u32, LuaError>;

/// Open the `math` library, failing when memory runs out.
pub(crate) fn try_open_math(vm: &mut Vm) -> Result<(), LuaError> {
    let ver = vm.version();
    let mut funcs: Vec<(&str, Native)> = vec![
        ("abs", m_abs),
        ("ceil", m_ceil),
        ("floor", m_floor),
        ("sqrt", m_sqrt),
        ("sin", m_sin),
        ("cos", m_cos),
        ("tan", m_tan),
        ("asin", m_asin),
        ("acos", m_acos),
        ("atan", m_atan),
        ("exp", m_exp),
        ("deg", m_deg),
        ("rad", m_rad),
        ("frexp", m_frexp),
        ("ldexp", m_ldexp),
        ("log", m_log),
        ("fmod", m_fmod),
        ("modf", m_modf),
        ("max", m_max),
        ("min", m_min),
        ("random", m_random),
        ("randomseed", m_randomseed),
    ];
    if ver >= V::Lua53 {
        funcs.extend([
            ("tointeger", m_tointeger as Native),
            ("type", m_type),
            ("ult", m_ult),
        ]);
    }
    // The pre-5.3 functions: native in 5.1/5.2, kept by the default
    // LUA_COMPAT_MATHLIB of the 5.3 and 5.4 builds, gone in 5.5.
    if ver <= V::Lua54 {
        funcs.extend([
            ("cosh", m_cosh as Native),
            ("sinh", m_sinh),
            ("tanh", m_tanh),
            ("pow", m_pow),
            ("log10", m_log10),
        ]);
    }
    if ver <= V::Lua52 {
        funcs.push(("atan2", m_atan2));
    }
    let t = vm.lib_table_puc("math")?;
    let mut entries: Vec<(&str, Value)> = Vec::with_capacity(funcs.len() + 6);
    // 5.4 on, counted: the generator's functions come last, as PUC's
    // `setrandfunc` makes them, over the state they share
    let rand_last = ver >= V::Lua54 && vm.heap.puc.silent == 0;
    for (name, f) in funcs {
        if rand_last && name.starts_with("random") {
            continue;
        }
        let b = inlinable_builtin(name.as_bytes()).unwrap_or(Builtin::None);
        let fv = vm.builtin(f, &[], b)?;
        // 5.4 on, the generator's functions are C closures over its state
        if ver >= V::Lua54
            && name.starts_with("random")
            && let Value::Native(n) = fv
        {
            vm.heap.set_native_puc_nup(n, 1);
        }
        entries.push((name, fv));
    }
    // Aliases are the same function value, so `math.atan2 == math.atan`
    // holds: 5.3/5.4 register `atan2` as `math_atan`, and 5.1's
    // LUA_COMPAT_MOD copies the `fmod` field to `mod`.
    let alias = match ver {
        V::Lua51 => Some(("mod", "fmod")),
        V::Lua53 | V::Lua54 => Some(("atan2", "atan")),
        _ => None,
    };
    if let Some((name, of)) = alias {
        let fv = entries.iter().find(|(k, _)| *k == of).map(|e| e.1);
        entries.push((name, fv.expect("the aliased function")));
    }
    entries.push(("pi", Value::Float(std::f64::consts::PI)));
    entries.push(("huge", Value::Float(f64::INFINITY)));
    if ver >= V::Lua53 {
        entries.push(("maxinteger", Value::Int(i64::MAX)));
        entries.push(("mininteger", Value::Int(i64::MIN)));
    }
    if rand_last {
        let sz = vm.heap.sizes;
        use crate::runtime::heap::PucKind;
        let (ud, cl) = (
            (PucKind::Userdata, sz.userdata(0, 32)),
            (PucKind::CClosure, sz.c_closure(1)),
        );
        // collectable, as PUC's closures are: not a light function
        let fix = std::mem::replace(&mut vm.heap.fix_natives, false);
        let r = vm.lib_native_stand_in(m_random, &[], &[ud, cl], 0);
        let s = r.and_then(|r| Ok((r, vm.lib_native_stand_in(m_randomseed, &[], &[cl], 0)?)));
        vm.heap.fix_natives = fix;
        let (r, s) = s?;
        // each a C closure over the generator's state
        for f in [r, s] {
            if let Value::Native(n) = f {
                vm.heap.set_native_puc_nup(n, 1);
            }
        }
        entries.push(("random", r));
        entries.push(("randomseed", s));
    }
    vm.lib_put_all(t, "math", entries)?;
    vm.lib_set_global("math", Value::Table(t))?;
    vm.barrier_back_table(t);
    Ok(())
}

/// The library function `math.<name>` is, for the functions a JIT may
/// inline: it replaces the call with its own code only while the field
/// still holds that function, since a program can assign any value to
/// it. `None` for other names.
#[doc(hidden)]
pub fn inlinable_builtin(name: &[u8]) -> Option<Builtin> {
    Some(match name {
        b"sin" => Builtin::MathSin,
        b"cos" => Builtin::MathCos,
        b"tan" => Builtin::MathTan,
        b"asin" => Builtin::MathAsin,
        b"acos" => Builtin::MathAcos,
        b"atan" => Builtin::MathAtan,
        b"exp" => Builtin::MathExp,
        b"log" => Builtin::MathLog,
        b"sqrt" => Builtin::MathSqrt,
        b"floor" => Builtin::MathFloor,
        b"ceil" => Builtin::MathCeil,
        b"max" => Builtin::MathMax,
        b"min" => Builtin::MathMin,
        b"fmod" => Builtin::MathFmod,
        _ => return None,
    })
}

/// PUC `pushnumint`: a float that fits an integer becomes one.
fn push_numint(f: f64) -> Value {
    match f2i_exact(f) {
        Some(i) => Value::Int(i),
        None => Value::Float(f),
    }
}

fn m_abs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // ≤5.2 has no Int subtype to overflow, so the integer fast path is only
    // an unobservable shortcut there.
    let v = match a.get(vm, 0) {
        Value::Int(i) => Value::Int(i.wrapping_abs()),
        _ => Value::Float(argcheck::check_number(vm, a, 0)?.abs()),
    };
    vm.nat_ret(fs, &[v])
}

/// `math.floor` / `math.ceil`: 5.3+ returns an integer when the result fits;
/// ≤5.2 pushes the float, which keeps `-0.0` and huge values intact.
fn round_with(vm: &mut Vm, fs: u32, nargs: u32, op: fn(f64) -> f64) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = match a.get(vm, 0) {
        Value::Int(i) => Value::Int(i),
        _ => {
            let r = op(argcheck::check_number(vm, a, 0)?);
            if vm.version() <= V::Lua52 {
                Value::Float(r)
            } else {
                push_numint(r)
            }
        }
    };
    vm.nat_ret(fs, &[v])
}

fn m_floor(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    round_with(vm, fs, nargs, f64::floor)
}

fn m_ceil(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    round_with(vm, fs, nargs, f64::ceil)
}

macro_rules! float_fn {
    ($name:ident, $op:expr) => {
        fn $name(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
            let x = argcheck::check_number(vm, Args::new(fs, nargs), 0)?;
            #[allow(clippy::redundant_closure_call)]
            let v = Value::Float(($op)(x));
            vm.nat_ret(fs, &[v])
        }
    };
}

float_fn!(m_sqrt, |x| cerrno::math1(MathFn::Sqrt, x));
float_fn!(m_sin, |x| cerrno::math1(MathFn::Sin, x));
float_fn!(m_cos, |x| cerrno::math1(MathFn::Cos, x));
float_fn!(m_tan, |x| cerrno::math1(MathFn::Tan, x));
float_fn!(m_asin, |x| cerrno::math1(MathFn::Asin, x));
float_fn!(m_acos, |x| cerrno::math1(MathFn::Acos, x));
float_fn!(m_exp, |x| cerrno::math1(MathFn::Exp, x));
float_fn!(m_cosh, |x| cerrno::math1(MathFn::Cosh, x));
float_fn!(m_sinh, |x| cerrno::math1(MathFn::Sinh, x));
float_fn!(m_tanh, f64::tanh);
float_fn!(m_log10, |x| cerrno::math1(MathFn::Log10, x));

fn m_deg(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = argcheck::check_number(vm, Args::new(fs, nargs), 0)?;
    // ≤5.2 divides by RADIANS_PER_DEGREE; 5.3 switched to multiplying by
    // 180/pi, which rounds differently (deg(3.7) differs in the last digit).
    let r = if vm.version() <= V::Lua52 {
        x / (std::f64::consts::PI / 180.0)
    } else {
        x * (180.0 / std::f64::consts::PI)
    };
    vm.nat_ret(fs, &[Value::Float(r)])
}

float_fn!(m_rad, |x: f64| x * (std::f64::consts::PI / 180.0));

fn m_frexp(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = argcheck::check_number(vm, Args::new(fs, nargs), 0)?;
    let (m, e) = frexp(x);
    vm.nat_ret(fs, &[Value::Float(m), Value::Int(e)])
}

fn m_ldexp(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let m = argcheck::check_number(vm, a, 0)?;
    // The exponent is a C `int` in every version.
    let e = argcheck::check_int(vm, a, 1)?;
    let r = cerrno::ldexp_applied(m, ldexp(m, e.into()));
    vm.nat_ret(fs, &[Value::Float(r)])
}

fn m_atan(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let y = argcheck::check_number(vm, a, 0)?;
    // ≤5.2 `atan` is one-argument; the two-argument form is `atan2`.
    let r = if vm.version() <= V::Lua52 {
        y.atan()
    } else {
        y.atan2(argcheck::opt_number(vm, a, 1, 1.0)?)
    };
    vm.nat_ret(fs, &[Value::Float(r)])
}

fn m_atan2(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let y = argcheck::check_number(vm, a, 0)?;
    let x = argcheck::check_number(vm, a, 1)?;
    vm.nat_ret(fs, &[Value::Float(y.atan2(x))])
}

fn m_pow(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = argcheck::check_number(vm, a, 0)?;
    let y = argcheck::check_number(vm, a, 1)?;
    vm.nat_ret(fs, &[Value::Float(cerrno::pow(x, y))])
}

fn m_log(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = argcheck::check_number(vm, a, 0)?;
    let ver = vm.version();
    // 5.1 `log` takes no base; 5.2 added it with a log10 special case and
    // 5.3 a log2 one, each exact where the quotient would not be.
    let log = |x| cerrno::math1(MathFn::Log, x);
    let r = if ver == V::Lua51 || a.is_none_or_nil(vm, 1) {
        log(x)
    } else {
        let base = argcheck::check_number(vm, a, 1)?;
        if base == 2.0 && ver >= V::Lua53 {
            cerrno::math1(MathFn::Log2, x)
        } else if base == 10.0 {
            cerrno::math1(MathFn::Log10, x)
        } else {
            // both calls leave their effect, the left one first
            let lx = log(x);
            lx / log(base)
        }
    };
    vm.nat_ret(fs, &[Value::Float(r)])
}

fn m_fmod(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() >= V::Lua53
        && let (Value::Int(x), Value::Int(d)) = (a.get(vm, 0), a.get(vm, 1))
    {
        // C `%` truncates, unlike the `%` operator; -1 is special-cased
        // because mininteger % -1 overflows in C.
        let v = match d {
            0 => return Err(arg_error(vm, 2, "zero")),
            -1 => 0,
            _ => x % d,
        };
        return vm.nat_ret(fs, &[Value::Int(v)]);
    }
    let x = argcheck::check_number(vm, a, 0)?;
    let y = argcheck::check_number(vm, a, 1)?;
    vm.nat_ret(fs, &[Value::Float(cerrno::fmod(x, y))])
}

fn m_modf(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() <= V::Lua52 {
        // C `modf`: both parts keep the sign of x (so -0.0 and -inf give
        // a -0.0 fraction).
        let x = argcheck::check_number(vm, a, 0)?;
        let ip = x.trunc();
        let fp = if x.is_infinite() {
            0.0f64.copysign(x)
        } else {
            x - ip
        };
        let fp = if fp == 0.0 { 0.0f64.copysign(x) } else { fp };
        return vm.nat_ret(fs, &[Value::Float(ip), Value::Float(fp)]);
    }
    if let Value::Int(i) = a.get(vm, 0) {
        return vm.nat_ret(fs, &[Value::Int(i), Value::Float(0.0)]);
    }
    let n = argcheck::check_number(vm, a, 0)?;
    let ip = if n < 0.0 { n.ceil() } else { n.floor() };
    let fp = if n == ip { 0.0 } else { n - ip };
    vm.nat_ret(fs, &[push_numint(ip), Value::Float(fp)])
}

fn m_tointeger(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // `lua_tointegerx`: numeric strings convert too, floats only when exact.
    let n = match argcheck::to_num(vm, a.get(vm, 0)) {
        Some(Num::Int(i)) => Some(i),
        Some(Num::Float(f)) => f2i_exact(f),
        None => None,
    };
    let v = match n {
        Some(i) if !a.is_none(0) => Value::Int(i),
        _ => {
            argcheck::check_any(vm, a, 0)?;
            Value::Nil
        }
    };
    vm.nat_ret(fs, &[v])
}

fn m_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = match argcheck::check_any(vm, a, 0)? {
        Value::Int(_) => vm.api_push_string(b"integer")?,
        Value::Float(_) => vm.api_push_string(b"float")?,
        _ => Value::Nil,
    };
    vm.nat_ret(fs, &[v])
}

fn m_ult(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = argcheck::check_integer(vm, a, 0)?;
    let y = argcheck::check_integer(vm, a, 1)?;
    vm.nat_ret(fs, &[Value::Bool((x as u64) < (y as u64))])
}

fn m_max(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    minmax(vm, fs, nargs, true)
}

fn m_min(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    minmax(vm, fs, nargs, false)
}
