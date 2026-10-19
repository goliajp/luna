//! `table.pack`, `table.move` and `table.create`.

use super::*;

pub(crate) fn t_pack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // `lua_createtable(L, n, 1)`: the arguments, nil ones included, fill an
    // array part of exactly their count; the table is young until it is
    // returned, so an emergency collection keeps it
    vm.api_check_before()?;
    let t = vm.alloc_table()?;
    let r = vm.table_op(|vm| {
        // SAFETY: `t` was allocated above and is held only by this local;
        // the borrow covers the one resize
        unsafe { t.as_mut() }.resize(&mut vm.heap, nargs as usize, 1)
    });
    if let Err(e) = r {
        return Err(vm.table_error(e));
    }
    vm.api_check_after(Value::Table(t))?;
    for i in 0..nargs {
        let v = vm.nat_arg(fs, nargs, i);
        // SAFETY: as above, for this one store
        unsafe { t.as_mut() }.set_list_slot(i as usize, v);
    }
    let nk = vm.str_value(b"n")?;
    vm.lib_set_value(t, nk, Value::Int(nargs as i64))?;
    vm.nat_ret(fs, &[Value::Table(t)])
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
                tab_seti_held(vm, a2, t + i, v, &[v])?;
            }
        } else {
            for i in (0..n).rev() {
                let v = tab_geti(vm, a1, f + i, 0)?;
                tab_seti_held(vm, a2, t + i, v, &[v])?;
            }
        }
    }
    vm.nat_ret(fs, &[a2])
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
    #[cfg(target_pointer_width = "64")]
    if m > (1 << 30) {
        return Err(vm.plain_err("table overflow"));
    }
    // a 32-bit `size_t` bounds both parts first: MAXASIZE is
    // `MAX_SIZET / (sizeof(Value) + 1)`, MAXHSIZE `MAX_SIZET / sizeof(Node)`
    // (8 and 24 bytes), and the array part is checked first
    #[cfg(target_pointer_width = "32")]
    if n > u64::from(u32::MAX / 9) || m.next_power_of_two() > u64::from(u32::MAX / 24) {
        return Err(vm.plain_err("table overflow"));
    }
    // the parts' sizes count in `Heap.bytes`, and `free_obj` takes them
    // off again: 5.5 sort.lua:22 checks `memdiff > N * 4` after
    // `table.create(N)`
    let t = vm.alloc_table_sized(n as usize, m as usize)?;
    vm.api_check_after(Value::Table(t))?;
    vm.nat_ret(fs, &[Value::Table(t)])
}
