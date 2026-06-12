//! table library. sort is a PUC auxsort-shaped quicksort with Lua
//! comparators (std's sort would panic on non-total orders).

use crate::runtime::{Gc, Table, Value};
use crate::vm::builtins::{arg_error, check_table, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_table(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        unsafe { t.as_mut() }.set(k, fv).expect("valid key");
    };
    set(vm, "insert", t_insert);
    set(vm, "remove", t_remove);
    set(vm, "concat", t_concat);
    set(vm, "unpack", t_unpack);
    set(vm, "pack", t_pack);
    set(vm, "move", t_move);
    set(vm, "create", t_create);
    set(vm, "sort", t_sort);
    vm.set_global("table", Value::Table(t));
    // the global unpack alias exists in 5.1 mode only (P08)
}

fn t_insert(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, t, "insert")?;
    let n = t.len();
    let (pos, v) = match nargs {
        2 => (n + 1, vm.nat_arg(fs, nargs, 1)),
        3 => {
            let pos = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as a position")?;
            if pos < 1 || pos > n + 1 {
                return Err(arg_error(vm, 2, "insert", "position out of bounds"));
            }
            (pos, vm.nat_arg(fs, nargs, 2))
        }
        _ => return Err(raise_str(vm, "wrong number of arguments to 'insert'")),
    };
    let tm = unsafe { t.as_mut() };
    let mut i = n;
    while i >= pos {
        let mv = tm.get_int(i);
        tm.set_int(i + 1, mv);
        i -= 1;
    }
    tm.set_int(pos, v);
    Ok(0)
}

fn t_remove(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, t, "remove")?;
    let n = t.len();
    let pos = if nargs >= 2 {
        let pos = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as a position")?;
        if n > 0 && (pos < 1 || pos > n + 1) {
            return Err(arg_error(vm, 2, "remove", "position out of bounds"));
        }
        pos
    } else {
        n
    };
    let tm = unsafe { t.as_mut() };
    let removed = tm.get_int(pos);
    if pos <= n {
        let mut i = pos;
        while i < n {
            let mv = tm.get_int(i + 1);
            tm.set_int(i, mv);
            i += 1;
        }
        tm.set_int(n, Value::Nil);
    }
    Ok(vm.nat_return(fs, &[removed]))
}

fn t_concat(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, t, "concat")?;
    let sep: Vec<u8> = match vm.nat_arg(fs, nargs, 1) {
        Value::Nil => Vec::new(),
        Value::Str(s) => s.as_bytes().to_vec(),
        Value::Int(i) => crate::numeric::num_to_string(crate::numeric::Num::Int(i)).into_bytes(),
        Value::Float(f) => {
            crate::numeric::num_to_string(crate::numeric::Num::Float(f)).into_bytes()
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
        t.len()
    };
    let mut out: Vec<u8> = Vec::new();
    let mut k = i;
    while k <= j {
        match t.get_int(k) {
            Value::Str(s) => out.extend_from_slice(s.as_bytes()),
            Value::Int(x) => out.extend_from_slice(
                crate::numeric::num_to_string(crate::numeric::Num::Int(x)).as_bytes(),
            ),
            Value::Float(x) => out.extend_from_slice(
                crate::numeric::num_to_string(crate::numeric::Num::Float(x)).as_bytes(),
            ),
            _ => {
                return Err(raise_str(
                    vm,
                    &format!("invalid value (at index {k}) in table for 'concat'"),
                ));
            }
        }
        if k < j {
            out.extend_from_slice(&sep);
        }
        k += 1;
    }
    let s = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[s]))
}

const MAX_UNPACK: i64 = 1_000_000;

fn t_unpack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, t, "unpack")?;
    let i = if nargs >= 2 && !vm.nat_arg(fs, nargs, 1).is_nil() {
        vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?
    } else {
        1
    };
    let j = if nargs >= 3 && !vm.nat_arg(fs, nargs, 2).is_nil() {
        vm.int_from(vm.nat_arg(fs, nargs, 2), "use as an index")?
    } else {
        t.len()
    };
    if i > j {
        return Ok(0);
    }
    let count = j.wrapping_sub(i).wrapping_add(1);
    if !(0..=MAX_UNPACK).contains(&count) {
        return Err(raise_str(vm, "too many results to unpack"));
    }
    let vals: Vec<Value> = (i..=j).map(|k| t.get_int(k)).collect();
    Ok(vm.nat_return(fs, &vals))
}

