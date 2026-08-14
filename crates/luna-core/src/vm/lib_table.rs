//! table library. sort is a PUC auxsort-shaped quicksort with Lua
//! comparators (std's sort would panic on non-total orders).

use crate::runtime::Value;
use crate::vm::builtins::{arg_error, check_table, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_table(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    };
    // v2.18: the table library's *surface* is dialect-specific, and luna
    // used to register the union of every dialect's functions. Measured
    // against stock PUC (`for n in pairs(table)`), that exposed
    // `create`/`move`/`pack`/`unpack` on 5.1, `create`/`move` on 5.2 and
    // `create` on 5.3/5.4, while omitting 5.1's `setn` and 5.2's `maxn`.
    // An embedder writing 5.1-targeted Lua would silently pick up 5.5 API.
    //
    // Ordering note: the enum runs Lua51 < Lua52 < Lua53 < Lua54 <
    // MacroLua < Lua55, so MacroLua (a 5.4 base) inherits exactly the 5.4
    // surface from these comparisons — pack/unpack/move but no create.
    use crate::version::LuaVersion as V;
    let ver = vm.version();

    // Present in every dialect.
    set(vm, "insert", t_insert);
    set(vm, "remove", t_remove);
    set(vm, "concat", t_concat);
    set(vm, "sort", t_sort);

    // 5.2+ — `pack`/`unpack` live in the table library. On 5.1 `unpack` is
    // a global only (registered in builtins), and `table.unpack` does not
    // exist.
    if ver >= V::Lua52 {
        set(vm, "unpack", t_unpack);
        set(vm, "pack", t_pack);
    }
    // 5.3+ — `table.move`.
    if ver >= V::Lua53 {
        set(vm, "move", t_move);
    }
    // 5.5+ — `table.create`.
    if ver >= V::Lua55 {
        set(vm, "create", t_create);
    }
    // 5.1 and 5.2 — `table.maxn`, dropped in 5.3.
    if ver <= V::Lua52 {
        set(vm, "maxn", t_maxn);
    }
    // 5.1 only — `getn` (length, replaced by `#`), `foreach`/`foreachi`
    // (replaced by `pairs`/`ipairs`), and `setn`, which PUC 5.1 keeps
    // registered purely to raise "'setn' is obsolete".
    if ver == V::Lua51 {
        set(vm, "getn", t_getn);
        set(vm, "foreach", t_foreach);
        set(vm, "foreachi", t_foreachi);
        set(vm, "setn", t_setn);
    }
    vm.set_global("table", Value::Table(t))
        .expect("stdlib registration");
    // once-per-table barrier so a post-init `Vm::open_table` call (P09 embed
    // API can re-open libraries mid-Propagate) demotes `t` back to gray —
    // no-op when phase != Propagate, where t was born current_white.
    vm.barrier_back_table(t);
    // the global unpack alias exists in 5.1 mode only (P08)
}

// ── Dialect-aware table-argument handling (v2.18) ──────────────────────
//
// PUC's table library changed shape twice, and luna implemented neither
// gradient — it used a hard type check (like <=5.2) together with
// metamethod-aware element access (like 5.3+), so it matched no dialect
// exactly.
//
// Measured against stock binaries:
//
//   5.1  `luaL_checktype` + `lua_rawgeti`/`lua_rawseti`  (hard, raw)
//   5.2  same                                            (hard, raw)
//   5.3+ `checktab` + `lua_geti`/`lua_seti`               (duck-typed,
//                                                          metamethod)
//
// The visible consequence, concatenating a proxy table whose contents
// live behind `__index`: 5.1 gives "", 5.2 raises "invalid value (nil)
// at index 1", 5.3+ reads through and gives "3,1,2".

/// PUC `ltablib.c` argument-check flags (5.3+).
const TAB_R: u8 = 1;
const TAB_W: u8 = 2;
const TAB_L: u8 = 4;
const TAB_RW: u8 = TAB_R | TAB_W;

