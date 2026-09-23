//! `bit32`, following PUC `lbitlib.c`: the 5.2 library, and 5.3's copy kept
//! by the default LUA_COMPAT_BITLIB. Results are unsigned 32-bit values.
//!
//! The two versions read their operands differently. 5.2's `lua_Unsigned`
//! is 32 bits and `luaL_checkunsigned` rounds a float to nearest and wraps
//! it (see [`argcheck::check_unsigned52`]); 5.3 takes `luaL_checkinteger`
//! (so the float must be integral) and trims to 32 bits afterwards. Shift
//! and field arguments are C `int`s on 5.2 and `lua_Integer`s on 5.3.

use crate::runtime::Value;
use crate::version::LuaVersion as V;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

type Native = fn(&mut Vm, u32, u32) -> Result<u32, LuaError>;

pub(crate) fn open_bit32(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let funcs: [(&str, Native); 12] = [
        ("band", b_and),
        ("bor", b_or),
        ("bxor", b_xor),
        ("bnot", b_not),
        ("btest", b_test),
        ("lshift", b_lshift),
        ("rshift", b_rshift),
        ("arshift", b_arshift),
        ("lrotate", b_lrot),
        ("rrotate", b_rrot),
        ("extract", b_extract),
        ("replace", b_replace),
    ];
    for (name, f) in funcs {
        let fv = vm.native(f);
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    }
    vm.set_global("bit32", Value::Table(t))
        .expect("stdlib registration");
    vm.barrier_back_table(t);
    // `luaL_requiref` also records the module in `package.loaded`, which is
    // what `require "bit32"` and error messages naming `bit32.band` look up.
    // bit32 opens after the package library, so it registers itself.
    let pk = Value::Str(vm.heap.intern(b"package"));
    let lk = Value::Str(vm.heap.intern(b"loaded"));
    if let Value::Table(pkg) = vm.globals().get(pk)
        && let Value::Table(loaded) = pkg.get(lk)
    {
        let k = Value::Str(vm.heap.intern(b"bit32"));
        // SAFETY: Gc<T> is NonNull<T> over the GC heap; the heap is single-threaded and the pointer is live as long as it is reachable from active roots (see heap.rs:5-7).
        unsafe { loaded.as_mut() }
            .set(&mut vm.heap, k, Value::Table(t))
            .expect("valid key");
        vm.barrier_back_table(loaded);
    }
}

const ALLONES: u64 = 0xFFFF_FFFF;

/// `checkunsigned`, as the full-width value 5.3 works with before trimming.
fn unsigned(vm: &mut Vm, a: Args, i: u32) -> Result<u64, LuaError> {
    if vm.version() <= V::Lua52 {
        Ok(argcheck::check_unsigned52(vm, a, i)?.into())
    } else {
        Ok(argcheck::check_integer(vm, a, i)? as u64)
    }
}

/// A shift, rotation or field argument: `luaL_checkint` on 5.2,
/// `luaL_checkinteger` on 5.3.
fn int_arg(vm: &mut Vm, a: Args, i: u32) -> Result<i64, LuaError> {
    if vm.version() <= V::Lua52 {
        Ok(argcheck::check_int(vm, a, i)?.into())
    } else {
        argcheck::check_integer(vm, a, i)
    }
}

fn push(vm: &mut Vm, fs: u32, r: u64) -> Result<u32, LuaError> {
    Ok(vm.nat_return(fs, &[Value::Int(r as i64)]))
}

fn andaux(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u64, LuaError> {
    let a = Args::new(fs, nargs);
    let mut r = u64::MAX;
    for i in 0..nargs {
        r &= unsigned(vm, a, i)?;
    }
    Ok(r & ALLONES)
}

fn b_and(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let r = andaux(vm, fs, nargs)?;
    push(vm, fs, r)
}

fn b_test(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let r = andaux(vm, fs, nargs)?;
    Ok(vm.nat_return(fs, &[Value::Bool(r != 0)]))
}

fn fold(vm: &mut Vm, fs: u32, nargs: u32, op: fn(u64, u64) -> u64) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let mut r = 0;
    for i in 0..nargs {
        r = op(r, unsigned(vm, a, i)?);
    }
    push(vm, fs, r & ALLONES)
}

