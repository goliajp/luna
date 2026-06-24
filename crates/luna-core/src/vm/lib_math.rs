//! math library. Integer/float dual semantics follow PUC lmathlib; the RNG
//! is xoshiro256** (PUC 5.4+'s algorithm), state per VM.

use crate::numeric::Num;
use crate::runtime::Value;
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_math(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    };
    set(vm, "abs", m_abs);
    set(vm, "ceil", m_ceil);
    set(vm, "floor", m_floor);
    set(vm, "sqrt", m_sqrt);
    set(vm, "sin", m_sin);
    set(vm, "cos", m_cos);
    set(vm, "tan", m_tan);
    set(vm, "asin", m_asin);
    set(vm, "acos", m_acos);
    set(vm, "atan", m_atan);
    set(vm, "exp", m_exp);
    set(vm, "deg", m_deg);
    set(vm, "rad", m_rad);
    set(vm, "frexp", m_frexp);
    set(vm, "ldexp", m_ldexp);
    set(vm, "log", m_log);
    set(vm, "fmod", m_fmod);
    set(vm, "modf", m_modf);
    set(vm, "tointeger", m_tointeger);
    set(vm, "type", m_type);
    set(vm, "ult", m_ult);
    set(vm, "max", m_max);
    set(vm, "min", m_min);
    set(vm, "random", m_random);
    set(vm, "randomseed", m_randomseed);
    // Legacy math entries kept for 5.1/5.2 (PUC 5.3 dropped them in favour of
    // the new `^` operator, `math.atan(y, x)`, and the integer subtype).
    // Always registered so cross-version libraries that happen to use them
    // still load — the version-aware tests don't assert their absence.
    if vm.version() <= crate::version::LuaVersion::Lua52 {
        set(vm, "atan2", m_atan2);
        set(vm, "cosh", m_cosh);
        set(vm, "sinh", m_sinh);
        set(vm, "tanh", m_tanh);
        set(vm, "log10", m_log10);
        set(vm, "pow", m_pow);
        set(vm, "mod", m_fmod);
    }
    let consts: [(&str, Value); 4] = [
        ("pi", Value::Float(std::f64::consts::PI)),
        ("huge", Value::Float(f64::INFINITY)),
        ("maxinteger", Value::Int(i64::MAX)),
        ("mininteger", Value::Int(i64::MIN)),
    ];
    for (name, v) in consts {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, v)
            .expect("valid key");
    }
    vm.set_global("math", Value::Table(t))
        .expect("stdlib registration");
    vm.barrier_back_table(t);
}

fn check_num(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<Num, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Int(x) => Ok(Num::Int(x)),
        Value::Float(x) => Ok(Num::Float(x)),
        Value::Str(s) => crate::numeric::str2num(s.as_bytes(), true, true)
            .ok_or_else(|| arg_error(vm, i + 1, who, "number expected, got string")),
        v => {
            let tn = vm.obj_typename(v);
            Err(arg_error(
                vm,
                i + 1,
                who,
                &format!("number expected, got {tn}"),
            ))
        }
    }
}