/// PUC `checktab`: the argument must be a table, or behave like one.
///
/// On <=5.2 there is no such notion — `luaL_checktype` demands a real
/// table — so the flags are ignored there and any non-table is rejected.
///
/// On 5.3+ a non-table passes when its metatable carries every metamethod
/// the caller needs. 5.5 additionally exempts strings from the `TAB_L`
/// requirement ("strings don't need '__len' to have a length"), which is
/// why `table.concat("abc")` reaches its element loop on 5.5 but is
/// rejected outright on 5.3/5.4.
fn checktab(vm: &mut Vm, v: Value, what: u8, argn: u32, who: &str) -> Result<(), LuaError> {
    if matches!(v, Value::Table(_)) {
        return Ok(());
    }
    let reject = |vm: &mut Vm| {
        let got = vm.obj_typename(v);
        Err(arg_error(
            vm,
            argn,
            who,
            &format!("table expected, got {got}"),
        ))
    };
    if vm.version() <= crate::version::LuaVersion::Lua52 {
        return reject(vm);
    }
    if vm.metatable_of(v).is_none() {
        return reject(vm);
    }
    let strings_skip_len =
        vm.version() >= crate::version::LuaVersion::Lua55 && matches!(v, Value::Str(_));
    let ok = (what & TAB_R == 0 || !vm.get_mm(v, crate::vm::exec::Mm::Index).is_nil())
        && (what & TAB_W == 0 || !vm.get_mm(v, crate::vm::exec::Mm::NewIndex).is_nil())
        && (what & TAB_L == 0
            || strings_skip_len
            || !vm.get_mm(v, crate::vm::exec::Mm::Len).is_nil());
    if ok { Ok(()) } else { reject(vm) }
}

/// Element read: raw on <=5.2 (`lua_rawgeti`), through `__index` on 5.3+
/// (`lua_geti`).
fn tab_geti(vm: &mut Vm, tv: Value, i: i64) -> Result<Value, LuaError> {
    if vm.version() <= crate::version::LuaVersion::Lua52 {
        // checktab already guaranteed a real table on these dialects.
        return Ok(match tv {
            Value::Table(t) => t.get(Value::Int(i)),
            _ => Value::Nil,
        });
    }
    vm.index_value(tv, Value::Int(i))
}

/// Element write: raw on <=5.2 (`lua_rawseti`), through `__newindex` on
/// 5.3+ (`lua_seti`).
fn tab_seti(vm: &mut Vm, tv: Value, i: i64, v: Value) -> Result<(), LuaError> {
    if vm.version() <= crate::version::LuaVersion::Lua52 {
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

/// 5.1 `table.setn(t, n)` — PUC 5.1 keeps this registered solely to raise
/// "'setn' is obsolete" (ltablib.c `setn`); the manual-length mechanism it
/// belonged to was removed in 5.1 itself. Registered on 5.1 only so the
/// dialect's table surface matches stock PUC.
fn t_setn(vm: &mut Vm, _fs: u32, _nargs: u32) -> Result<u32, LuaError> {
    Err(raise_str(vm, "'setn' is obsolete"))
}

/// 5.1 `table.getn(t)` — synonymous with `#t` (luna's `checked_len`).
fn t_getn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    check_table(vm, tv, "getn")?;
    let n = vm.checked_len(tv)?;
    Ok(vm.nat_return(fs, &[Value::Int(n)]))
}

/// 5.1 `table.foreach(t, f)` — iterate every key/value via `next`, returning
/// the first non-nil result the callback produces. Replaced by `pairs` in 5.2+.
fn t_foreach(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, tv, "foreach")?;
    let f = vm.nat_arg(fs, nargs, 1);
    let mut key = Value::Nil;
    loop {
        match t
            .next(key)
            .map_err(|_| vm.plain_err("invalid key to 'next'"))?
        {
            Some((k, v)) => {
                let rs = vm.call_value(f, &[k, v])?;
                if let Some(r) = rs.first().copied()
                    && !r.is_nil()
                {
                    return Ok(vm.nat_return(fs, &[r]));
                }
                key = k;
            }
            None => return Ok(vm.nat_return(fs, &[Value::Nil])),
        }
    }
}

/// 5.1 `table.foreachi(t, f)` — iterate integer keys 1..n and call `f(i, t[i])`,
/// short-circuit on the first non-nil return. Replaced by `ipairs` in 5.2+.
fn t_foreachi(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    check_table(vm, tv, "foreachi")?;
    let f = vm.nat_arg(fs, nargs, 1);
    let n = vm.checked_len(tv)?;
    for i in 1..=n {
        let v = tab_geti(vm, tv, i)?;
        let rs = vm.call_value(f, &[Value::Int(i), v])?;
        if let Some(r) = rs.first().copied()
            && !r.is_nil()
        {
            return Ok(vm.nat_return(fs, &[r]));
        }
    }
    Ok(vm.nat_return(fs, &[Value::Nil]))
}

