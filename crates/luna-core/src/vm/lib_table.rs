//! table library, following each dialect's `ltablib.c` (and 5.1's
//! `luaB_unpack`). ≤5.2 checks for a real table and reads/writes raw with C
//! `int` indices; 5.3+ duck-types the argument (`checktab`) and goes through
//! metamethods with `lua_Integer` indices. `sort` is PUC's quicksort run in
//! place, so the comparator sees the same calls in the same order.

use crate::runtime::{Gc, LuaStr, Value};
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::{Mm, Vm};

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
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    }
    vm.set_global("table", Value::Table(t))
        .expect("stdlib registration");
    // once-per-table barrier so a post-init `Vm::open_table` call (P09 embed
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
    let ok = ver >= V::Lua53
        && !a.is_none(i)
        && vm.metatable_of(v).is_some()
        && (what & TAB_R == 0 || !vm.get_mm(v, Mm::Index).is_nil())
        && (what & TAB_W == 0 || !vm.get_mm(v, Mm::NewIndex).is_nil())
        && (what & TAB_L == 0
            || (ver >= V::Lua55 && matches!(v, Value::Str(_)))
            || !vm.get_mm(v, Mm::Len).is_nil());
    if ok {
        Ok(v)
    } else {
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
            None => Err(raise_str(vm, "object length is not a number")),
        };
    }
    let n = match n {
        Some(crate::numeric::Num::Int(i)) => Some(i),
        Some(crate::numeric::Num::Float(f)) => crate::runtime::value::f2i_exact(f),
        None => None,
    };
    n.ok_or_else(|| raise_str(vm, "object length is not an integer"))
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
    vm.index_value(tv, Value::Int(i))
}

/// Element write: raw on ≤5.2 (`lua_rawseti`), through `__newindex` on
/// 5.3+ (`lua_seti`).
fn tab_seti(vm: &mut Vm, tv: Value, i: i64, v: Value) -> Result<(), LuaError> {
    if vm.version() <= V::Lua52 {
        if let Value::Table(t) = tv {
            // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
            let r = unsafe { t.as_mut() }.set(&mut vm.heap, Value::Int(i), v);
            debug_assert!(r.is_ok(), "integer key is never nil/NaN");
            let _ = r;
            vm.barrier_back_table(t);
        }
        return Ok(());
    }
    vm.newindex_value(tv, Value::Int(i), v)
}

/// 5.1 `table.setn`: obsolete, but still checks its table first.
fn t_setn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    argcheck::check_table(vm, Args::new(fs, nargs), 0)?;
    Err(raise_str(vm, "'setn' is obsolete"))
}

/// 5.1 `table.getn(t)`.
fn t_getn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (_, n) = aux_getn(vm, Args::new(fs, nargs), 0)?;
    Ok(vm.nat_return(fs, &[Value::Int(n)]))
}

/// 5.1 `table.foreach(t, f)`: the first non-nil result of `f(k, v)`, or no
/// result at all.
fn t_foreach(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
fn t_foreachi(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let (tv, n) = aux_getn(vm, a, 0)?;
    let f = argcheck::check_function(vm, a, 1)?;
    for i in 1..=n {
        let v = tab_geti(vm, tv, i)?;
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
fn t_maxn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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

fn t_insert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
                let mv = tab_geti(vm, tv, i - 1)?;
                tab_seti(vm, tv, i, mv)?;
                i -= 1;
            }
            pos
        }
        _ => return Err(raise_str(vm, "wrong number of arguments to 'insert'")),
    };
    let v = a.get(vm, nargs - 1);
    tab_seti(vm, tv, pos, v)?;
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
                let mv = tab_geti(vm, tv, i64::from(i) - 1)?;
                tab_seti(vm, tv, i.into(), mv)?;
                i -= 1;
            }
            pos
        }
        _ => return Err(raise_str(vm, "wrong number of arguments to 'insert'")),
    };
    let v = a.get(vm, a.n - 1);
    tab_seti(vm, tv, pos.into(), v)?;
    Ok(0)
}

