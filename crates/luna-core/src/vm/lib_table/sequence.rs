//! Sequence functions: insert, remove, concat, unpack, pack, move and create.

use super::{TAB_R, TAB_RW, TAB_W, aux_getn, checktab, geti_push, obj_len, seti_pop};
use crate::runtime::{Gc, LuaStr, Value};
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

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
                let mv = geti_push(vm, tv, i - 1)?;
                seti_pop(vm, tv, i, mv)?;
                i -= 1;
            }
            pos
        }
        _ => return Err(raise_str(vm, "wrong number of arguments to 'insert'")),
    };
    let v = a.get(vm, nargs - 1);
    seti_pop(vm, tv, pos, v)?;
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
                let mv = geti_push(vm, tv, i64::from(i) - 1)?;
                seti_pop(vm, tv, i.into(), mv)?;
                i -= 1;
            }
            pos
        }
        _ => return Err(raise_str(vm, "wrong number of arguments to 'insert'")),
    };
    let v = a.get(vm, a.n - 1);
    seti_pop(vm, tv, pos.into(), v)?;
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
    let removed = geti_push(vm, tv, pos)?;
    while pos < size {
        let mv = geti_push(vm, tv, pos + 1)?;
        seti_pop(vm, tv, pos, mv)?;
        pos += 1;
    }
    vm.native_push(1);
    seti_pop(vm, tv, pos, Value::Nil)?;
    Ok(vm.nat_return(fs, &[removed]))
}

/// The separator `concat` appends: the argument string itself (rooted by
/// the call frame) or a number's rendering.
enum Sep {
    Str(Gc<LuaStr>),
    Bytes(Vec<u8>),
}

impl Sep {
    fn bytes(&self) -> &[u8] {
        match self {
            Sep::Str(s) => s.as_bytes(),
            Sep::Bytes(b) => b,
        }
    }
}

/// `luaL_optlstring(L, 2, "")`.
fn concat_sep(vm: &mut Vm, a: Args) -> Result<Sep, LuaError> {
    Ok(match a.get(vm, 1) {
        Value::Str(s) => Sep::Str(s),
        _ if a.is_none_or_nil(vm, 1) => Sep::Bytes(Vec::new()),
        v => match argcheck::to_str_bytes(vm, v) {
            Some(b) => Sep::Bytes(b),
            None => return Err(argcheck::type_error(vm, a, 1, "string")),
        },
    })
}

pub(super) fn t_concat(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (tv, sep, i, last) = if vm.version() <= V::Lua52 {
        // ≤5.2 reads the separator before checking the table, and takes
        // the length only when no end index is given.
        let sep = concat_sep(vm, a)?;
        let tv = checktab(vm, a, 0, TAB_R)?;
        let i = argcheck::opt_int(vm, a, 2, 1)?;
        let last = if a.is_none_or_nil(vm, 3) {
            obj_len(vm, tv)? as i32
        } else {
            argcheck::check_int(vm, a, 3)?
        };
        (tv, sep, i64::from(i), i64::from(last))
    } else {
        let (tv, n) = aux_getn(vm, a, TAB_R)?;
        let sep = concat_sep(vm, a)?;
        let i = argcheck::opt_integer(vm, a, 2, 1)?;
        let last = argcheck::opt_integer(vm, a, 3, n)?;
        (tv, sep, i, last)
    };
    let mut out: Vec<u8> = Vec::new();
    let mut slotted = vm.native_buffinit(0);
    // PUC appends `[i, last)` each followed by the separator, then `last`
    // on its own, so `last == maxinteger` never overflows the counter.
    let mut k = i;
    while k < last {
        concat_field(vm, tv, k, &mut out)?;
        out.extend_from_slice(sep.bytes());
        vm.native_buffgrown(&mut slotted, out.len());
        k += 1;
    }
    if k == last {
        concat_field(vm, tv, last, &mut out)?;
    }
    let s = vm.built_str(&out)?;
    Ok(vm.nat_return(fs, &[s]))
}

