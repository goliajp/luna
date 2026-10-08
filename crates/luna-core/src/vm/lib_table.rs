//! table library, following each dialect's `ltablib.c` (and 5.1's
//! `luaB_unpack`). ≤5.2 checks for a real table and reads/writes raw with C
//! `int` indices; 5.3+ duck-types the argument (`checktab`) and goes through
//! metamethods with `lua_Integer` indices. `sort` is PUC's quicksort run in
//! place, so the comparator sees the same calls in the same order.

use crate::runtime::{TableError, Value};
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::raise_str;
use crate::vm::error::LuaError;
use crate::vm::exec::{Mm, Vm};

mod legacy;
mod sequence;
mod sort;
use legacy::{t_foreach, t_foreachi, t_getn, t_maxn, t_setn};
pub(crate) use sequence::t_unpack;
use sequence::{t_concat, t_create, t_insert, t_move, t_pack, t_remove};
use sort::t_sort;

type Native = fn(&mut Vm, u32, u32) -> Result<u32, LuaError>;

pub(crate) fn open_table(vm: &mut Vm) {
    let ver = vm.version();
    let t = vm.heap.new_table();
    // Ordering note: the enum runs Lua51 < Lua52 < Lua53 < Lua54 <
    // MacroLua < Lua55, so MacroLua (a 5.4 base) inherits exactly the 5.4
    // surface from these comparisons.
    let mut funcs: Vec<(&str, Native)> = vec![
        ("insert", t_insert),
        ("remove", t_remove),
        ("concat", t_concat),
        ("sort", t_sort),
    ];
    // 5.2+ — on 5.1 `unpack` is a base-library global (builtins).
    if ver >= V::Lua52 {
        funcs.extend([("unpack", t_unpack as Native), ("pack", t_pack)]);
    }
    if ver >= V::Lua53 {
        funcs.push(("move", t_move));
    }
    if ver >= V::Lua55 {
        funcs.push(("create", t_create));
    }
    // LUA_COMPAT_MAXN is on in the default 5.2 build only (5.3's default
    // LUA_COMPAT_5_2 does not include it).
    if ver <= V::Lua52 {
        funcs.push(("maxn", t_maxn));
    }
    // 5.1 keeps `setn` registered purely to raise "'setn' is obsolete".
    if ver == V::Lua51 {
        funcs.extend([
            ("getn", t_getn as Native),
            ("foreach", t_foreach),
            ("foreachi", t_foreachi),
            ("setn", t_setn),
        ]);
    }
    for (name, f) in funcs {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: `t` is the table allocated above, so it is alive; no reference into it is held across this call, and `set` does not collect
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    }
    vm.set_global("table", Value::Table(t))
        .expect("stdlib registration");
    // once-per-table barrier so a post-init `Vm::open_table` call (the embed
    // API can re-open libraries mid-Propagate) demotes `t` back to gray —
    // no-op when phase != Propagate, where t was born current_white.
    vm.barrier_back_table(t);
    // LUA_COMPAT_UNPACK (default in 5.2): `_G.unpack = table.unpack`, the
    // same function value.
    if ver == V::Lua52 {
        let k = Value::Str(vm.heap.intern(b"unpack"));
        let f = t.get(k);
        vm.set_global("unpack", f).expect("stdlib registration");
    }
}

/// PUC `ltablib.c` argument-check flags (5.3+).
const TAB_R: u8 = 1;
const TAB_W: u8 = 2;
const TAB_L: u8 = 4;
const TAB_RW: u8 = TAB_R | TAB_W;

/// ≤5.2 `luaL_checktype(L, i, LUA_TTABLE)`; 5.3+ `checktab`: a non-table
/// passes when its metatable carries every metamethod the caller needs. 5.5
/// exempts strings from `TAB_L` ("strings don't need '__len' to have a
/// length").
fn checktab(vm: &mut Vm, a: Args, i: u32, what: u8) -> Result<Value, LuaError> {
    let v = a.get(vm, i);
    if matches!(v, Value::Table(_)) {
        return Ok(v);
    }
    let ver = vm.version();
    // the metatable and each field tested are pushed, and stay pushed for
    // the error when one is missing
    let mut pushed = 0;
    let mut ok = ver >= V::Lua53 && !a.is_none(i) && vm.metatable_of(v).is_some();
    if ok {
        pushed = 1;
        let len_free = ver >= V::Lua55 && matches!(v, Value::Str(_));
        for (flag, mm) in [(TAB_R, Mm::Index), (TAB_W, Mm::NewIndex), (TAB_L, Mm::Len)] {
            if what & flag == 0 || (flag == TAB_L && len_free) {
                continue;
            }
            pushed += 1;
            if vm.get_mm(v, mm).is_nil() {
                ok = false;
                break;
            }
        }
    }
    if ok {
        Ok(v)
    } else {
        vm.native_push(pushed);
        Err(argcheck::type_error(vm, a, i, "table"))
    }
}