fn t_remove(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
    let removed = tab_geti(vm, tv, pos)?;
    while pos < size {
        let mv = tab_geti(vm, tv, pos + 1)?;
        tab_seti(vm, tv, pos, mv)?;
        pos += 1;
    }
    tab_seti(vm, tv, pos, Value::Nil)?;
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

fn t_concat(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
    // PUC appends `[i, last)` each followed by the separator, then `last`
    // on its own, so `last == maxinteger` never overflows the counter.
    let mut k = i;
    while k < last {
        concat_field(vm, tv, k, &mut out)?;
        out.extend_from_slice(sep.bytes());
        k += 1;
    }
    if k == last {
        concat_field(vm, tv, last, &mut out)?;
    }
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

fn concat_field(vm: &mut Vm, tv: Value, k: i64, out: &mut Vec<u8>) -> Result<(), LuaError> {
    match tab_geti(vm, tv, k)? {
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
        // `n >= INT_MAX || !lua_checkstack(L, n)` (5.2: INT_MAX - 10). The
        // stack check is against live room, so a coroutine that already
        // holds values cannot unpack as many (coroutine.lua :530).
        let n = count - 1;
        let too_many = if ver == V::Lua52 {
            n > i128::from(i32::MAX) - 10
        } else {
            n >= i128::from(i32::MAX)
        };
        !too_many && count < i128::from(vm.stack_room())
    };
    if !fits {
        return Err(raise_str(vm, "too many results to unpack"));
    }
    let mut vals: Vec<Value> = Vec::with_capacity(count as usize);
    for k in i..e {
        vals.push(tab_geti(vm, tv, k)?);
    }
    vals.push(tab_geti(vm, tv, e)?);
    Ok(vm.nat_return(fs, &vals))
}

fn t_pack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.heap.new_table();
    {
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        let tm = unsafe { t.as_mut() };
        for i in 0..nargs {
            let v = vm.nat_arg(fs, nargs, i);
            let _ = tm.set_int(&mut vm.heap, i as i64 + 1, v);
        }
    }
    let nk = Value::Str(vm.heap.intern(b"n"));
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }
        .set(&mut vm.heap, nk, Value::Int(nargs as i64))
        .expect("valid key");
    // SETLIST-style once-per-table barrier: t is born BLACK if we're mid-
    // Propagate, and the bulk inserts above are bare `set_int`/`set` that
    // don't barrier. PUC's `lua_seti`/`lua_setfield` in `tpack` do.
    vm.barrier_back_table(t);
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

fn t_move(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
                let v = tab_geti(vm, a1, f + i)?;
                tab_seti(vm, a2, t + i, v)?;
            }
        } else {
            for i in (0..n).rev() {
                let v = tab_geti(vm, a1, f + i)?;
                tab_seti(vm, a2, t + i, v)?;
            }
        }
    }
    Ok(vm.nat_return(fs, &[a2]))
}

fn t_create(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }.ensure_array(&mut vm.heap, n as usize);
    // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
    unsafe { t.as_mut() }.ensure_hash(&mut vm.heap, m as usize);
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

fn t_sort(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let ver = vm.version();
    let (tv, n) = aux_getn(vm, a, TAB_RW)?;
    // 5.3+ checks the size and the comparator only for a non-trivial array.
    if ver >= V::Lua53 && n <= 1 {
        return Ok(0);
    }
    if ver >= V::Lua53 && n >= i64::from(i32::MAX) {
        return Err(arg_error(vm, 1, "array too big"));
    }
    let comp = if a.is_none_or_nil(vm, 1) {
        None
    } else {
        Some(argcheck::check_function(vm, a, 1)?)
    };
    // PUC keeps every element it is holding on the Lua stack; this frame
    // of `sort_scratch` is that stack, traced by `gc_roots`, so a
    // `collectgarbage()` inside the comparator cannot free them.
    let frame = match comp {
        None => pure_snapshot(tv, n),
        Some(_) => None,
    };
    let snapshot = frame.as_ref().map(Vec::len);
    vm.sort_scratch.push(frame.unwrap_or_default());
    let s = Sorter { tv, comp, snapshot };
    let r = if ver <= V::Lua52 {
        s.auxsort_int(vm, 1, n as i32)
    } else {
        s.auxsort(vm, 1, n as u32, 0)
    };
    let frame = vm.sort_scratch.pop().expect("sort frame");
    r?;
    if let Some(len) = snapshot {
        for (i, v) in frame[..len].iter().enumerate() {
            tab_seti(vm, tv, i as i64 + 1, *v)?;
        }
    }
    Ok(0)
}

/// The elements, when sorting them cannot be observed: no comparator,
/// and `t[1..n]` all non-NaN numbers or all strings. Then no comparison
/// calls a metamethod or fails, and no element access reaches `__index`
/// or `__newindex` (every slot is present), so running the same
/// algorithm over a copy and storing the result gives the same table as
/// sorting in place, without a table access per step.
fn pure_snapshot(tv: Value, n: i64) -> Option<Vec<Value>> {
    let Value::Table(t) = tv else {
        return None;
    };
    let mut out = Vec::with_capacity(usize::try_from(n).ok()?);
    let mut strings = None;
    for i in 1..=n {
        let v = t.get(Value::Int(i));
        let is_str = match v {
            Value::Int(_) => false,
            Value::Float(f) if !f.is_nan() => false,
            Value::Str(_) => true,
            _ => return None,
        };
        if *strings.get_or_insert(is_str) != is_str {
            return None;
        }
        out.push(v);
    }
    Some(out)
}

