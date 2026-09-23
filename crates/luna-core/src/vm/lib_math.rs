//! math library, following each dialect's `lmathlib.c`. ≤5.2 has no integer
//! subtype, so results there are floats and integer arguments go through
//! `luaL_checkint`; 5.3+ keeps integers integral (`pushnumint`). The RNG is
//! xoshiro256** (PUC 5.4+'s algorithm), state per VM.

use crate::numeric::Num;
use crate::runtime::Value;
use crate::runtime::value::f2i_exact;
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

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

/// `math.max` / `math.min`. ≤5.2 converts every argument with
/// `luaL_checknumber` and compares doubles; 5.3+ compares the arguments
/// themselves with `lua_compare` (metamethods included) and returns the
/// winner unconverted.
fn minmax(vm: &mut Vm, fs: u32, nargs: u32, want_max: bool) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    if vm.version() <= V::Lua52 {
        let mut best = argcheck::check_number(vm, a, 0)?;
        for i in 1..nargs {
            let d = argcheck::check_number(vm, a, i)?;
            if if want_max { d > best } else { d < best } {
                best = d;
            }
        }
        return Ok(vm.nat_return(fs, &[Value::Float(best)]));
    }
    if nargs == 0 {
        return Err(arg_error(vm, 1, "value expected"));
    }
    let mut best = a.get(vm, 0);
    for i in 1..nargs {
        let v = a.get(vm, i);
        let swap = match (best, v) {
            (Value::Int(x), Value::Int(y)) => {
                if want_max {
                    x < y
                } else {
                    y < x
                }
            }
            (Value::Float(x), Value::Float(y)) => {
                if want_max {
                    x < y
                } else {
                    y < x
                }
            }
            _ => {
                if want_max {
                    vm.less_than(best, v, false)?
                } else {
                    vm.less_than(v, best, false)?
                }
            }
        };
        if swap {
            best = v;
        }
    }
    Ok(vm.nat_return(fs, &[best]))
}

fn m_max(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    minmax(vm, fs, nargs, true)
}

fn m_min(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    minmax(vm, fs, nargs, false)
}

/// A float in [0, 1) from the top 53 bits (PUC 5.4 `I2d`).
fn rand_float(vm: &mut Vm) -> f64 {
    (vm.rng_next() >> 11) as f64 * (0.5 / (1u64 << 52) as f64)
}

fn m_random(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    match vm.version() {
        V::Lua51 => random_51(vm, fs, nargs),
        V::Lua52 => random_52(vm, fs, nargs),
        _ => random_53(vm, fs, nargs),
    }
}

/// 5.1: `luaL_checkint` bounds, float result.
fn random_51(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let r = rand_float(vm);
    let v = match nargs {
        0 => r,
        1 => {
            let u = argcheck::check_int(vm, a, 0)?;
            if u < 1 {
                return Err(arg_error(vm, 1, "interval is empty"));
            }
            (r * f64::from(u)).floor() + 1.0
        }
        2 => {
            let l = argcheck::check_int(vm, a, 0)?;
            let u = argcheck::check_int(vm, a, 1)?;
            if l > u {
                return Err(arg_error(vm, 2, "interval is empty"));
            }
            (r * f64::from(u.wrapping_sub(l).wrapping_add(1))).floor() + f64::from(l)
        }
        _ => return Err(raise_str(vm, "wrong number of arguments")),
    };
    Ok(vm.nat_return(fs, &[Value::Float(v)]))
}

/// 5.2: the bounds are plain numbers, so `random(3.5)` is valid.
fn random_52(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let r = rand_float(vm);
    let v = match nargs {
        0 => r,
        1 => {
            let u = argcheck::check_number(vm, a, 0)?;
            // a NaN bound fails the C comparison too
            if u < 1.0 || u.is_nan() {
                return Err(arg_error(vm, 1, "interval is empty"));
            }
            (r * u).floor() + 1.0
        }
        2 => {
            let l = argcheck::check_number(vm, a, 0)?;
            let u = argcheck::check_number(vm, a, 1)?;
            if l > u || l.is_nan() || u.is_nan() {
                return Err(arg_error(vm, 2, "interval is empty"));
            }
            (r * (u - l + 1.0)).floor() + l
        }
        _ => return Err(raise_str(vm, "wrong number of arguments")),
    };
    Ok(vm.nat_return(fs, &[Value::Float(v)]))
}

/// 5.3+: integer bounds; both emptiness checks blame argument 1.
fn random_53(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v53 = vm.version() == V::Lua53;
    let (low, up) = match nargs {
        0 => {
            let r = rand_float(vm);
            return Ok(vm.nat_return(fs, &[Value::Float(r)]));
        }
        1 => {
            let up = argcheck::check_integer(vm, a, 0)?;
            // 5.4: a single 0 asks for all 64 random bits.
            if up == 0 && !v53 {
                let v = Value::Int(vm.rng_next() as i64);
                return Ok(vm.nat_return(fs, &[v]));
            }
            (1, up)
        }
        2 => (
            argcheck::check_integer(vm, a, 0)?,
            argcheck::check_integer(vm, a, 1)?,
        ),
        _ => return Err(raise_str(vm, "wrong number of arguments")),
    };
    if low > up {
        return Err(arg_error(vm, 1, "interval is empty"));
    }
    if v53 {
        // 5.3 scales a float in [0, 1), so the interval must fit an integer.
        if low < 0 && up > i64::MAX.wrapping_add(low) {
            return Err(arg_error(vm, 1, "interval too large"));
        }
        let r = rand_float(vm) * ((up - low) as f64 + 1.0);
        return Ok(vm.nat_return(fs, &[Value::Int((r as i64).wrapping_add(low))]));
    }
    let n = (up as u64).wrapping_sub(low as u64);
    let p = project(vm, n);
    Ok(vm.nat_return(fs, &[Value::Int(p.wrapping_add(low as u64) as i64)]))
}

/// PUC 5.4 `project`: mask the random value down to the smallest Mersenne
/// number not below `n` and retry until it lands in [0, n].
fn project(vm: &mut Vm, n: u64) -> u64 {
    let mut ran = vm.rng_next();
    let mut lim = n;
    let mut sh = 1;
    while lim & lim.wrapping_add(1) != 0 {
        lim |= lim >> sh;
        sh *= 2;
    }
    loop {
        ran &= lim;
        if ran <= n {
            return ran;
        }
        ran = vm.rng_next();
    }
}

fn m_randomseed(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    // ≤5.3 seeds C `srand` and returns nothing. luna has one generator for
    // every dialect; only the argument rules and the result count differ.
    let seed: u64 = match vm.version() {
        V::Lua51 => argcheck::check_int(vm, a, 0)? as u64,
        V::Lua52 => argcheck::check_unsigned52(vm, a, 0)?.into(),
        V::Lua53 => argcheck::check_number(vm, a, 0)? as i64 as u64,
        _ => {
            let (n1, n2) = if a.is_none(0) {
                vm.rng_auto_seed()
            } else {
                (
                    argcheck::check_integer(vm, a, 0)?,
                    argcheck::opt_integer(vm, a, 1, 0)?,
                )
            };
            vm.rng_seed(n1 as u64, n2 as u64);
            return Ok(vm.nat_return(fs, &[Value::Int(n1), Value::Int(n2)]));
        }
    };
    vm.rng_seed(seed, 0);
    Ok(0)
}
