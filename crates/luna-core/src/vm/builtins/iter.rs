//! Table traversal: `next`, `pairs` and `ipairs`.

use super::*;

pub(super) fn nat_next(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let k = a.get(vm, 1);
    // 5.3+ tables keep integer keys only as integers, and `luaH_next` looks a
    // float key up without normalizing it: an integral float never matches.
    if let Value::Float(f) = k
        && vm.version() >= LuaVersion::Lua53
        && crate::runtime::value::f2i_exact(f).is_some()
    {
        return Err(vm.plain_err("invalid key to 'next'"));
    }
    match t.next(k) {
        Ok(Some((k, v))) => Ok(vm.nat_return(fs, &[k, v])),
        Ok(None) => Ok(vm.nat_return(fs, &[Value::Nil])),
        Err(_) => Err(vm.plain_err("invalid key to 'next'")),
    }
}

/// `pairs` without a `__pairs` metamethod (the dispatcher in exec.rs calls a
/// present `__pairs` yieldably through `Vm::begin_pairs`, which also covers
/// a `pairs` reached through `call_value` here).
pub(crate) fn nat_pairs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::exec::Mm;
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    // 5.3+ take any value (the iterator fails later if it is no table).
    if ver >= LuaVersion::Lua53 {
        argcheck::check_any(vm, a, 0)?;
    }
    let t = a.get(vm, 0);
    if ver >= LuaVersion::Lua52 {
        let mm = vm.get_mm(t, Mm::Pairs);
        if !mm.is_nil() {
            let n = pairs_mm_results(vm);
            // 5.2/5.3 call `__pairs` with a plain `lua_call` (not yieldable);
            // this path is only reached from C on 5.4+, where yielding is
            // impossible anyway.
            let res = vm.call_value(mm, &[t])?;
            let mut out = [Value::Nil; 4];
            for (slot, v) in out.iter_mut().zip(res) {
                *slot = v;
            }
            return Ok(vm.nat_return(fs, &out[..n]));
        }
    }
    if ver <= LuaVersion::Lua52 {
        argcheck::check_table(vm, a, 0)?;
    }
    let it = vm.nat_upval(fs, 0);
    // 5.5 adds a fourth value, the (nil) to-be-closed variable.
    if ver >= LuaVersion::Lua55 {
        Ok(vm.nat_return(fs, &[it, t, Value::Nil, Value::Nil]))
    } else {
        Ok(vm.nat_return(fs, &[it, t, Value::Nil]))
    }
}

/// How many results `pairs` takes from a `__pairs` metamethod: 5.5 keeps four
/// (the fourth is a to-be-closed value), 5.2–5.4 three.
pub(crate) fn pairs_mm_results(vm: &Vm) -> usize {
    if vm.version() >= LuaVersion::Lua55 {
        4
    } else {
        3
    }
}

/// PUC `ipairsaux` — the iterator behind `ipairs`. Exposed
/// `pub(crate)` so the trace JIT (`Vm::jit_op_tforcall`) can
/// fn-pointer-compare against it for the v3 fast path (skip
/// `begin_call` + `nat_arg` and call `Table::get_int` directly).
#[doc(hidden)]
pub fn ipairs_iter(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let i = match vm.nat_arg(fs, nargs, 1) {
        Value::Int(i) if vm.version() >= LuaVersion::Lua53 => i,
        _ if vm.version() <= LuaVersion::Lua52 => return ipairs_iter_raw(vm, fs, nargs),
        _ => argcheck::check_integer(vm, Args::new(fs, nargs), 1)?,
    };
    let tv = vm.nat_arg(fs, nargs, 0);
    // `luaL_intop(+, i, 1)`: wraps at the top of the integer range.
    let next_i = i.wrapping_add(1);
    // PUC 5.3+ ipairsaux uses lua_geti, honouring __index on any value.
    let v = vm.index_value(tv, Value::Int(next_i))?;
    if v.is_nil() {
        Ok(vm.nat_return(fs, &[Value::Nil]))
    } else {
        Ok(vm.nat_return(fs, &[Value::Int(next_i), v]))
    }
}

/// ≤5.2 `ipairsaux`: the control value is a C int read before the table is
/// checked, elements are read raw, and the end of the sequence is no values
/// on 5.1 but a single nil on 5.2.
#[cold]
fn ipairs_iter_raw(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let i = argcheck::check_int(vm, a, 1)?.wrapping_add(1);
    let t = argcheck::check_table(vm, a, 0)?;
    let v = t.get_int(i as i64);
    if !v.is_nil() {
        Ok(vm.nat_return(fs, &[Value::Int(i as i64), v]))
    } else if vm.version() == LuaVersion::Lua51 {
        Ok(vm.nat_return(fs, &[]))
    } else {
        Ok(vm.nat_return(fs, &[Value::Nil]))
    }
}

pub(super) fn nat_ipairs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    if ver >= LuaVersion::Lua53 {
        argcheck::check_any(vm, a, 0)?;
    }
    let t = a.get(vm, 0);
    // 5.2 honoured `__ipairs(t)`, and so does 5.3's default build
    // (LUA_COMPAT_5_2 → LUA_COMPAT_IPAIRS): the metamethod's first three
    // results replace the iterator triplet. 5.4 dropped it. nextvar.lua 5.2
    // :459 paginates a proxy through it.
    if (ver == LuaVersion::Lua52 || ver == LuaVersion::Lua53)
        && let Some(mt) = vm.metatable_of(t)
    {
        let key = Value::Str(vm.heap.intern(b"__ipairs"));
        let mm = mt.get(key);
        if !mm.is_nil() {
            // `lua_call`: not yieldable.
            let rs = vm.call_value(mm, &[t])?;
            let mut out = [Value::Nil; 3];
            for (slot, v) in out.iter_mut().zip(rs) {
                *slot = v;
            }
            return Ok(vm.nat_return(fs, &out));
        }
    }
    if ver <= LuaVersion::Lua52 {
        argcheck::check_table(vm, a, 0)?;
    }
    let it = vm.nat_upval(fs, 0);
    Ok(vm.nat_return(fs, &[it, t, Value::Int(0)]))
}
