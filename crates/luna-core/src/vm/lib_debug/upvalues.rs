//! Upvalue access: `getupvalue`, `setupvalue`, `upvalueid` and `upvaluejoin`.

use super::check_int;
use crate::runtime::{Gc, LuaClosure, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{Args, check_any, check_function};
use crate::vm::builtins::arg_error;
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

/// 1-based upvalue index → raw `upvals[]` index. 5.1 functions keep their
/// environment outside the upvalues, so luna's `_ENV` cell is skipped there.
fn visible_upvalue_index(vm: &Vm, cl: Gc<LuaClosure>, n: i64) -> Option<usize> {
    if n < 1 {
        return None;
    }
    if vm.version() <= LuaVersion::Lua51 {
        let env = cl.proto.env_upval_idx as usize;
        return (0..cl.proto.upvals.len())
            .filter(|&i| i != env)
            .nth((n - 1) as usize);
    }
    // `load` gives a chunk without upvalues a closure with one cell for
    // `_ENV` anyway; the prototype is what says which upvalues exist
    ((n as usize) <= cl.proto.upvals.len()).then(|| (n - 1) as usize)
}

/// PUC `aux_upvalue`'s name for upvalue `idx` of a Lua closure: `None` when
/// 5.1 has no name for it (a stripped function carries none).
fn upvalue_name(vm: &Vm, cl: Gc<LuaClosure>, idx: usize) -> Option<String> {
    let name = &cl.proto.upvals[idx].name;
    if !name.is_empty() {
        return Some(name.to_string());
    }
    Some(
        match vm.version() {
            LuaVersion::Lua51 => return None,
            LuaVersion::Lua52 => "",
            LuaVersion::Lua53 => "(*no name)",
            _ => "(no name)",
        }
        .to_string(),
    )
}

/// PUC `auxupvalue`: the checked index and function arguments.
fn upvalue_args(vm: &mut Vm, a: Args) -> Result<(Value, i64), LuaError> {
    let n = check_int(vm, a, 1)?;
    Ok((check_function(vm, a, 0)?, n))
}

pub(super) fn d_getupvalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (f, n) = upvalue_args(vm, Args::new(fs, nargs))?;
    let found = match f {
        // 5.1 does not let Lua touch C upvalues
        Value::Native(_) if vm.version() == LuaVersion::Lua51 => None,
        Value::Native(nc) => usize::try_from(n - 1)
            .ok()
            .and_then(|i| nc.upvals.get(i).copied())
            .map(|v| (String::new(), v)),
        Value::Closure(cl) => visible_upvalue_index(vm, cl, n).and_then(|idx| {
            upvalue_name(vm, cl, idx).map(|name| (name, vm.upvalue_value(cl, idx)))
        }),
        _ => unreachable!("checked function"),
    };
    match found {
        Some((name, value)) => {
            let nm = Value::Str(vm.heap.intern(name.as_bytes()));
            Ok(vm.nat_return(fs, &[nm, value]))
        }
        None => Ok(vm.nat_return(fs, &[])),
    }
}

pub(super) fn d_setupvalue(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let value = check_any(vm, a, 2)?;
    let (f, n) = upvalue_args(vm, a)?;
    let name = match f {
        // library and embedder natives trust what they keep in their
        // upvalues (a state table, a function pointer); none of them may
        // be replaced from Lua
        Value::Native(_) => None,
        Value::Closure(cl) => match visible_upvalue_index(vm, cl, n) {
            Some(idx) => {
                let name = upvalue_name(vm, cl, idx);
                if name.is_some() {
                    vm.upvalue_set_value(cl, idx, value);
                }
                name
            }
            None => None,
        },
        _ => unreachable!("checked function"),
    };
    match name {
        Some(name) => {
            let nm = Value::Str(vm.heap.intern(name.as_bytes()));
            Ok(vm.nat_return(fs, &[nm]))
        }
        None => Ok(vm.nat_return(fs, &[])),
    }
}

/// PUC `lua_upvalueid` of upvalue `n` of `f`: the address identifying it,
/// `None` when out of range (a luna native with no upvalues is PUC's light C
/// function).
fn upvalue_id(f: Value, n: i64) -> Option<*const ()> {
    let i = usize::try_from(n - 1).ok()?;
    match f {
        Value::Closure(cl) => cl.upvals().get(i).map(|u| u.as_ptr() as *const ()),
        Value::Native(nc) => nc.upvals.get(i).map(|v| v as *const Value as *const ()),
        _ => unreachable!("checked function"),
    }
}

/// PUC `checkupval(L, argf, argnup, pnup)`. 5.2/5.3 reject an index out of
/// range; 5.4+ does only when joining (`pnup` given).
fn check_upval(
    vm: &mut Vm,
    a: Args,
    argf: u32,
    argnup: u32,
    joining: bool,
) -> Result<(Value, i64, Option<*const ()>), LuaError> {
    let n = check_int(vm, a, argnup)?;
    let f = check_function(vm, a, argf)?;
    let id = upvalue_id(f, n);
    if id.is_none() && (joining || vm.version() <= LuaVersion::Lua53) {
        return Err(arg_error(vm, argnup + 1, "invalid upvalue index"));
    }
    Ok((f, n, id))
}

pub(super) fn d_upvalueid(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (_, _, id) = check_upval(vm, Args::new(fs, nargs), 0, 1, false)?;
    let r = id.map_or(Value::Nil, Value::LightUserdata);
    Ok(vm.nat_return(fs, &[r]))
}

pub(super) fn d_upvaluejoin(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (f1, n1, _) = check_upval(vm, a, 0, 1, true)?;
    let (f2, n2, _) = check_upval(vm, a, 2, 3, true)?;
    let (Value::Closure(f1), Value::Closure(f2)) = (f1, f2) else {
        let argn = if matches!(f1, Value::Native(_)) { 1 } else { 3 };
        return Err(arg_error(vm, argn, "Lua function expected"));
    };
    let uv = f2.upvals()[(n2 - 1) as usize];
    // SAFETY: `f1` is a Lua closure argument kept alive by its stack slot, and `check_upval` bounded `n1` by its upvalue count; `uv` is a separate handle read out before the borrow, which covers one store
    unsafe { f1.as_mut() }.upvals_mut()[(n1 - 1) as usize] = uv;
    // f1's upvalue slice just changed; re-gray it so the collector re-traces
    vm.heap.barrier_back(f1);
    Ok(0)
}