fn t_pack(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.heap.new_table();
    {
        let tm = unsafe { t.as_mut() };
        for i in 0..nargs {
            let v = vm.nat_arg(fs, nargs, i);
            tm.set_int(i as i64 + 1, v);
        }
    }
    let nk = Value::Str(vm.heap.intern(b"n"));
    unsafe { t.as_mut() }
        .set(nk, Value::Int(nargs as i64))
        .expect("valid key");
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

fn t_move(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a1v = vm.nat_arg(fs, nargs, 0);
    let a1 = check_table(vm, a1v, "move")?;
    let f = vm.int_from(vm.nat_arg(fs, nargs, 1), "use as an index")?;
    let e = vm.int_from(vm.nat_arg(fs, nargs, 2), "use as an index")?;
    let d = vm.int_from(vm.nat_arg(fs, nargs, 3), "use as an index")?;
    let (a2v, a2) = if nargs >= 5 {
        let v = vm.nat_arg(fs, nargs, 4);
        (v, check_table(vm, v, "move")?)
    } else {
        (a1v, a1)
    };
    if e >= f {
        if d > f && d <= e && a1.ptr_eq(a2) {
            // overlapping forward: copy backwards
            let mut i = e;
            while i >= f {
                let v = a1.get_int(i);
                unsafe { a2.as_mut() }.set_int(d + (i - f), v);
                i -= 1;
            }
        } else {
            for i in f..=e {
                let v = a1.get_int(i);
                unsafe { a2.as_mut() }.set_int(d + (i - f), v);
            }
        }
    }
    Ok(vm.nat_return(fs, &[a2v]))
}

fn t_create(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // 5.5: size hints only; semantics are a fresh empty table
    let _ = vm.int_from(vm.nat_arg(fs, nargs, 0), "use as a size")?;
    let t = vm.heap.new_table();
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

fn t_sort(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = vm.nat_arg(fs, nargs, 0);
    let t = check_table(vm, t, "sort")?;
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
    let n = t.len();
    if n > i64::from(u32::MAX) {
        return Err(arg_error(vm, 1, "sort", "array too big"));
    }
    let mut v: Vec<Value> = (1..=n).map(|i| t.get_int(i)).collect();
    sort_slice(vm, &mut v, comp, t)?;
    Ok(0)
}

fn lt(vm: &mut Vm, comp: Option<Value>, a: Value, b: Value) -> Result<bool, LuaError> {
    match comp {
        Some(f) => Ok(vm
            .call_value(f, &[a, b])?
            .first()
            .copied()
            .unwrap_or(Value::Nil)
            .truthy()),
        None => vm.less_than(a, b, false),
    }
}

/// Quicksort with median-of-three and an invalid-order guard (PUC auxsort
/// shape); writes back into the table as it finishes partitions.
fn sort_slice(
    vm: &mut Vm,
    v: &mut [Value],
    comp: Option<Value>,
    t: Gc<Table>,
) -> Result<(), LuaError> {
    fn quick(
        vm: &mut Vm,
        v: &mut [Value],
        comp: Option<Value>,
        mut lo: usize,
        mut hi: usize,
    ) -> Result<(), LuaError> {
        while lo < hi {
            // insertion sort for small ranges
            if hi - lo < 12 {
                for i in lo + 1..=hi {
                    let mut j = i;
                    while j > lo && lt(vm, comp, v[j], v[j - 1])? {
                        v.swap(j, j - 1);
                        j -= 1;
                    }
                }
                return Ok(());
            }
            // median of three at lo, mid, hi
            let mid = lo + (hi - lo) / 2;
            if lt(vm, comp, v[mid], v[lo])? {
                v.swap(mid, lo);
            }
            if lt(vm, comp, v[hi], v[mid])? {
                v.swap(hi, mid);
                if lt(vm, comp, v[mid], v[lo])? {
                    v.swap(mid, lo);
                }
            }
            let pivot = v[mid];
            v.swap(mid, hi - 1);
            let (mut i, mut j) = (lo, hi - 1);
            loop {
                i += 1;
                while lt(vm, comp, v[i], pivot)? {
                    if i >= hi {
                        return Err(raise_str(vm, "invalid order function for sorting"));
                    }
                    i += 1;
                }
                j -= 1;
                while lt(vm, comp, pivot, v[j])? {
                    if j <= lo {
                        return Err(raise_str(vm, "invalid order function for sorting"));
                    }
                    j -= 1;
                }
                if i >= j {
                    break;
                }
                v.swap(i, j);
            }
            v.swap(i, hi - 1);
            // recurse on the smaller side, loop on the larger
            if i - lo < hi - i {
                if i > 0 {
                    quick(vm, v, comp, lo, i - 1)?;
                }
                lo = i + 1;
            } else {
                quick(vm, v, comp, i + 1, hi)?;
                if i == 0 {
                    break;
                }
                hi = i - 1;
            }
        }
        Ok(())
    }
    if !v.is_empty() {
        let hi = v.len() - 1;
        quick(vm, v, comp, 0, hi)?;
    }
    let tm = unsafe { t.as_mut() };
    for (i, &val) in v.iter().enumerate() {
        tm.set_int(i as i64 + 1, val);
    }
    Ok(())
}