/// The length the library works with: 5.1 `luaL_getn` (the raw border, as
/// an `int`), 5.2 `luaL_len` (`__len`, truncated to an `int`), 5.3+
/// `luaL_len` (`__len`, which must yield an integer).
fn obj_len(vm: &mut Vm, v: Value) -> Result<i64, LuaError> {
    let ver = vm.version();
    if ver == V::Lua51 {
        let n = match v {
            Value::Table(t) => t.len(),
            _ => 0,
        };
        return Ok(i64::from(n as i32));
    }
    let lv = vm.len_value(v)?;
    if let Value::Int(n) = lv
        && ver >= V::Lua53
    {
        return Ok(n);
    }
    let n = argcheck::to_num(vm, lv);
    if ver == V::Lua52 {
        return match n {
            Some(n) => Ok(i64::from(n.as_f64() as i64 as i32)),
            None => {
                // `luaL_len` leaves the length pushed for the error
                vm.native_push(1);
                Err(raise_str(vm, "object length is not a number"))
            }
        };
    }
    let n = match n {
        Some(crate::numeric::Num::Int(i)) => Some(i),
        Some(crate::numeric::Num::Float(f)) => crate::runtime::value::f2i_exact(f),
        None => None,
    };
    n.ok_or_else(|| {
        vm.native_push(1);
        raise_str(vm, "object length is not an integer")
    })
}

/// `aux_getn`: the table check, then the length.
fn aux_getn(vm: &mut Vm, a: Args, what: u8) -> Result<(Value, i64), LuaError> {
    let tv = checktab(vm, a, 0, what | TAB_L)?;
    let n = obj_len(vm, tv)?;
    Ok((tv, n))
}

/// Element read: raw on ≤5.2 (`lua_rawgeti`), through `__index` on 5.3+
/// (`lua_geti`).
fn tab_geti(vm: &mut Vm, tv: Value, i: i64) -> Result<Value, LuaError> {
    if vm.version() <= V::Lua52 {
        // checktab already guaranteed a real table on these dialects.
        return Ok(match tv {
            Value::Table(t) => t.get(Value::Int(i)),
            _ => Value::Nil,
        });
    }
    with_key_pushed(vm, |vm| vm.index_value(tv, Value::Int(i)))
}

/// 5.3's `lua_geti` and `lua_seti` push the index as a key before they reach
/// a metamethod, which then runs above it
fn with_key_pushed<R>(
    vm: &mut Vm,
    f: impl FnOnce(&mut Vm) -> Result<R, LuaError>,
) -> Result<R, LuaError> {
    let key = u32::from(vm.version() == V::Lua53);
    vm.native_push(key);
    let r = f(vm)?;
    vm.native_pop(key);
    Ok(r)
}

/// `lua_geti` / `lua_rawgeti`: [`tab_geti`], the value left pushed.
fn geti_push(vm: &mut Vm, tv: Value, i: i64) -> Result<Value, LuaError> {
    let v = tab_geti(vm, tv, i)?;
    vm.native_push(1);
    Ok(v)
}

/// `lua_seti` / `lua_rawseti`: [`tab_seti`] of the value on top, popped.
fn seti_pop(vm: &mut Vm, tv: Value, i: i64, v: Value) -> Result<(), LuaError> {
    tab_seti(vm, tv, i, v)?;
    vm.native_pop(1);
    Ok(())
}

/// Element write: raw on ≤5.2 (`lua_rawseti`), through `__newindex` on
/// 5.3+ (`lua_seti`).
fn tab_seti(vm: &mut Vm, tv: Value, i: i64, v: Value) -> Result<(), LuaError> {
    if vm.version() <= V::Lua52 {
        if let Value::Table(t) = tv {
            // SAFETY: `t` is the table argument, kept alive by its stack slot; no reference into it is live across the `set`, which does not collect
            match unsafe { t.as_mut() }.set(&mut vm.heap, Value::Int(i), v) {
                Ok(()) => {}
                // lua_rawseti in Redis refuses a read-only table
                Err(e @ TableError::ReadOnly) => return Err(vm.table_error(e)),
                Err(_) => return Err(vm.rt_err("table overflow")),
            }
            vm.barrier_back_table(t);
        }
        return Ok(());
    }
    with_key_pushed(vm, |vm| vm.newindex_value(tv, Value::Int(i), v))
}
