//! Sequence functions: insert, remove, concat, unpack, pack, move and create.

use super::{
    TAB_L, TAB_R, TAB_RW, TAB_W, aux_getn, checktab, obj_len, tab_geti, tab_geti_held, tab_seti,
    tab_seti_held,
};
use crate::runtime::mem::LVec;
use crate::runtime::{Gc, LuaStr, Value};
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

mod concat;
mod pack_move;
pub(super) use concat::t_concat;
pub(crate) use pack_move::*;

pub(super) fn t_insert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    let (tv, n) = aux_getn(vm, a, TAB_RW)?;
    if ver <= V::Lua52 {
        return insert_int(vm, a, tv, n as i32);
    }
    // first empty slot; a __len of maxinteger wraps (5.4's luaL_intop), so
    // the 2-argument form then writes at mininteger.
    let e = n.wrapping_add(1);
    let pos = match nargs {
        2 => e,
        3 => {
            let pos = argcheck::check_integer(vm, a, 1)?;
            let inside = if ver == V::Lua53 {
                1 <= pos && pos <= e
            } else {
                (pos as u64).wrapping_sub(1) < e as u64
            };
            if !inside {
                return Err(arg_error(vm, 2, "position out of bounds"));
            }
            let mut i = e;
            while i > pos {
                let mv = tab_geti(vm, tv, i - 1, 0)?;
                tab_seti_held(vm, tv, i, mv, &[mv])?;
                i -= 1;
            }
            pos
        }
        _ => return Err(raise_str(vm, "wrong number of arguments to 'insert'")),
    };
    // the value is the last argument, not pushed again
    let v = a.get(vm, nargs - 1);
    tab_seti(vm, tv, pos, v, 0)?;
    Ok(0)
}

/// ≤5.2 `tinsert`, in C `int`s. 5.1 has no bounds check: a position past
/// the end grows the array (`e = pos`).
fn insert_int(vm: &mut Vm, a: Args, tv: Value, n: i32) -> Result<u32, LuaError> {
    let mut e = n.wrapping_add(1);
    let pos = match a.n {
        2 => e,
        3 => {
            let pos = argcheck::check_int(vm, a, 1)?;
            if vm.version() == V::Lua51 {
                e = e.max(pos);
            } else if !(1 <= pos && pos <= e) {
                return Err(arg_error(vm, 2, "position out of bounds"));
            }
            let mut i = e;
            while i > pos {
                let mv = tab_geti(vm, tv, i64::from(i) - 1, 0)?;
                tab_seti(vm, tv, i.into(), mv, 1)?;
                i -= 1;
            }
            pos
        }
        _ => return Err(raise_str(vm, "wrong number of arguments to 'insert'")),
    };
    let v = a.get(vm, a.n - 1);
    tab_seti(vm, tv, pos.into(), v, 0)?;
    Ok(0)
}

pub(super) fn t_remove(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    let (tv, size) = aux_getn(vm, a, TAB_RW)?;
    let (mut pos, size) = if ver <= V::Lua52 {
        let size = size as i32;
        let pos = argcheck::opt_int(vm, a, 1, size)?;
        if ver == V::Lua51 {
            // 5.1 quietly returns nothing for a position outside [1, n].
            if !(1 <= pos && pos <= size) {
                return Ok(0);
            }
        } else if pos != size && !(1 <= pos && pos <= size.wrapping_add(1)) {
            // 5.2 and 5.3 blame argument 1 here; 5.4 corrected it to 2.
            return Err(arg_error(vm, 1, "position out of bounds"));
        }
        (i64::from(pos), i64::from(size))
    } else {
        let pos = argcheck::opt_integer(vm, a, 1, size)?;
        if pos != size {
            if ver == V::Lua53 {
                if !(1 <= pos && pos <= size.wrapping_add(1)) {
                    return Err(arg_error(vm, 1, "position out of bounds"));
                }
            } else if (pos as u64).wrapping_sub(1) > size as u64 {
                return Err(arg_error(vm, 2, "position out of bounds"));
            }
        }
        (pos, size)
    };
    // the removed value stays pushed while the rest shift down; each
    // shifted value and the final nil are pushed above it
    let removed = tab_geti(vm, tv, pos, 0)?;
    while pos < size {
        let mv = tab_geti_held(vm, tv, pos + 1, &[removed])?;
        tab_seti_held(vm, tv, pos, mv, &[removed, mv])?;
        pos += 1;
    }
    tab_seti_held(vm, tv, pos, Value::Nil, &[removed, Value::Nil])?;
    vm.nat_ret(fs, &[removed])
}

