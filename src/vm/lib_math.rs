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
        unsafe { t.as_mut() }.set(k, fv).expect("valid key");
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
    let consts: [(&str, Value); 4] = [
        ("pi", Value::Float(std::f64::consts::PI)),
        ("huge", Value::Float(f64::INFINITY)),
        ("maxinteger", Value::Int(i64::MAX)),
        ("mininteger", Value::Int(i64::MIN)),
    ];
    for (name, v) in consts {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        unsafe { t.as_mut() }.set(k, v).expect("valid key");
    }
    vm.set_global("math", Value::Table(t));
}

fn check_num(vm: &mut Vm, fs: u32, nargs: u32, i: u32, who: &str) -> Result<Num, LuaError> {
    match vm.nat_arg(fs, nargs, i) {
        Value::Int(x) => Ok(Num::Int(x)),
        Value::Float(x) => Ok(Num::Float(x)),
        Value::Str(s) => crate::numeric::str2num(s.as_bytes(), true, true)
            .ok_or_else(|| arg_error(vm, i + 1, who, "number expected, got string")),
        v => Err(arg_error(
            vm,
            i + 1,
            who,
            &format!("number expected, got {}", v.type_name()),
        )),
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
        _ => (
            check_int(vm, fs, nargs, 0, "random")?,
            check_int(vm, fs, nargs, 1, "random")?,
        ),
    };
    if lo > hi {
        return Err(arg_error(vm, nargs.min(2), "random", "interval is empty"));
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
