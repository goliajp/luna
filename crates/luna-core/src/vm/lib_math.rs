//! math library, following each dialect's `lmathlib.c`. ≤5.2 has no integer
//! subtype, so results there are floats and integer arguments go through
//! `luaL_checkint`; 5.3+ keeps integers integral (`pushnumint`). The RNG is
//! xoshiro256** (PUC 5.4+'s algorithm), state per VM.

use crate::numeric::Num;
use crate::runtime::Value;
use crate::runtime::value::f2i_exact;
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::arg_error;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

mod minmax;
mod random;
use minmax::minmax;
use random::{m_random, m_randomseed};

type Native = fn(&mut Vm, u32, u32) -> Result<u32, LuaError>;

pub(crate) fn open_math(vm: &mut Vm) {
    let ver = vm.version();
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, v: Value| {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, v)
            .expect("valid key");
    };
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
    for (name, f) in funcs {
        let fv = vm.native(f);
        set(vm, name, fv);
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
        let k = Value::Str(vm.heap.intern(of.as_bytes()));
        let fv = t.get(k);
        set(vm, name, fv);
    }
    set(vm, "pi", Value::Float(std::f64::consts::PI));
    set(vm, "huge", Value::Float(f64::INFINITY));
    if ver >= V::Lua53 {
        set(vm, "maxinteger", Value::Int(i64::MAX));
        set(vm, "mininteger", Value::Int(i64::MIN));
    }
    vm.set_global("math", Value::Table(t))
        .expect("stdlib registration");
    vm.barrier_back_table(t);
}

/// The native registered as `math.<name>`, for the functions a JIT may
/// inline: it replaces the call with its own code only while the field
/// still holds this function, since a program can assign any value to
/// it. `None` for other names.
#[doc(hidden)]
pub fn inlinable_native(name: &[u8]) -> Option<crate::runtime::value::NativeFn> {
    let f: Native = match name {
        b"sin" => m_sin,
        b"cos" => m_cos,
        b"tan" => m_tan,
        b"asin" => m_asin,
        b"acos" => m_acos,
        b"atan" => m_atan,
        b"exp" => m_exp,
        b"log" => m_log,
        b"sqrt" => m_sqrt,
        b"floor" => m_floor,
        b"ceil" => m_ceil,
        b"max" => m_max,
        b"min" => m_min,
        _ => return None,
    };
    Some(f)
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
    Ok(vm.nat_return(fs, &[v]))
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
    Ok(vm.nat_return(fs, &[v]))
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
            Ok(vm.nat_return(fs, &[v]))
        }
    };
}

float_fn!(m_sqrt, f64::sqrt);
float_fn!(m_sin, f64::sin);
float_fn!(m_cos, f64::cos);
float_fn!(m_tan, f64::tan);
float_fn!(m_asin, f64::asin);
float_fn!(m_acos, f64::acos);
float_fn!(m_exp, f64::exp);
float_fn!(m_cosh, f64::cosh);
float_fn!(m_sinh, f64::sinh);
float_fn!(m_tanh, f64::tanh);
float_fn!(m_log10, f64::log10);

fn m_deg(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = argcheck::check_number(vm, Args::new(fs, nargs), 0)?;
    // ≤5.2 divides by RADIANS_PER_DEGREE; 5.3 switched to multiplying by
    // 180/pi, which rounds differently (deg(3.7) differs in the last digit).
    let r = if vm.version() <= V::Lua52 {
        x / (std::f64::consts::PI / 180.0)
    } else {
        x * (180.0 / std::f64::consts::PI)
    };
    Ok(vm.nat_return(fs, &[Value::Float(r)]))
}

float_fn!(m_rad, |x: f64| x * (std::f64::consts::PI / 180.0));

/// frexp: x = m * 2^e with 0.5 <= |m| < 1 (or m == x for 0/inf/nan).
fn frexp(x: f64) -> (f64, i64) {
    if x == 0.0 || x.is_nan() || x.is_infinite() {
        return (x, 0);
    }
    let bits = x.to_bits();
    let exp_field = ((bits >> 52) & 0x7FF) as i64;
    if exp_field == 0 {
        // subnormal: normalize by scaling up, then adjust the exponent back
        let (m, e) = frexp(x * f64::from_bits(0x435u64 << 52)); // x * 2^54
        return (m, e - 54);
    }
    // force the stored exponent to represent 2^-1 so the mantissa lands in
    // [0.5, 1); the true exponent is then exp_field - 1022
    let m_bits = (bits & !(0x7FFu64 << 52)) | (1022u64 << 52);
    (f64::from_bits(m_bits), exp_field - 1022)
}