/// `table.unpack`, and 5.1's global `unpack` (`luaB_unpack`).
pub(crate) fn t_unpack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    // 5.3/5.4 have no table check at all: `luaL_len` and `lua_geti` fail on
    // their own. 5.5 added `aux_getn(TAB_R)`; ≤5.2 hard-checks.
    let (tv, n) = match ver {
        V::Lua53 | V::Lua54 | V::MacroLua => (a.get(vm, 0), None),
        _ => {
            let tv = checktab(vm, a, 0, TAB_R | TAB_L)?;
            if ver >= V::Lua55 {
                let n = obj_len(vm, tv)?;
                (tv, Some(n))
            } else {
                (tv, None)
            }
        }
    };
    let (i, e) = if ver <= V::Lua52 {
        let i = argcheck::opt_int(vm, a, 1, 1)?;
        let e = if a.is_none_or_nil(vm, 2) {
            obj_len(vm, tv)? as i32
        } else {
            argcheck::check_int(vm, a, 2)?
        };
        (i64::from(i), i64::from(e))
    } else {
        let i = argcheck::opt_integer(vm, a, 1, 1)?;
        let e = match n {
            _ if !a.is_none_or_nil(vm, 2) => argcheck::check_integer(vm, a, 2)?,
            Some(n) => n,
            None => obj_len(vm, tv)?,
        };
        (i, e)
    };
    if i > e {
        return Ok(0);
    }
    let count = (e as i128) - (i as i128) + 1;
    let fits = if ver == V::Lua51 {
        // LUAI_MAXCSTACK: a C function may hold 8000 slots, its arguments
        // included. (`n <= 0` there is C int overflow, i.e. too many.)
        count <= i128::from(i32::MAX) && count + i128::from(nargs) <= 8000
    } else {
        // `n >= INT_MAX || !lua_checkstack(L, ++n)` (5.2: INT_MAX - 10)
        let n = count - 1;
        let too_many = if ver == V::Lua52 {
            n > i128::from(i32::MAX) - 10
        } else {
            n >= i128::from(i32::MAX)
        };
        !too_many && i64::try_from(count).is_ok_and(|c| vm.checkstack(fs + 1 + nargs, c))
    };
    if !fits {
        return Err(raise_str(vm, "too many results to unpack"));
    }
    if let Value::Table(t) = tv
        && (ver <= V::Lua52 || vm.get_mm(tv, crate::vm::exec::Mm::Index).is_nil())
    {
        // every read is a raw one: straight into the result slots
        let n = count as usize;
        vm.grow_stack(fs as usize + n)?;
        for (k, slot) in vm.stack[fs as usize..fs as usize + n]
            .iter_mut()
            .enumerate()
        {
            *slot = t.get_int(i + k as i64);
        }
        return Ok(n as u32);
    }
    // the values go to slots above the arguments as they are read, the
    // results so far pushed under each read (a nested `__index` call runs
    // above them, and the table stays rooted in its argument slot), then
    // move down to the results
    let count = count as usize;
    let base = (fs + 1 + nargs) as usize;
    vm.grow_stack(base + count)?;
    let mut slot = base;
    for k in i..e {
        let v = tab_geti(vm, tv, k, (slot - base) as u32)?;
        vm.stack[slot] = v;
        slot += 1;
    }
    let v = tab_geti(vm, tv, e, (slot - base) as u32)?;
    vm.stack[slot] = v;
    vm.stack.copy_within(base..base + count, fs as usize);
    Ok(count as u32)
}