fn b_or(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    fold(vm, fs, nargs, |x, y| x | y)
}

fn b_xor(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    fold(vm, fs, nargs, |x, y| x ^ y)
}

fn b_not(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let r = !unsigned(vm, Args::new(fs, nargs), 0)?;
    push(vm, fs, r & ALLONES)
}

/// `b_shift`: a positive displacement shifts left, a negative one right;
/// 32 or more clears everything.
fn shift(r: u64, i: i64) -> u64 {
    if i < 0 {
        let i = i.wrapping_neg();
        if i >= 32 { 0 } else { (r & ALLONES) >> i }
    } else if i >= 32 {
        0
    } else {
        (r << i) & ALLONES
    }
}

fn b_lshift(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let r = unsigned(vm, a, 0)?;
    let i = int_arg(vm, a, 1)?;
    push(vm, fs, shift(r, i))
}

fn b_rshift(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let r = unsigned(vm, a, 0)?;
    let i = int_arg(vm, a, 1)?;
    push(vm, fs, shift(r, i.wrapping_neg()))
}

fn b_arshift(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let r = unsigned(vm, a, 0)?;
    let i = int_arg(vm, a, 1)?;
    let r = if i < 0 || r & (1 << 31) == 0 {
        shift(r, i.wrapping_neg())
    } else if i >= 32 {
        ALLONES
    } else {
        // shift in copies of the sign bit
        ((r >> i) | !(ALLONES >> i)) & ALLONES
    };
    push(vm, fs, r)
}

/// `b_rot`: the displacement is taken mod 32. It is read before the value
/// (`b_rot(L, luaL_checkint(L, 2))`), so a bad displacement is reported
/// first.
fn rotate(vm: &mut Vm, fs: u32, nargs: u32, negate: bool) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let d = int_arg(vm, a, 1)?;
    let r = unsigned(vm, a, 0)? & ALLONES;
    let d = if negate { d.wrapping_neg() } else { d };
    let r = (r as u32).rotate_left((d & 31) as u32);
    push(vm, fs, r.into())
}

fn b_lrot(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    rotate(vm, fs, nargs, false)
}

fn b_rrot(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    rotate(vm, fs, nargs, true)
}

/// `fieldargs`: field `f` at argument `farg`, width at `farg + 1`
/// (default 1).
fn fieldargs(vm: &mut Vm, a: Args, farg: u32) -> Result<(u32, u32), LuaError> {
    let f = int_arg(vm, a, farg)?;
    let w = if a.is_none_or_nil(vm, farg + 1) {
        1
    } else {
        int_arg(vm, a, farg + 1)?
    };
    if f < 0 {
        return Err(arg_error(vm, farg + 1, "field cannot be negative"));
    }
    if w <= 0 {
        return Err(arg_error(vm, farg + 2, "width must be positive"));
    }
    // PUC adds in `int`/`lua_Integer`, and a sum that overflows slips past
    // this check into undefined shifts; the true sum is what is meant.
    if f.saturating_add(w) > 32 {
        return Err(raise_str(vm, "trying to access non-existent bits"));
    }
    Ok((f as u32, w as u32))
}

/// `mask(w)`: `w` low bits set, 1 <= w <= 32.
fn mask(w: u32) -> u64 {
    ALLONES >> (32 - w)
}

fn b_extract(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let r = unsigned(vm, a, 0)? & ALLONES;
    let (f, w) = fieldargs(vm, a, 1)?;
    push(vm, fs, (r >> f) & mask(w))
}

fn b_replace(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let r = unsigned(vm, a, 0)? & ALLONES;
    let v = unsigned(vm, a, 1)? & ALLONES;
    let (f, w) = fieldargs(vm, a, 2)?;
    let m = mask(w);
    push(vm, fs, ((r & !(m << f)) | ((v & m) << f)) & ALLONES)
}