/// 5.1 `table.maxn(t)` — the largest positive numeric key. PUC dropped it in
/// 5.2; reinstate for the 5.1 suite (closure.lua: tail-recursion test uses it).
fn t_maxn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, tv, "maxn")?;
    let mut max: f64 = 0.0;
    let mut key = Value::Nil;
    loop {
        let entry = t
            .next(key)
            .map_err(|_| vm.plain_err("invalid key to 'next'"))?;
        match entry {
            Some((k, _)) => {
                if let Some(n) = match k {
                    Value::Int(i) => Some(i as f64),
                    Value::Float(f) => Some(f),
                    _ => None,
                } && n > max
                {
                    max = n;
                }
                key = k;
            }
            None => break,
        }
    }
    Ok(vm.nat_return(fs, &[Value::Float(max)]))
}

fn t_insert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    checktab(vm, tv, TAB_RW | TAB_L, 1, "insert")?;
    let n = vm.checked_len(tv)?;
    // first empty slot; PUC lets this overflow (a __len of maxinteger inserts
    // at mininteger), so the shift loop must not run for the 2-argument form.
    let e = n.wrapping_add(1);
    // PUC tinsert uses lua_geti/lua_seti, honouring __index/__newindex.
    let (pos, v) = match nargs {
        2 => (e, vm.nat_arg(fs, nargs, 1)),
        3 => {
            let pos = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as a position")?;
            // PUC: (unsigned)pos - 1 < (unsigned)e  (rejects 0 and > e)
            if (pos as u64).wrapping_sub(1) >= e as u64 {
                return Err(arg_error(vm, 2, "insert", "position out of bounds"));
            }
            let mut i = e;
            while i > pos {
                let mv = tab_geti(vm, tv, i - 1)?;
                tab_seti(vm, tv, i, mv)?;
                i -= 1;
            }
            (pos, vm.nat_arg(fs, nargs, 2))
        }
        _ => return Err(raise_str(vm, "wrong number of arguments to 'insert'")),
    };
    tab_seti(vm, tv, pos, v)?;
    Ok(0)
}

fn t_remove(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    checktab(vm, tv, TAB_RW | TAB_L, 1, "remove")?;
    let n = vm.checked_len(tv)?;
    let pos = if nargs >= 2 {
        let pos = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as a position")?;
        if n > 0 && (pos < 1 || pos > n + 1) {
            return Err(arg_error(vm, 2, "remove", "position out of bounds"));
        }
        pos
    } else {
        n
    };
    // PUC tremove uses lua_geti/lua_seti, honouring __index/__newindex.
    let removed = tab_geti(vm, tv, pos)?;
    if pos <= n {
        let mut i = pos;
        while i < n {
            let mv = tab_geti(vm, tv, i + 1)?;
            tab_seti(vm, tv, i, mv)?;
            i += 1;
        }
        tab_seti(vm, tv, n, Value::Nil)?;
    }
    Ok(vm.nat_return(fs, &[removed]))
}

fn t_concat(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    checktab(vm, tv, TAB_R | TAB_L, 1, "concat")?;
    // PUC ≤5.2 has no integer subtype; `tostring(2.0)` is `"2"`, not
    // `"2.0"`. Pass through to numeric formatter so concat's element /
    // separator rendering matches `tostring` / `print` (caught by
    // tests/e2e_programs.rs::e2e_5_1/5_2 table_index_sort divergence).
    let float_fmt = vm.float_fmt();
    let sep: Vec<u8> = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => Vec::new(),
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Int(i) => crate::numeric::num_to_string(crate::numeric::Num::Int(i)).into_bytes(),
        Value::Float(f) => {
            crate::numeric::num_to_string_for(crate::numeric::Num::Float(f), float_fmt).into_bytes()
        }
        v => {
            return Err(arg_error(
                vm,
                2,
                "concat",
                &format!("string expected, got {}", v.type_name()),
            ));
        }
    };
    let i = if nargs >= 3 {
        vm.int_from(vm.nat_arg(fs, nargs, 2), "use as an index")?
    } else {
        1
    };
    let j = if nargs >= 4 {
        vm.int_from(vm.nat_arg(fs, nargs, 3), "use as an index")?
    } else {
        vm.checked_len(tv)?
    };
    let mut out: Vec<u8> = Vec::new();
    // PUC structure: append `[i, j)` each followed by a separator, then the
    // last element on its own. Splitting the final step avoids the `k += 1`
    // overflow when `j == i64::MAX` (e.g. concat at index `maxi`).
    let mut k = i;
    while k < j {
        concat_field(vm, tv, k, &mut out, float_fmt)?;
        out.extend_from_slice(&sep);
        k += 1;
    }
    if i <= j {
        concat_field(vm, tv, j, &mut out, float_fmt)?;
    }
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

