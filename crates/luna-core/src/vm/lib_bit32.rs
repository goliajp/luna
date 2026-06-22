//! PUC 5.2 `bit32` library. Every operation treats its operands as unsigned
//! 32-bit integers — operands wider than 32 bits are truncated mod 2^32, the
//! result is reported in [0, 2^32). 5.3 retired the library in favour of
//! native 64-bit bitwise operators, so this surface is only registered when
//! the VM is running in `Lua52` mode.

use crate::runtime::Value;
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

pub(crate) fn open_bit32(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let set = |vm: &mut Vm, name: &str, f| {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }.set(&mut vm.heap, k, fv).expect("valid key");
    };
    set(vm, "band", b_band);
    set(vm, "bor", b_bor);
    set(vm, "bxor", b_bxor);
    set(vm, "bnot", b_bnot);
    set(vm, "btest", b_btest);
    set(vm, "lshift", b_lshift);
    set(vm, "rshift", b_rshift);
    set(vm, "arshift", b_arshift);
    set(vm, "lrotate", b_lrotate);
    set(vm, "rrotate", b_rrotate);
    set(vm, "extract", b_extract);
    set(vm, "replace", b_replace);
    vm.set_global("bit32", Value::Table(t));
    vm.barrier_back_table(t);
}

/// `lua_tounsignedx` for `bit32`: an integer-valued double becomes its low 32
/// bits (PUC `b_arg` mods by 2^32). Strings flow through `tonumber`, then the
/// same modular reduction.
fn to_u32(vm: &mut Vm, v: Value, n: u32, who: &str) -> Result<u32, LuaError> {
    let f = match v {
        Value::Int(i) => i as f64,
        Value::Float(f) => f,
        Value::Str(s) => match crate::numeric::str2num(s.as_bytes(), true, true) {
            Some(crate::numeric::Num::Int(i)) => i as f64,
            Some(crate::numeric::Num::Float(f)) => f,
            None => return Err(arg_error(vm, n, who, "number expected")),
        },
        _ => return Err(arg_error(vm, n, who, "number expected")),
    };
    if !f.is_finite() || f.fract() != 0.0 {
        return Err(arg_error(vm, n, who, "number has no integer representation"));
    }
    // PUC `b_arg`: cast to `lua_Unsigned` (= unsigned long long) then truncate
    // mod 2^32. Floats outside i64 range fold via wrapping before the mask.
    let bits = if f >= 0.0 && f < (u64::MAX as f64) {
        f as u64
    } else {
        (f as i64) as u64
    };
    Ok((bits & 0xFFFF_FFFF) as u32)
}

fn fold(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    who: &str,
    init: u32,
    op: fn(u32, u32) -> u32,
) -> Result<u32, LuaError> {
    let mut acc = init;
    for i in 0..nargs {
        let v = vm.nat_arg(fs, nargs, i);
        let u = to_u32(vm, v, i + 1, who)?;
        acc = op(acc, u);
    }
    Ok(vm.nat_return(fs, &[Value::Int(acc as i64)]))
}

fn b_band(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    fold(vm, fs, nargs, "band", 0xFFFF_FFFF, |a, b| a & b)
}

fn b_bor(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    fold(vm, fs, nargs, "bor", 0, |a, b| a | b)
}

fn b_bxor(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    fold(vm, fs, nargs, "bxor", 0, |a, b| a ^ b)
}

fn b_btest(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let mut acc: u32 = 0xFFFF_FFFF;
    for i in 0..nargs {
        let v = vm.nat_arg(fs, nargs, i);
        let u = to_u32(vm, v, i + 1, "btest")?;
        acc &= u;
    }
    Ok(vm.nat_return(fs, &[Value::Bool(acc != 0)]))
}

fn b_bnot(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.nat_arg(fs, nargs, 0);
    let u = to_u32(vm, v, 1, "bnot")?;
    Ok(vm.nat_return(fs, &[Value::Int((!u) as i64)]))
}

/// PUC `b_shift`: a positive `disp` shifts left; a negative one shifts right
/// (used by both `lshift` and `rshift` with sign-inverted `disp`). Shifts of
/// 32 bits or more zero the result.
fn signed_shift(x: u32, disp: i32) -> u32 {
    let d = disp.unsigned_abs();
    if d >= 32 {
        0
    } else if disp >= 0 {
        x.wrapping_shl(d)
    } else {
        x.wrapping_shr(d)
    }
}