/// ldexp: m * 2^e, scaling in chunks so a large |e| can't overflow a single
/// power-of-two multiply.
fn ldexp(mut m: f64, mut e: i64) -> f64 {
    if m == 0.0 || m.is_nan() || m.is_infinite() {
        return m;
    }
    while e > 1023 {
        m *= f64::from_bits(0x7FEu64 << 52); // 2^1023
        e -= 1023;
        if m == 0.0 || m.is_infinite() {
            return m;
        }
    }
    while e < -1022 {
        m *= f64::from_bits(0x001u64 << 52); // 2^-1022
        e += 1022;
        if m == 0.0 || m.is_infinite() {
            return m;
        }
    }
    m * f64::from_bits(((e + 1023) as u64) << 52)
}

fn m_frexp(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = argcheck::check_number(vm, Args::new(fs, nargs), 0)?;
    let (m, e) = frexp(x);
    Ok(vm.nat_return(fs, &[Value::Float(m), Value::Int(e)]))
}

fn m_ldexp(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let m = argcheck::check_number(vm, a, 0)?;
    // The exponent is a C `int` in every version.
    let e = argcheck::check_int(vm, a, 1)?;
    Ok(vm.nat_return(fs, &[Value::Float(ldexp(m, e.into()))]))
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
    Ok(vm.nat_return(fs, &[Value::Float(r)]))
}

fn m_atan2(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let y = argcheck::check_number(vm, a, 0)?;
    let x = argcheck::check_number(vm, a, 1)?;
    Ok(vm.nat_return(fs, &[Value::Float(y.atan2(x))]))
}

fn m_pow(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = argcheck::check_number(vm, a, 0)?;
    let y = argcheck::check_number(vm, a, 1)?;
    Ok(vm.nat_return(fs, &[Value::Float(x.powf(y))]))
}

fn m_log(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = argcheck::check_number(vm, a, 0)?;
    let ver = vm.version();
    // 5.1 `log` takes no base; 5.2 added it with a log10 special case and
    // 5.3 a log2 one, each exact where the quotient would not be.
    let r = if ver == V::Lua51 || a.is_none_or_nil(vm, 1) {
        x.ln()
    } else {
        let base = argcheck::check_number(vm, a, 1)?;
        if base == 2.0 && ver >= V::Lua53 {
            x.log2()
        } else if base == 10.0 {
            x.log10()
        } else {
            x.ln() / base.ln()
        }
    };
    Ok(vm.nat_return(fs, &[Value::Float(r)]))
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
        return Ok(vm.nat_return(fs, &[Value::Int(v)]));
    }
    let x = argcheck::check_number(vm, a, 0)?;
    let y = argcheck::check_number(vm, a, 1)?;
    Ok(vm.nat_return(fs, &[Value::Float(x % y)]))
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
        return Ok(vm.nat_return(fs, &[Value::Float(ip), Value::Float(fp)]));
    }
    if let Value::Int(i) = a.get(vm, 0) {
        return Ok(vm.nat_return(fs, &[Value::Int(i), Value::Float(0.0)]));
    }
    let n = argcheck::check_number(vm, a, 0)?;
    let ip = if n < 0.0 { n.ceil() } else { n.floor() };
    let fp = if n == ip { 0.0 } else { n - ip };
    Ok(vm.nat_return(fs, &[push_numint(ip), Value::Float(fp)]))
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
    Ok(vm.nat_return(fs, &[v]))
}

fn m_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = match argcheck::check_any(vm, a, 0)? {
        Value::Int(_) => Value::Str(vm.heap.intern(b"integer")),
        Value::Float(_) => Value::Str(vm.heap.intern(b"float")),
        _ => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn m_ult(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = argcheck::check_integer(vm, a, 0)?;
    let y = argcheck::check_integer(vm, a, 1)?;
    Ok(vm.nat_return(fs, &[Value::Bool((x as u64) < (y as u64))]))
}

fn m_max(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    minmax(vm, fs, nargs, true)
}

fn m_min(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    minmax(vm, fs, nargs, false)
}
