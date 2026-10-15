//! `string.format`, one driver per generation of PUC's `str_format`:
//! 5.1/5.2 pass numbers through C casts and truncate at zero bytes, 5.3
//! reads integers exactly, 5.4 validates every specification's flags.
//! The items themselves are rendered by [`crate::vm::cfmt`].

use crate::runtime::Value;
use crate::runtime::mem::LVec;
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_bytes, raise_str};
use crate::vm::cfmt::{self, Spec};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;
mod item51;
mod item54;
mod quoted;
use item51::item51;
use item54::item54;
pub(crate) use quoted::*;

/// PUC `MAX_FORMAT`: a 5.4+ specification plus '%', a length modifier
/// and the terminator must fit in 32 bytes.
const MAX_FORMAT: usize = 32;
const FLAGS: &[u8] = b"-+ #0";
/// 2^63, the first double past `i64`.
const TWO63: f64 = 9_223_372_036_854_775_808.0;

pub(crate) fn s_format(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let f = argcheck::check_string(vm, a, 0)?;
    let fmt = f.as_bytes();
    let v = vm.version();
    // room for the format and an item before the buffer regrows
    let mut out = vm.new_buf();
    vm.buf_reserve(&mut out, fmt.len() + cfmt::ITEM_MAX)?;
    let mut arg = 0u32;
    let mut i = 0;
    // the buffer's slot counts only when a callback runs or an error is
    // raised: it is added then, from the content built so far
    while i < fmt.len() {
        let c = fmt[i];
        i += 1;
        if c != b'%' {
            // the run of plain bytes up to the next '%', in one copy
            let end = fmt[i..]
                .iter()
                .position(|&b| b == b'%')
                .map_or(fmt.len(), |k| i + k);
            vm.buf_push(&mut out, &fmt[i - 1..end])?;
            i = end;
            continue;
        }
        // a '%' at the very end pairs with the string's terminating zero
        if fmt.get(i) == Some(&b'%') {
            vm.buf_push(&mut out, b"%")?;
            i += 1;
            continue;
        }
        arg += 1;
        if arg >= nargs {
            let buf = vm.buffer_slot(out.len());
            vm.native_push(buf);
            return Err(arg_error(vm, arg + 1, "no value"));
        }
        let item = if v >= LuaVersion::Lua54 {
            item54(vm, a, arg, fmt, i, &mut out)
        } else {
            item51(vm, a, arg, fmt, i, &mut out)
        };
        i = match item {
            Ok(i) => i,
            Err(e) => {
                let buf = vm.buffer_slot(out.len());
                vm.native_push(buf);
                return Err(e);
            }
        };
    }
    let s = vm.lib_built_str(&out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[s])
}

fn byte_at(fmt: &[u8], i: usize) -> u8 {
    fmt.get(i).copied().unwrap_or(0)
}

/// ≤5.3 `scanformat`: at most five flag bytes, two width digits, two
/// precision digits; the next byte is the conversion, whatever it is.
/// Returns the position of the conversion byte.
fn scanformat(vm: &mut Vm, fmt: &[u8], start: usize) -> Result<usize, LuaError> {
    let mut p = start;
    while fmt.get(p).is_some_and(|c| FLAGS.contains(c)) {
        p += 1;
    }
    if p - start > FLAGS.len() {
        return Err(raise_str(vm, "invalid format (repeated flags)"));
    }
    let digit = |p: usize| fmt.get(p).is_some_and(u8::is_ascii_digit);
    for _ in 0..2 {
        if digit(p) {
            p += 1;
        }
    }
    if fmt.get(p) == Some(&b'.') {
        p += 1;
        for _ in 0..2 {
            if digit(p) {
                p += 1;
            }
        }
    }
    if digit(p) {
        return Err(raise_str(
            vm,
            "invalid format (width or precision too long)",
        ));
    }
    Ok(p)
}

/// Argument `arg` when it is a string `luaL_tolstring` gives back as it
/// is (no `__tostring` in the way).
fn plain_str(vm: &mut Vm, a: Args, arg: u32) -> Option<crate::runtime::Gc<crate::runtime::LuaStr>> {
    let v = a.get(vm, arg);
    match v {
        Value::Str(s) if vm.get_mm(v, crate::vm::exec::Mm::ToString).is_nil() => Some(s),
        _ => None,
    }
}
