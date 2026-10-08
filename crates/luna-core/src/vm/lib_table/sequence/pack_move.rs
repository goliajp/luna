//! `table.pack`, `table.move` and `table.create`.

use super::*;

pub(crate) fn t_pack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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

pub(crate) fn t_move(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
                let v = tab_geti(vm, a1, f + i, 0)?;
                tab_seti(vm, a2, t + i, v, 0)?;
            }
        } else {
            for i in (0..n).rev() {
                let v = tab_geti(vm, a1, f + i, 0)?;
                tab_seti(vm, a2, t + i, v, 0)?;
            }
        }
    }
    Ok(vm.nat_return(fs, &[a2]))
}

pub(crate) fn t_create(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
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