fn concat_field(
    vm: &mut Vm,
    tv: Value,
    k: i64,
    out: &mut Vec<u8>,
    float_fmt: crate::numeric::FloatFmt,
) -> Result<(), LuaError> {
    match tab_geti(vm, tv, k)? {
        Value::Str(s) => out.extend_from_slice(s.as_bytes()),
        Value::Int(x) => {
            let mut buf = [0u8; 20];
            out.extend_from_slice(crate::numeric::write_i64_dec(x, &mut buf))
        }
        Value::Float(x) => out.extend_from_slice(
            crate::numeric::num_to_string_for(crate::numeric::Num::Float(x), float_fmt).as_bytes(),
        ),
        v => {
            // PUC ltablib tconcat: "invalid value (%s) at index %I in
            // table for 'concat'" — type name inside the parens
            // (v2.14 fixture 5.5/341).
            let tn = vm.obj_typename(v);
            return Err(raise_str(
                vm,
                &format!("invalid value ({tn}) at index {k} in table for 'concat'"),
            ));
        }
    }
    Ok(())
}

const MAX_UNPACK: i64 = 1_000_000;

pub(crate) fn t_unpack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let tv = vm.nat_arg(fs, nargs, 0);
    // PUC only gave tunpack a type check in 5.5 (`aux_getn(L, 1, TAB_R)`).
    // 5.3/5.4 call `luaL_len` straight away, so `table.unpack("abc")` yields
    // three nils rather than a type error, and `table.unpack(42)` raises
    // "attempt to get length of a number value". <=5.2 hard-checks, which is
    // what checktab does on those dialects.
    if vm.version() >= crate::version::LuaVersion::Lua55
        || vm.version() <= crate::version::LuaVersion::Lua52
    {
        checktab(vm, tv, TAB_R | TAB_L, 1, "unpack")?;
    }
    let i = if nargs >= 2 && !vm.nat_arg(fs, nargs, 1).is_nil() {
        vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?
    } else {
        1
    };
    let j = if nargs >= 3 && !vm.nat_arg(fs, nargs, 2).is_nil() {
        vm.int_from(vm.nat_arg(fs, nargs, 2), "use as an index")?
    } else {
        vm.checked_len(tv)?
    };
    if i > j {
        return Ok(0);
    }
    // result count in i128 so a full-i64 span (minint..maxint) can't wrap the
    // guard and trigger an astronomically large loop. PUC bounds the push by
    // `lua_checkstack(L, n+1)` — fail when there is not enough room above
    // the current top for the n returned values plus a slot for the
    // intermediate result table reference. coroutine.lua's "bug (stack
    // overflow)" series asserts a coroutine whose body already used a few
    // slots cannot unpack lim-10 values, so the check is against live
    // `stack_room`, not a static MAX_UNPACK absolute.
    let count = (j as i128) - (i as i128) + 1;
    if count >= MAX_UNPACK as i128 || count + 1 > vm.stack_room() as i128 {
        return Err(raise_str(vm, "too many results to unpack"));
    }
    // PUC unpack uses lua_geti, honouring __index.
    let mut vals: Vec<Value> = Vec::with_capacity(count as usize);
    for k in i..=j {
        vals.push(tab_geti(vm, tv, k)?);
    }
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
    let a1v = vm.nat_arg(fs, nargs, 0);
    checktab(vm, a1v, TAB_R, 1, "move")?;
    let f = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    let e = vm.int_from(vm.nat_arg(fs, nargs, 2), "use as an index")?;
    let d = vm.int_from(vm.nat_arg(fs, nargs, 3), "use as an index")?;
    let a2v = if nargs >= 5 {
        let v = vm.nat_arg(fs, nargs, 4);
        checktab(vm, v, TAB_W, 5, "move")?;
        v
    } else {
        a1v
    };
    // PUC table.move uses lua_geti/lua_seti, honouring __index/__newindex.
    if e >= f {
        // range/overflow guards before moving (PUC tmove): a maxint-sized
        // range or a destination that wraps past maxint is rejected, not
        // looped over
        if !(f > 0 || (e as i128) < i64::MAX as i128 + f as i128) {
            return Err(arg_error(vm, 3, "move", "too many elements to move"));
        }
        let n = e as i128 - f as i128 + 1;
        if (d as i128) > i64::MAX as i128 - n + 1 {
            return Err(arg_error(vm, 4, "move", "destination wrap around"));
        }
        if d > f && d <= e && a1v.raw_eq(a2v) {
            // overlapping forward: copy backwards
            let mut i = e;
            while i >= f {
                let v = tab_geti(vm, a1v, i)?;
                tab_seti(vm, a2v, d + (i - f), v)?;
                i -= 1;
            }
        } else {
            for i in f..=e {
                let v = tab_geti(vm, a1v, i)?;
                tab_seti(vm, a2v, d + (i - f), v)?;
            }
        }
    }
    Ok(vm.nat_return(fs, &[a2v]))
}