/// PUC's quicksort (`auxsort`), translated with its stack discipline: `geti`
/// pushes, `set2` stores and pops the top two, comparisons address stack
/// slots relative to the top.
struct Sorter {
    tv: Value,
    comp: Option<Value>,
    /// `Some(n)`: the elements were copied into the first `n` slots of the
    /// sort frame (see [`pure_snapshot`]) and are read and written there.
    snapshot: Option<usize>,
}

fn invalid_order(vm: &mut Vm) -> LuaError {
    raise_str(vm, "invalid order function for sorting")
}

impl Sorter {
    fn stack(vm: &mut Vm) -> &mut Vec<Value> {
        vm.sort_scratch.last_mut().expect("sort frame")
    }

    fn at(vm: &mut Vm, rel: usize) -> Value {
        let st = Self::stack(vm);
        st[st.len() - rel]
    }

    fn pop(vm: &mut Vm, n: usize) {
        let st = Self::stack(vm);
        st.truncate(st.len() - n);
    }

    fn push(vm: &mut Vm, v: Value) {
        Self::stack(vm).push(v);
    }

    fn geti(&self, vm: &mut Vm, i: i64) -> Result<(), LuaError> {
        let v = match self.snapshot {
            Some(_) => Self::stack(vm)[(i - 1) as usize],
            None => tab_geti(vm, self.tv, i)?,
        };
        Self::push(vm, v);
        Ok(())
    }

    /// `t[i] = top`, then pop.
    fn seti(&self, vm: &mut Vm, i: i64) -> Result<(), LuaError> {
        let v = Self::at(vm, 1);
        match self.snapshot {
            Some(_) => Self::stack(vm)[(i - 1) as usize] = v,
            None => tab_seti(vm, self.tv, i, v)?,
        }
        Self::pop(vm, 1);
        Ok(())
    }

    /// `set2`: `t[i] = top`, pop, `t[j] = top`, pop.
    fn set2(&self, vm: &mut Vm, i: i64, j: i64) -> Result<(), LuaError> {
        self.seti(vm, i)?;
        self.seti(vm, j)
    }

    /// `sort_comp(L, -a, -b)`: is the value `a` slots down less than the one
    /// `b` slots down?
    fn lt(&self, vm: &mut Vm, a: usize, b: usize) -> Result<bool, LuaError> {
        let x = Self::at(vm, a);
        let y = Self::at(vm, b);
        match self.comp {
            // sort is an unprotected C call: the comparator runs non-yieldable.
            Some(f) => Ok(vm
                .call_noyield(f, &[x, y])?
                .first()
                .is_some_and(|r| r.truthy())),
            None => match (x, y) {
                (Value::Int(a), Value::Int(b)) => Ok(a < b),
                (Value::Float(a), Value::Float(b)) => Ok(a < b),
                _ => vm.less_than(x, y, false),
            },
        }
    }

    /// ≤5.2 `auxsort` on C `int` indices. 5.1 detects a bad comparator only
    /// once the scan has run past the range (`i > u`, `j < l`); 5.2 one
    /// step earlier.
    fn auxsort_int(&self, vm: &mut Vm, mut l: i32, mut u: i32) -> Result<(), LuaError> {
        let strict = vm.version() == V::Lua52;
        while l < u {
            self.geti(vm, l.into())?;
            self.geti(vm, u.into())?;
            if self.lt(vm, 1, 2)? {
                self.set2(vm, l.into(), u.into())?;
            } else {
                Self::pop(vm, 2);
            }
            if u - l == 1 {
                break;
            }
            let mut i = (l + u) / 2;
            self.geti(vm, i.into())?;
            self.geti(vm, l.into())?;
            if self.lt(vm, 2, 1)? {
                self.set2(vm, i.into(), l.into())?;
            } else {
                Self::pop(vm, 1);
                self.geti(vm, u.into())?;
                if self.lt(vm, 1, 2)? {
                    self.set2(vm, i.into(), u.into())?;
                } else {
                    Self::pop(vm, 2);
                }
            }
            if u - l == 2 {
                break;
            }
            self.geti(vm, i.into())?;
            let pivot = Self::at(vm, 1);
            Self::push(vm, pivot);
            self.geti(vm, (u - 1).into())?;
            self.set2(vm, i.into(), (u - 1).into())?;
            i = l;
            let mut j = u - 1;
            loop {
                i += 1;
                self.geti(vm, i.into())?;
                while self.lt(vm, 1, 2)? {
                    if if strict { i >= u } else { i > u } {
                        return Err(invalid_order(vm));
                    }
                    Self::pop(vm, 1);
                    i += 1;
                    self.geti(vm, i.into())?;
                }
                j -= 1;
                self.geti(vm, j.into())?;
                while self.lt(vm, 3, 1)? {
                    if if strict { j <= l } else { j < l } {
                        return Err(invalid_order(vm));
                    }
                    Self::pop(vm, 1);
                    j -= 1;
                    self.geti(vm, j.into())?;
                }
                if j < i {
                    Self::pop(vm, 3);
                    break;
                }
                self.set2(vm, i.into(), j.into())?;
            }
            self.geti(vm, (u - 1).into())?;
            self.geti(vm, i.into())?;
            self.set2(vm, (u - 1).into(), i.into())?;
            // recurse into the smaller half [j..i], loop on the larger [l..u]
            if i - l < u - i {
                j = l;
                i -= 1;
                l = i + 2;
            } else {
                j = i + 1;
                i = u;
                u = j - 2;
            }
            self.auxsort_int(vm, j, i)?;
        }
        Ok(())
    }

