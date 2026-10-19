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
mod puc_fns;
pub(crate) use puc_fns::PUC_FNS;

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
            Some(n) => Ok(i64::from(
                crate::vm::c_abi::number2integer(ver, n.as_f64()) as i32
            )),
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

/// 5.3's `lua_geti` and `lua_seti` push the index as a key before they
/// reach a metamethod, which then runs above it.
fn key_slot(vm: &Vm) -> u32 {
    u32::from(vm.version() == V::Lua53)
}

/// Element read: raw on ≤5.2 (`lua_rawgeti`), through `__index` on 5.3+
/// (`lua_geti`). `before`: the values the native has pushed by now, which
/// an `__index` runs above and an error counts, written on the stack.
fn tab_geti(vm: &mut Vm, tv: Value, i: i64, before: u32) -> Result<Value, LuaError> {
    if vm.version() <= V::Lua52 {
        // checktab already guaranteed a real table on these dialects.
        return Ok(match tv {
            Value::Table(t) => t.get(Value::Int(i)),
            _ => Value::Nil,
        });
    }
    hold_key(vm, i, before)?;
    let extra = before + key_slot(vm);
    vm.index_value_pushed(tv, Value::Int(i), extra)
}

/// [`tab_geti`] with the values the native has pushed before it, `held`
/// (at most two), written where they go when an `__index` or an error
/// can see them.
fn tab_geti_held(vm: &mut Vm, tv: Value, i: i64, held: &[Value]) -> Result<Value, LuaError> {
    if vm.version() <= V::Lua52 {
        return tab_geti(vm, tv, i, 0);
    }
    let (all, n) = with_key(vm, i, held);
    vm.index_value_holding(tv, Value::Int(i), &all[..n])
}

/// [`tab_seti`] with the values pushed before it, as [`tab_geti_held`].
fn tab_seti_held(vm: &mut Vm, tv: Value, i: i64, v: Value, held: &[Value]) -> Result<(), LuaError> {
    if vm.version() <= V::Lua52 {
        return tab_seti(vm, tv, i, v, 0);
    }
    let (all, n) = with_key(vm, i, held);
    vm.newindex_value_holding(tv, Value::Int(i), v, &all[..n])
}

/// `held`, then the key 5.3's `lua_geti` / `lua_seti` push.
fn with_key(vm: &Vm, i: i64, held: &[Value]) -> ([Value; 3], usize) {
    let mut all = [Value::Nil; 3];
    all[..held.len()].copy_from_slice(held);
    all[held.len()] = Value::Int(i);
    (all, held.len() + key_slot(vm) as usize)
}

/// 5.3's `lua_geti` / `lua_seti` push the key, above the `before` values
/// the native has pushed (and written) by then.
fn hold_key(vm: &mut Vm, i: i64, before: u32) -> Result<(), LuaError> {
    if key_slot(vm) == 0 {
        return Ok(());
    }
    let top = vm.running_natives.last().map_or(0, |a| a.top_off);
    vm.native_hold(top + before as i32, &[Value::Int(i)])
}

/// Element write: raw on ≤5.2 (`lua_rawseti`), through `__newindex` on
/// 5.3+ (`lua_seti`). `before`: the values the native has pushed by now,
/// the value itself among them when a read pushed it (an argument is
/// not pushed again).
fn tab_seti(vm: &mut Vm, tv: Value, i: i64, v: Value, before: u32) -> Result<(), LuaError> {
    if vm.version() <= V::Lua52 {
        if let Value::Table(t) = tv {
            // SAFETY: `t` is the table argument, kept alive by its stack slot; no reference into it is live across the `set`, which does not collect
            match vm.table_op(|vm| unsafe { t.as_mut() }.set(&mut vm.heap, Value::Int(i), v)) {
                Ok(()) => {}
                // lua_rawseti in Redis refuses a read-only table
                Err(e @ (TableError::ReadOnly | TableError::Memory(_))) => {
                    return Err(vm.table_error(e));
                }
                Err(_) => return Err(vm.rt_err("table overflow")),
            }
            vm.heap.barrier_store(t, &[v]);
        }
        return Ok(());
    }
    hold_key(vm, i, before)?;
    let extra = before + key_slot(vm);
    vm.newindex_value_pushed(tv, Value::Int(i), v, extra)
}
