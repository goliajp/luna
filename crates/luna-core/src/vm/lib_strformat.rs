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
use crate::vm::puc_buffer::PucBuffer;
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

/// How PUC's `str_format` put an item's text into its `luaL_Buffer`.
pub(crate) enum Added {
    /// written into room asked for first (`luaL_addsize`; 5.1 copies it
    /// with `luaL_addlstring`)
    Sized,
    /// a character or a few at a time (`addquoted`)
    Chars,
    /// the whole string (`luaL_addvalue`)
    Value(crate::runtime::Gc<crate::runtime::LuaStr>),
}

/// PUC `MAX_ITEM`, the room `str_format` asks for before an item (5.1
/// formats into a C array instead).
fn max_item(v: LuaVersion) -> usize {
    match v {
        LuaVersion::Lua51 | LuaVersion::Lua52 => 512,
        LuaVersion::Lua53 => 120 + 308,
        _ => 120,
    }
}

pub(crate) fn s_format<const PRE: bool>(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // 5.1's buffer keeps its strings on the stack: an error lets them go.
    // Every other way out leaves the stack as it found it (a 5.1 result
    // takes the strings off as it joins them).
    let m = vm.lib_roots.mark();
    let at = m.at();
    let r = format::<PRE>(vm, fs, nargs);
    if PRE && r.is_err() {
        vm.lib_roots.release(m);
    }
    debug_assert!(r.is_err() || vm.lib_roots.len() == at);
    r
}

#[inline(always)]
fn format<const PRE: bool>(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let f = argcheck::check_string(vm, a, 0)?;
    let fmt = f.as_bytes();
    let v = vm.version();
    // room for the format and an item before the buffer regrows
    let mut out = vm.new_buf();
    vm.buf_reserve(&mut out, fmt.len() + cfmt::ITEM_MAX)?;
    let mut b = PucBuffer::new_by::<PRE>(vm);
    // While the content, and the room asked for before an item, fit the
    // buffer's first block, nothing the buffer does allocates and its count
    // is the content's length: it is told only once that stops holding
    // (`quick` cleared), from the length it had before that step.
    let first = b.first_size();
    let mut quick = true;
    macro_rules! tell {
        ($fits:expr, $before:expr, $step:expr) => {
            if !(quick && $fits) {
                if quick {
                    quick = false;
                    b.set_count($before);
                }
                $step;
            }
        };
    }
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
            let k = end - (i - 1);
            tell!(out.len() <= first, out.len() - k, b.add_chars(vm, &out, k)?);
            i = end;
            continue;
        }
        // a '%' at the very end pairs with the string's terminating zero
        if fmt.get(i) == Some(&b'%') {
            vm.buf_push(&mut out, b"%")?;
            tell!(out.len() <= first, out.len() - 1, b.add_chars(vm, &out, 1)?);
            i += 1;
            continue;
        }
        if v >= LuaVersion::Lua52 {
            let m = max_item(v);
            tell!(out.len() + m <= first, out.len(), b.prep(vm, &out, m)?);
            // 5.4 on ask for more for a `%f`
            if v >= LuaVersion::Lua54 && item_conv(fmt, i) == b'f' {
                let m = 110 + 308;
                tell!(out.len() + m <= first, out.len(), b.prep(vm, &out, m)?);
            }
        }
        arg += 1;
        if arg >= nargs {
            let buf = vm.buffer_slot(out.len());
            vm.native_push(buf);
            return Err(arg_error(vm, arg + 1, "no value"));
        }
        let before = out.len();
        let item = if v >= LuaVersion::Lua54 {
            item54::<PRE>(vm, a, arg, fmt, i, &mut out)
        } else {
            item51::<PRE>(vm, a, arg, fmt, i, &mut out)
        };
        i = match item {
            Ok((i, Added::Value(s))) => {
                tell!(out.len() <= first, before, b.add_value(vm, &out, s)?);
                i
            }
            Ok((i, Added::Chars)) => {
                let k = out.len() - before;
                tell!(out.len() <= first, before, b.add_chars(vm, &out, k)?);
                i
            }
            Ok((i, Added::Sized)) if v == LuaVersion::Lua51 => {
                let k = out.len() - before;
                tell!(out.len() <= first, before, b.add_chars(vm, &out, k)?);
                i
            }
            Ok((i, Added::Sized)) => {
                let k = out.len() - before;
                tell!(out.len() <= first, before, b.add_size(k));
                i
            }
            Err(e) => {
                let buf = vm.buffer_slot(out.len());
                vm.native_push(buf);
                return Err(e);
            }
        };
    }
    if out.len() > crate::runtime::string::MAX_LEN {
        return Err(vm.rt_err("string length overflow"));
    }
    if quick {
        b.set_count(out.len());
    }
    let s = b.finish::<PRE>(vm, &out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[Value::Str(s)])
}

/// The conversion byte of the item whose specification starts at `i`.
fn item_conv(fmt: &[u8], i: usize) -> u8 {
    let mut p = i;
    while fmt.get(p).is_some_and(|c| b"-+#0 123456789.".contains(c)) {
        p += 1;
    }
    byte_at(fmt, p)
}

fn byte_at(fmt: &[u8], i: usize) -> u8 {
    fmt.get(i).copied().unwrap_or(0)
}

/// ≤5.3 `scanformat`: at most five flag bytes, two width digits, two
/// precision digits; the next byte is the conversion, whatever it is.
/// Returns the position of the conversion byte.
#[inline]
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
fn plain_str<const PRE: bool>(
    vm: &mut Vm,
    a: Args,
    arg: u32,
) -> Result<Option<crate::runtime::Gc<crate::runtime::LuaStr>>, LuaError> {
    let v = a.get(vm, arg);
    Ok(match v {
        Value::Str(s) if vm.get_mm(v, crate::vm::exec::Mm::ToString).is_nil() => {
            // `luaL_tolstring`'s `luaL_callmeta`
            vm.metafield_check::<PRE>(v, b"__tostring")?;
            Some(s)
        }
        _ => None,
    })
}