fn t_create(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let n = vm.int_from(vm.nat_arg(fs, nargs, 0), "use as a size")?;
    if !(0..=i32::MAX as i64).contains(&n) {
        return Err(arg_error(vm, 1, "create", "out of range"));
    }
    let m = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => 0,
        v => vm.int_from(v, "use as a size")?,
    };
    if !(0..=i32::MAX as i64).contains(&m) {
        return Err(arg_error(vm, 2, "create", "out of range"));
    }
    // PUC MAXHBITS: a hash part needs ceillog2(m) <= 30 bits; beyond 2^30
    // slots the resize raises "table overflow" rather than attempting it.
    if m > (1 << 30) {
        return Err(raise_str(vm, "table overflow"));
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
    let tv = vm.nat_arg(fs, nargs, 0);
    checktab(vm, tv, TAB_RW | TAB_L, 1, "sort")?;
    let comp = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => None,
        f @ (Value::Closure(_) | Value::Native(_)) => Some(f),
        v => {
            return Err(arg_error(
                vm,
                2,
                "sort",
                &format!("function expected, got {}", v.type_name()),
            ));
        }
    };
    let n = vm.checked_len(tv)?;
    if n > i64::from(u32::MAX) {
        return Err(arg_error(vm, 1, "sort", "array too big"));
    }
    // PUC sort uses lua_geti/lua_seti, honouring __index/__newindex.
    // Snapshot the working set into `vm.sort_scratch`, which `gc_roots`
    // traces — so a `collectgarbage()` invoked inside the comparator
    // (sort.lua's `load(..)(); collectgarbage()` callback) cannot free
    // strings/tables still held by the in-flight quicksort. Operating
    // on a Rust-local `Vec<Value>` (the pre-v2.1 shape) is UAF here.
    let cap = n.max(0) as usize;
    vm.sort_scratch.push(Vec::with_capacity(cap));
    let result = (|| -> Result<(), LuaError> {
        for i in 1..=n {
            let val = tab_geti(vm, tv, i)?;
            vm.sort_scratch.last_mut().unwrap().push(val);
        }
        sort_scratch_top(vm, comp)?;
        for i in 0..cap {
            let val = vm.sort_scratch.last().unwrap()[i];
            tab_seti(vm, tv, i as i64 + 1, val)?;
        }
        Ok(())
    })();
    // Pop scratch even on error so the rooted slots don't leak.
    vm.sort_scratch.pop();
    result?;
    Ok(0)
}

fn lt(vm: &mut Vm, comp: Option<Value>, a: Value, b: Value) -> Result<bool, LuaError> {
    match comp {
        // sort is an unprotected C call: the comparator runs non-yieldable.
        Some(f) => Ok(vm
            .call_noyield(f, &[a, b])?
            .first()
            .copied()
            .unwrap_or(Value::Nil)
            .truthy()),
        None => vm.less_than(a, b, false),
    }
}