fn check_f64(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<f64, LuaError> {
    Ok(check_num(vm, fs, nargs, i, who)?.as_f64())
}

fn check_int(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<i64, LuaError> {
    match check_num(vm, fs, nargs, i, who)? {
        Num::Int(x) => Ok(x),
        Num::Float(f) => crate::runtime::value::f2i_exact(f)
            .ok_or_else(|| arg_error(vm, i + 1, who, "number has no integer representation")),
    }
}

/// PUC pushnumint: float with an exact integer value becomes an integer.
fn push_numint(f: f64) -> Value {
    match crate::runtime::value::f2i_exact(f) {
        Some(i) => Value::Int(i),
        None => Value::Float(f),
    }
}

fn m_abs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = match check_num(vm, fs, nargs, 0, "abs")? {
        Num::Int(i) => Value::Int(i.wrapping_abs()),
        Num::Float(f) => Value::Float(f.abs()),
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn m_floor(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = match check_num(vm, fs, nargs, 0, "floor")? {
        Num::Int(i) => Value::Int(i),
        Num::Float(f) => push_numint(f.floor()),
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn m_ceil(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = match check_num(vm, fs, nargs, 0, "ceil")? {
        Num::Int(i) => Value::Int(i),
        Num::Float(f) => push_numint(f.ceil()),
    };
    Ok(vm.nat_return(fs, &[v]))
}

macro_rules! float_fn {
    ($name:ident, $who:literal, $op:expr) => {
        fn $name(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
            let x = check_f64(vm, fs, nargs, 0, $who)?;
            #[allow(clippy::redundant_closure_call)]
            let v = Value::Float(($op)(x));
            Ok(vm.nat_return(fs, &[v]))
        }
    };
}

float_fn!(m_sqrt, "sqrt", |x: f64| x.sqrt());
float_fn!(m_sin, "sin", |x: f64| x.sin());
float_fn!(m_cos, "cos", |x: f64| x.cos());
float_fn!(m_tan, "tan", |x: f64| x.tan());
float_fn!(m_asin, "asin", |x: f64| x.asin());
float_fn!(m_acos, "acos", |x: f64| x.acos());
float_fn!(m_exp, "exp", |x: f64| x.exp());
float_fn!(m_deg, "deg", |x: f64| x.to_degrees());
float_fn!(m_rad, "rad", |x: f64| x.to_radians());

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
    let x = check_f64(vm, fs, nargs, 0, "frexp")?;
    let (m, e) = frexp(x);
    Ok(vm.nat_return(fs, &[Value::Float(m), Value::Int(e)]))
}

fn m_ldexp(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let m = check_f64(vm, fs, nargs, 0, "ldexp")?;
    let e = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an exponent")?;
    Ok(vm.nat_return(fs, &[Value::Float(ldexp(m, e))]))
}

fn m_atan(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let y = check_f64(vm, fs, nargs, 0, "atan")?;
    let x = if nargs >= 2 {
        check_f64(vm, fs, nargs, 1, "atan")?
    } else {
        1.0
    };
    Ok(vm.nat_return(fs, &[Value::Float(y.atan2(x))]))
}

fn m_log(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = check_f64(vm, fs, nargs, 0, "log")?;
    let v = if nargs >= 2 {
        let base = check_f64(vm, fs, nargs, 1, "log")?;
        if base == 2.0 {
            x.log2()
        } else if base == 10.0 {
            x.log10()
        } else {
            x.ln() / base.ln()
        }
    } else {
        x.ln()
    };
    Ok(vm.nat_return(fs, &[Value::Float(v)]))
}

fn m_fmod(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = check_num(vm, fs, nargs, 0, "fmod")?;
    let b = check_num(vm, fs, nargs, 1, "fmod")?;
    let v = match (a, b) {
        (Num::Int(a), Num::Int(b)) => {
            if b == 0 {
                return Err(arg_error(vm, 2, "fmod", "zero"));
            }
            // C fmod truncates (unlike the % operator's floor semantics)
            Value::Int(a.wrapping_rem(b))
        }
        (a, b) => Value::Float(a.as_f64() % b.as_f64()),
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn m_modf(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC fast path: an integer argument is returned unchanged (+ 0.0)
    if let Value::Int(i) = vm.nat_arg(fs, nargs, 0) {
        return Ok(vm.nat_return(fs, &[Value::Int(i), Value::Float(0.0)]));
    }
    let x = check_f64(vm, fs, nargs, 0, "modf")?;
    let ip = if x >= 0.0 { x.floor() } else { x.ceil() };
    let fp = if x.is_infinite() { 0.0 } else { x - ip };
    Ok(vm.nat_return(fs, &[Value::Float(ip), Value::Float(fp)]))
}

fn m_tointeger(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = match vm.nat_arg(fs, nargs, 0) {
        Value::Int(i) => Value::Int(i),
        Value::Float(f) => crate::runtime::value::f2i_exact(f)
            .map(Value::Int)
            .unwrap_or(Value::Nil),
        Value::Str(s) => match crate::numeric::str2num(s.as_bytes(), true, true) {
            Some(Num::Int(i)) => Value::Int(i),
            Some(Num::Float(f)) => crate::runtime::value::f2i_exact(f)
                .map(Value::Int)
                .unwrap_or(Value::Nil),
            None => Value::Nil,
        },
        _ => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn m_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = match vm.nat_arg(fs, nargs, 0) {
        Value::Int(_) => Value::Str(vm.heap.intern(b"integer")),
        Value::Float(_) => Value::Str(vm.heap.intern(b"float")),
        _ => Value::Nil,
    };
    Ok(vm.nat_return(fs, &[v]))
}

fn m_ult(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = check_int(vm, fs, nargs, 0, "ult")?;
    let b = check_int(vm, fs, nargs, 1, "ult")?;
    Ok(vm.nat_return(fs, &[Value::Bool((a as u64) < (b as u64))]))
}

fn minmax(vm: &mut Vm, fs: u32, nargs: u32, who: &str, want_max: bool) -> Result<u32, LuaError> {
    if nargs == 0 {
        return Err(raise_str(
            vm,
            &format!("bad argument #1 to '{who}' (value expected)"),
        ));
    }
    let mut best = vm.nat_arg(fs, nargs, 0);
    check_num(vm, fs, nargs, 0, who)?;
    for i in 1..nargs {
        check_num(vm, fs, nargs, i, who)?;
        let v = vm.nat_arg(fs, nargs, i);
        let swap = if want_max {
            vm.less_than(best, v, false)?
        } else {
            vm.less_than(v, best, false)?
        };
        if swap {
            best = v;
        }
    }
    Ok(vm.nat_return(fs, &[best]))
}

fn m_max(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    minmax(vm, fs, nargs, "max", true)
}

fn m_min(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    minmax(vm, fs, nargs, "min", false)
}

fn m_random(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (lo, hi) = match nargs {
        0 => {
            // float in [0, 1): top 53 bits
            let bits = vm.rng_next() >> 11;
            let v = Value::Float(bits as f64 * (1.0 / (1u64 << 53) as f64));
            return Ok(vm.nat_return(fs, &[v]));
        }
        1 => {
            let m = check_int(vm, fs, nargs, 0, "random")?;
            if m == 0 {
                // random(0): all 64 bits as an integer
                let v = Value::Int(vm.rng_next() as i64);
                return Ok(vm.nat_return(fs, &[v]));
            }
            (1, m)
        }
        2 => (
            check_int(vm, fs, nargs, 0, "random")?,
            check_int(vm, fs, nargs, 1, "random")?,
        ),
        _ => return Err(raise_str(vm, "wrong number of arguments")),
    };
    if lo > hi {
        return Err(arg_error(vm, nargs.min(2), "random", "interval is empty"));
    }
    // PUC 5.3 `math.random`: bounds the interval by `up <= MAXINTEGER + low`
    // (lmathlib.c). 5.4 rebuilt random on a 64-bit RNG and dropped the
    // check. math.lua 5.3 :800-:803 still expect huge non-overflowing
    // ranges (`0..maxint`, `minint..-1`) to succeed, so encoding the exact
    // PUC condition keeps both the success and the :819+ failure cases.
    if vm.version() <= crate::version::LuaVersion::Lua53 && lo < 0 && hi > i64::MAX.wrapping_add(lo)
    {
        return Err(arg_error(vm, nargs.min(2), "random", "interval too large"));
    }
    // PUC project(): uniform in [0, range] by rejection
    let range = (hi as u64).wrapping_sub(lo as u64);
    let v = if range == u64::MAX {
        vm.rng_next()
    } else {
        let lim = range.wrapping_add(1);
        // rejection threshold: largest multiple of lim that fits
        let t = u64::MAX - u64::MAX % lim;
        loop {
            let r = vm.rng_next();
            if r < t {
                break r % lim;
            }
        }
    };
    let out = Value::Int((lo as u64).wrapping_add(v) as i64);
    Ok(vm.nat_return(fs, &[out]))
}

fn m_randomseed(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (s0, s1) = if nargs == 0 {
        vm.rng_auto_seed()
    } else {
        let x = match check_num(vm, fs, nargs, 0, "randomseed")? {
            Num::Int(i) => i,
            Num::Float(f) => f.to_bits() as i64,
        };
        let y = if nargs >= 2 {
            check_int(vm, fs, nargs, 1, "randomseed")?
        } else {
            0
        };
        (x, y)
    };
    vm.rng_seed(s0 as u64, s1 as u64);
    Ok(vm.nat_return(fs, &[Value::Int(s0), Value::Int(s1)]))
}

// ---- pre-5.3 math entries (kept registered for ≤5.2 — see open_math) ----

fn m_atan2(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let y = check_f64(vm, fs, nargs, 0, "atan2")?;
    let x = check_f64(vm, fs, nargs, 1, "atan2")?;
    Ok(vm.nat_return(fs, &[Value::Float(y.atan2(x))]))
}

fn m_cosh(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = check_f64(vm, fs, nargs, 0, "cosh")?;
    Ok(vm.nat_return(fs, &[Value::Float(x.cosh())]))
}

fn m_sinh(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = check_f64(vm, fs, nargs, 0, "sinh")?;
    Ok(vm.nat_return(fs, &[Value::Float(x.sinh())]))
}

fn m_tanh(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = check_f64(vm, fs, nargs, 0, "tanh")?;
    Ok(vm.nat_return(fs, &[Value::Float(x.tanh())]))
}

fn m_log10(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = check_f64(vm, fs, nargs, 0, "log10")?;
    Ok(vm.nat_return(fs, &[Value::Float(x.log10())]))
}

fn m_pow(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let x = check_f64(vm, fs, nargs, 0, "pow")?;
    let y = check_f64(vm, fs, nargs, 1, "pow")?;
    Ok(vm.nat_return(fs, &[Value::Float(x.powf(y))]))
}