fn concat_field(vm: &mut Vm, tv: Value, k: i64, out: &mut Vec<u8>) -> Result<(), LuaError> {
    // pushed until it is added
    match geti_push(vm, tv, k)? {
        Value::Str(s) => out.extend_from_slice(s.as_bytes()),
        Value::Int(x) => {
            let mut buf = [0u8; 20];
            out.extend_from_slice(crate::numeric::write_i64_dec(x, &mut buf))
        }
        v @ Value::Float(_) => {
            out.extend_from_slice(&argcheck::to_str_bytes(vm, v).expect("a number converts"))
        }
        // `luaL_typename`: the base type name, never `__name`.
        v => {
            let msg = format!(
                "invalid value ({}) at index {k} in table for 'concat'",
                v.type_name()
            );
            return Err(raise_str(vm, &msg));
        }
    }
    vm.native_pop(1);
    Ok(())
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
            let tv = checktab(vm, a, 0, TAB_R)?;
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
    let mut vals: Vec<Value> = Vec::with_capacity(count as usize);
    for k in i..e {
        vals.push(geti_push(vm, tv, k)?);
    }
    vals.push(geti_push(vm, tv, e)?);
    Ok(vm.nat_return(fs, &vals))
}

pub(super) fn t_pack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.heap.new_table();
    {
        // SAFETY: `t` was allocated above and is held only by this local;
        // `tm` is the only reference into it, and the heap calls made while
        // it lives (`set_int`, `intern`, `set`) do not collect
        let tm = unsafe { t.as_mut() };
        // `lua_createtable(L, n, 1)`: the arguments, nil ones included, fill
        // an array part of exactly their count
        tm.resize(&mut vm.heap, nargs as usize, 1);
        for i in 0..nargs {
            tm.set_list_slot(i as usize, vm.nat_arg(fs, nargs, i));
        }
        let nk = Value::Str(vm.heap.intern(b"n"));
        tm.set(&mut vm.heap, nk, Value::Int(nargs as i64))
            .expect("valid key");
    }
    // SETLIST-style once-per-table barrier: t is born BLACK if we're mid-
    // Propagate, and the bulk inserts above are bare `set_int`/`set` that
    // don't barrier. PUC's `lua_seti`/`lua_setfield` in `tpack` do.
    vm.barrier_back_table(t);
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

pub(super) fn t_move(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let f = argcheck::check_integer(vm, a, 1)?;
    let e = argcheck::check_integer(vm, a, 2)?;
    let t = argcheck::check_integer(vm, a, 3)?;
    let tt = if a.is_none_or_nil(vm, 4) { 0 } else { 4 };
    let a1 = checktab(vm, a, 0, TAB_R)?;
    let a2 = checktab(vm, a, tt, TAB_W)?;
    if e >= f {
        if !(f > 0 || e < i64::MAX.wrapping_add(f)) {
            return Err(arg_error(vm, 3, "too many elements to move"));
        }
        let n = e - f + 1;
        if t > i64::MAX - n + 1 {
            return Err(arg_error(vm, 4, "destination wrap around"));
        }
        // Forward unless the destination overlaps the source in the same
        // table; "same" is `lua_compare(EQ)`, so `__eq` takes part.
        if t > e || t <= f || (tt != 0 && !vm.equal(a1, a2)?) {
            for i in 0..n {
                let v = geti_push(vm, a1, f + i)?;
                seti_pop(vm, a2, t + i, v)?;
            }
        } else {
            for i in (0..n).rev() {
                let v = geti_push(vm, a1, f + i)?;
                seti_pop(vm, a2, t + i, v)?;
            }
        }
    }
    Ok(vm.nat_return(fs, &[a2]))
}

pub(super) fn t_create(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let n = argcheck::check_integer(vm, a, 0)? as u64;
    let m = argcheck::opt_integer(vm, a, 1, 0)? as u64;
    if n > i32::MAX as u64 {
        return Err(arg_error(vm, 1, "out of range"));
    }
    if m > i32::MAX as u64 {
        return Err(arg_error(vm, 2, "out of range"));
    }
    // PUC MAXHBITS: a hash part needs ceillog2(m) <= 30 bits; beyond 2^30
    // slots the resize raises "table overflow" rather than attempting it.
    // That is a `luaG_runerror` from inside a C function: no position.
    if m > (1 << 30) {
        return Err(vm.plain_err("table overflow"));
    }
    let t = vm.heap.new_table();
    // `ensure_array` / `ensure_hash` credit the box-size delta straight to
    // `Heap.bytes` via `apply_bytes_delta`; `free_obj` later subtracts
    // `Table::internal_bytes()` so the round-trip is symmetric. 5.5
    // sort.lua:22 pins this round-trip (`memdiff > N * 4` after
    // `table.create(N)`).
    // SAFETY: `t` was allocated above and is held only by this local; `tm`
    // is the only reference into it, and the two calls do not collect
    let tm = unsafe { t.as_mut() };
    tm.ensure_array(&mut vm.heap, n as usize);
    tm.ensure_hash(&mut vm.heap, m as usize);
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}