#[inline]
fn scratch_get(vm: &Vm, i: usize) -> Value {
    vm.sort_scratch.last().unwrap()[i]
}

#[inline]
fn scratch_swap(vm: &mut Vm, i: usize, j: usize) {
    vm.sort_scratch.last_mut().unwrap().swap(i, j);
}

#[inline]
fn scratch_len(vm: &Vm) -> usize {
    vm.sort_scratch.last().unwrap().len()
}

/// Quicksort with median-of-three and an invalid-order guard (PUC auxsort
/// shape), operating on `vm.sort_scratch.last()` so GC sees the working
/// set as a root and the comparator's `collectgarbage()` can't free
/// snapshotted entries (sort.lua regression — see `t_sort`).
fn sort_scratch_top(vm: &mut Vm, comp: Option<Value>) -> Result<(), LuaError> {
    fn quick(
        vm: &mut Vm,
        comp: Option<Value>,
        mut lo: usize,
        mut hi: usize,
    ) -> Result<(), LuaError> {
        while lo < hi {
            // insertion sort only for tiny ranges; 4+ elements go through the
            // partition so an invalid order function is detected (PUC errors
            // for any size, and insertion sort can't notice inconsistency)
            if hi - lo < 3 {
                for i in lo + 1..=hi {
                    let mut j = i;
                    loop {
                        if j <= lo {
                            break;
                        }
                        let a = scratch_get(vm, j);
                        let b = scratch_get(vm, j - 1);
                        if !lt(vm, comp, a, b)? {
                            break;
                        }
                        scratch_swap(vm, j, j - 1);
                        j -= 1;
                    }
                }
                return Ok(());
            }
            // median of three at lo, mid, hi — values are re-fetched from
            // scratch on each compare because the comparator may have
            // mutated the table or freed Gc temporaries we previously
            // copied out.
            let mid = lo + (hi - lo) / 2;
            {
                let a = scratch_get(vm, mid);
                let b = scratch_get(vm, lo);
                if lt(vm, comp, a, b)? {
                    scratch_swap(vm, mid, lo);
                }
            }
            {
                let a = scratch_get(vm, hi);
                let b = scratch_get(vm, mid);
                if lt(vm, comp, a, b)? {
                    scratch_swap(vm, hi, mid);
                    let c = scratch_get(vm, mid);
                    let d = scratch_get(vm, lo);
                    if lt(vm, comp, c, d)? {
                        scratch_swap(vm, mid, lo);
                    }
                }
            }
            // Park the pivot at `hi - 1` so future iterations re-read it
            // from the GC-rooted scratch every compare instead of a
            // Rust-local copy that the next callback could dangle.
            scratch_swap(vm, mid, hi - 1);
            let pivot_idx = hi - 1;
            let (mut i, mut j) = (lo, hi - 1);
            loop {
                i += 1;
                loop {
                    let a = scratch_get(vm, i);
                    let p = scratch_get(vm, pivot_idx);
                    if !lt(vm, comp, a, p)? {
                        break;
                    }
                    if i >= hi {
                        return Err(raise_str(vm, "invalid order function for sorting"));
                    }
                    i += 1;
                }
                j -= 1;
                loop {
                    let p = scratch_get(vm, pivot_idx);
                    let b = scratch_get(vm, j);
                    if !lt(vm, comp, p, b)? {
                        break;
                    }
                    if j <= lo {
                        return Err(raise_str(vm, "invalid order function for sorting"));
                    }
                    j -= 1;
                }
                if i >= j {
                    break;
                }
                scratch_swap(vm, i, j);
            }
            scratch_swap(vm, i, hi - 1);
            // recurse on the smaller side, loop on the larger
            if i - lo < hi - i {
                if i > 0 {
                    quick(vm, comp, lo, i - 1)?;
                }
                lo = i + 1;
            } else {
                quick(vm, comp, i + 1, hi)?;
                if i == 0 {
                    break;
                }
                hi = i - 1;
            }
        }
        Ok(())
    }
    let n = scratch_len(vm);
    if n > 0 {
        quick(vm, comp, 0, n - 1)?;
    }
    Ok(())
}
