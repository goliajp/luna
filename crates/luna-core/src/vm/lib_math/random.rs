//! `math.random` and `math.randomseed` for each dialect.

use crate::runtime::Value;
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// A float in [0, 1) from the top 53 bits (PUC 5.4 `I2d`).
fn rand_float(vm: &mut Vm) -> f64 {
    (vm.rng_next() >> 11) as f64 * (0.5 / (1u64 << 52) as f64)
}

pub(super) fn m_random(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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

pub(super) fn m_randomseed(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