fn shift_arg(vm: &mut Vm, fs: u32, nargs: u32, who: &str) -> Result<(u32, i32), LuaError> {
    let v0 = vm.nat_arg(fs, nargs, 0);
    let v1 = vm.nat_arg(fs, nargs, 1);
    let x = to_u32(vm, v0, 1, who)?;
    let d = vm.int_from(v1, "use as a number")?;
    // PUC `b_shift` clamps |disp| to 32 via the shift implementation; pass the
    // signed value through unchanged.
    let d32 = if d > i32::MAX as i64 {
        i32::MAX
    } else if d < i32::MIN as i64 {
        i32::MIN
    } else {
        d as i32
    };
    Ok((x, d32))
}

fn b_lshift(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (x, d) = shift_arg(vm, fs, nargs, "lshift")?;
    let r = signed_shift(x, d);
    Ok(vm.nat_return(fs, &[Value::Int(r as i64)]))
}

fn b_rshift(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (x, d) = shift_arg(vm, fs, nargs, "rshift")?;
    let r = signed_shift(x, -d);
    Ok(vm.nat_return(fs, &[Value::Int(r as i64)]))
}

fn b_arshift(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (x, d) = shift_arg(vm, fs, nargs, "arshift")?;
    let r = if d >= 0 {
        // PUC arithmetic right shift: sign-extend the 32-bit value before the
        // shift; a disp >= 32 saturates to all-sign.
        let s = x as i32;
        if d >= 32 {
            if s < 0 { 0xFFFF_FFFF } else { 0 }
        } else {
            (s >> d) as u32
        }
    } else {
        signed_shift(x, -d)
    };
    Ok(vm.nat_return(fs, &[Value::Int(r as i64)]))
}

fn rotate(x: u32, disp: i32) -> u32 {
    // PUC `b_rotate`: `disp` reduced mod 32 (negative wraps positive).
    let d = (disp.rem_euclid(32)) as u32;
    x.rotate_left(d)
}

fn b_lrotate(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (x, d) = shift_arg(vm, fs, nargs, "lrotate")?;
    Ok(vm.nat_return(fs, &[Value::Int(rotate(x, d) as i64)]))
}

fn b_rrotate(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let (x, d) = shift_arg(vm, fs, nargs, "rrotate")?;
    Ok(vm.nat_return(fs, &[Value::Int(rotate(x, -d) as i64)]))
}

fn field_args(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    field_idx: u32,
    who: &str,
) -> Result<(u32, u32), LuaError> {
    let field = vm.int_from(vm.nat_arg(fs, nargs, field_idx), "use as a number")?;
    let width = if nargs > field_idx + 1 {
        vm.int_from(vm.nat_arg(fs, nargs, field_idx + 1), "use as a number")?
    } else {
        1
    };
    // PUC `fieldargs`: f in [0, 31], w in [1, 32], f + w in [1, 32].
    if !(0..=31).contains(&field) {
        return Err(arg_error(vm, field_idx + 1, who, "field out of range"));
    }
    if !(1..=32).contains(&width) {
        return Err(arg_error(vm, field_idx + 2, who, "trying to access non-existent bits"));
    }
    if field + width > 32 {
        return Err(arg_error(vm, field_idx + 1, who, "trying to access non-existent bits"));
    }
    Ok((field as u32, width as u32))
}

fn b_extract(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let n = to_u32(vm, vm.nat_arg(fs, nargs, 0), 1, "extract")?;
    let (f, w) = field_args(vm, fs, nargs, 1, "extract")?;
    let mask = if w == 32 { 0xFFFF_FFFF } else { (1u32 << w) - 1 };
    let r = (n >> f) & mask;
    Ok(vm.nat_return(fs, &[Value::Int(r as i64)]))
}

fn b_replace(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let n = to_u32(vm, vm.nat_arg(fs, nargs, 0), 1, "replace")?;
    let v = to_u32(vm, vm.nat_arg(fs, nargs, 1), 2, "replace")?;
    let (f, w) = field_args(vm, fs, nargs, 2, "replace")?;
    let mask = if w == 32 { 0xFFFF_FFFF } else { (1u32 << w) - 1 };
    let r = (n & !(mask << f)) | ((v & mask) << f);
    Ok(vm.nat_return(fs, &[Value::Int(r as i64)]))
}

// keep `raise_str` available for future error paths
#[allow(dead_code)]
fn _keep(_vm: &mut Vm) -> LuaError {
    raise_str(_vm, "unused")
}
