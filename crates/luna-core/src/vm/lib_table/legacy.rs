//! Functions only the older dialects register: 5.1's setn, getn, foreach and
//! foreachi, and maxn through 5.2.

use super::{aux_getn, tab_geti};
use crate::runtime::Value;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::raise_str;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// 5.1 `table.setn`: obsolete, but still checks its table first.
pub(super) fn t_setn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    argcheck::check_table(vm, Args::new(fs, nargs), 0)?;
    Err(raise_str(vm, "'setn' is obsolete"))
}

/// 5.1 `table.getn(t)`.
pub(super) fn t_getn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (_, n) = aux_getn(vm, Args::new(fs, nargs), 0)?;
    Ok(vm.nat_return(fs, &[Value::Int(n)]))
}

/// 5.1 `table.foreach(t, f)`: the first non-nil result of `f(k, v)`, or no
/// result at all.
pub(super) fn t_foreach(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let f = argcheck::check_function(vm, a, 1)?;
    let mut key = Value::Nil;
    while let Some((k, v)) = t
        .next(key)
        .map_err(|_| vm.plain_err("invalid key to 'next'"))?
    {
        let r = vm.call_value(f, &[k, v])?.first().copied();
        if let Some(r) = r
            && !r.is_nil()
        {
            return Ok(vm.nat_return(fs, &[r]));
        }
        key = k;
    }
    Ok(0)
}

/// 5.1 `table.foreachi(t, f)`: `f(i, t[i])` for 1..n, stopping at the first
/// non-nil result, which is returned (otherwise nothing is).
pub(super) fn t_foreachi(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (tv, n) = aux_getn(vm, a, 0)?;
    let f = argcheck::check_function(vm, a, 1)?;
    for i in 1..=n {
        let v = tab_geti(vm, tv, i, 0)?;
        let r = vm.call_value(f, &[Value::Int(i), v])?.first().copied();
        if let Some(r) = r
            && !r.is_nil()
        {
            return Ok(vm.nat_return(fs, &[r]));
        }
    }
    Ok(0)
}

/// 5.1/5.2 `table.maxn(t)`: the largest positive numeric key, as a float.
pub(super) fn t_maxn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = argcheck::check_table(vm, Args::new(fs, nargs), 0)?;
    let mut max: f64 = 0.0;
    let mut key = Value::Nil;
    while let Some((k, _)) = t
        .next(key)
        .map_err(|_| vm.plain_err("invalid key to 'next'"))?
    {
        let n = match k {
            Value::Int(i) => i as f64,
            Value::Float(f) => f,
            _ => f64::NAN,
        };
        if n > max {
            max = n;
        }
        key = k;
    }
    Ok(vm.nat_return(fs, &[Value::Float(max)]))
}