    /// 5.3+ `partition`: pivot P on top of the stack, a[lo] <= P == a[up-1]
    /// <= a[up].
    fn partition(&self, vm: &mut Vm, lo: u32, up: u32) -> Result<u32, LuaError> {
        let mut i = lo;
        let mut j = up - 1;
        loop {
            i += 1;
            self.geti(vm, i.into())?;
            while self.lt(vm, 1, 2)? {
                if i == up - 1 {
                    return Err(invalid_order(vm));
                }
                Self::pop(vm, 1);
                i += 1;
                self.geti(vm, i.into())?;
            }
            j -= 1;
            self.geti(vm, j.into())?;
            while self.lt(vm, 3, 1)? {
                if j < i {
                    return Err(invalid_order(vm));
                }
                Self::pop(vm, 1);
                j -= 1;
                self.geti(vm, j.into())?;
            }
            if j < i {
                Self::pop(vm, 1);
                self.set2(vm, (up - 1).into(), i.into())?;
                return Ok(i);
            }
            self.set2(vm, i.into(), j.into())?;
        }
    }

    /// 5.3+ `auxsort` on `unsigned int` indices, with PUC's randomized pivot
    /// once a partition comes out badly unbalanced.
    fn auxsort(&self, vm: &mut Vm, mut lo: u32, mut up: u32, mut rnd: u32) -> Result<(), LuaError> {
        while lo < up {
            self.geti(vm, lo.into())?;
            self.geti(vm, up.into())?;
            if self.lt(vm, 1, 2)? {
                self.set2(vm, lo.into(), up.into())?;
            } else {
                Self::pop(vm, 2);
            }
            if up - lo == 1 {
                return Ok(());
            }
            let mut p = if up - lo < 100 || rnd == 0 {
                (lo + up) / 2
            } else {
                let r4 = (up - lo) / 4;
                let r = if vm.version() == V::Lua53 {
                    rnd
                } else {
                    rnd ^ lo ^ up
                };
                r % (r4 * 2) + (lo + r4)
            };
            self.geti(vm, p.into())?;
            self.geti(vm, lo.into())?;
            if self.lt(vm, 2, 1)? {
                self.set2(vm, p.into(), lo.into())?;
            } else {
                Self::pop(vm, 1);
                self.geti(vm, up.into())?;
                if self.lt(vm, 1, 2)? {
                    self.set2(vm, p.into(), up.into())?;
                } else {
                    Self::pop(vm, 2);
                }
            }
            if up - lo == 2 {
                return Ok(());
            }
            self.geti(vm, p.into())?;
            let pivot = Self::at(vm, 1);
            Self::push(vm, pivot);
            self.geti(vm, (up - 1).into())?;
            self.set2(vm, p.into(), (up - 1).into())?;
            p = self.partition(vm, lo, up)?;
            let n;
            if p - lo < up - p {
                self.auxsort(vm, lo, p - 1, rnd)?;
                n = p - lo;
                lo = p + 1;
            } else {
                self.auxsort(vm, p + 1, up, rnd)?;
                n = up - p;
                up = p - 1;
            }
            if up.wrapping_sub(lo) / 128 > n {
                rnd = randomize_pivot();
            }
        }
        Ok(())
    }
}

/// PUC `l_randomizePivot`: any cheap varying value (5.3/5.4 mix `clock()`
/// and `time()`).
fn randomize_pivot() -> u32 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() ^ d.as_secs() as u32)
}
