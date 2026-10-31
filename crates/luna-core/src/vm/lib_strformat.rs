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
use item54::{item54, spec_end};
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
    // takes the strings off as it joins them). Only a begun buffer pushes
    // any.
    #[cfg(debug_assertions)]
    let at = vm.lib_roots.len();
    let mut b: Option<PucBuffer> = None;
    let r = format::<PRE>(vm, fs, nargs, &mut b);
    if PRE && r.is_err() {
        abandon(vm, &mut b);
    }
    #[cfg(debug_assertions)]
    debug_assert!((r.is_err() && !PRE) || vm.lib_roots.len() == at);
    r
}

#[cold]
#[inline(never)]
fn abandon(vm: &mut Vm, b: &mut Option<PucBuffer>) {
    if let Some(b) = b.take() {
        b.abandon(vm);
    }
}

/// `string.format`'s work. The buffer is made only once the content and
/// the room asked for before an item outgrow its first block (until then
/// it allocates nothing and counts the content's length), with the length
/// before that step: `lim`, one past the first block until then and zero
/// after, tells in one compare both that and whether the buffer exists.
#[inline(always)]
fn format<const PRE: bool>(
    vm: &mut Vm,
    fs: u32,
    nargs: u32,
    b: &mut Option<PucBuffer>,
) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let f = argcheck::check_string(vm, a, 0)?;
    let fmt = f.as_bytes();
    let v = vm.version();
    // room for the format and an item before the buffer regrows
    let mut out = vm.new_buf();
    vm.buf_reserve(&mut out, fmt.len() + cfmt::ITEM_MAX)?;
    let mut lim = PucBuffer::first_size::<PRE>(vm) + 1;
    macro_rules! tell {
        ($need:expr, $before:expr, |$b:ident| $step:expr) => {
            if $need >= lim {
                let $b = match b {
                    Some(b) => b,
                    None => {
                        lim = 0;
                        b.insert(PucBuffer::begun::<PRE>(vm, $before))
                    }
                };
                $step;
            }
        };
    }
    // the room asked for before each item (5.2 on; a constant for `PRE`),
    // and whether a `%f` asks for more (5.4 on)
    let room = if PRE { 512 } else { max_item(v) };
    let room_f = !PRE && v >= LuaVersion::Lua54;
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
            tell!(out.len(), out.len() - k, |b| b.add_chars(vm, &out, k)?);
            i = end;
            continue;
        }
        // a '%' at the very end pairs with the string's terminating zero
        if fmt.get(i) == Some(&b'%') {
            vm.buf_push(&mut out, b"%")?;
            tell!(out.len(), out.len() - 1, |b| b.add_chars(vm, &out, 1)?);
            i += 1;
            continue;
        }
        if !PRE || v >= LuaVersion::Lua52 {
            tell!(out.len() + room, out.len(), |b| b.prep(vm, &out, room)?);
        }
        arg += 1;
        if arg >= nargs {
            return Err(no_value(vm, out.len(), arg));
        }
        let mut added = Added::Sized;
        // `PRE` is 5.1 and 5.2: their build has no 5.4 item to carry
        let (before, item) = if room_f {
            // 5.4 on read the specification once (`getformat`'s bytes end
            // at `p`), and ask for more for a `%f` once it is read (an
            // over-long one is refused first, by the item)
            let p = spec_end(fmt, i);
            if byte_at(fmt, p) == b'f' && p - i + 1 < MAX_FORMAT - 10 {
                let m = 110 + 308;
                tell!(out.len() + m, out.len(), |b| b.prep(vm, &out, m)?);
            }
            let before = out.len();
            let item = item54::<PRE>(vm, a, arg, fmt, (i, p), &mut out, &mut added);
            (before, item)
        } else {
            let before = out.len();
            let item = item51::<PRE>(vm, v, a, arg, fmt, i, &mut out, &mut added);
            (before, item)
        };
        i = match item {
            Ok(i) => i,
            Err(e) => return Err(item_failed(vm, out.len(), e)),
        };
        tell!(out.len(), before, |b| {
            let k = out.len() - before;
            match added {
                Added::Value(s) => b.add_value(vm, &out, s)?,
                Added::Sized if !PRE || v != LuaVersion::Lua51 => b.add_size(k),
                // 5.1 copies a sized item with `luaL_addlstring`
                _ => b.add_chars(vm, &out, k)?,
            }
        });
    }
    if out.len() > crate::runtime::string::MAX_LEN {
        return Err(vm.rt_err("string length overflow"));
    }
    let s = if lim != 0 {
        PucBuffer::finish_first::<PRE>(vm, &out)?
    } else {
        finish_begun::<PRE>(vm, b, &out)?
    };
    vm.recycle_buf(out);
    vm.nat_ret1(fs, Value::Str(s))
}

/// Argument `arg` missing: the buffer's slot, `n` bytes so far, goes on
/// the stack before the error, as PUC's is there.
#[cold]
#[inline(never)]
fn no_value(vm: &mut Vm, n: usize, arg: u32) -> LuaError {
    let buf = vm.buffer_slot(n);
    vm.native_push(buf);
    arg_error(vm, arg + 1, "no value")
}

/// An item raised `e`: the buffer's slot, `n` bytes so far, goes on the
/// stack first, as PUC's is there.
#[cold]
#[inline(never)]
fn item_failed(vm: &mut Vm, n: usize, e: LuaError) -> LuaError {
    let buf = vm.buffer_slot(n);
    vm.native_push(buf);
    e
}

/// The result of a format that outgrew the buffer's first block: out of
/// line, as the formats that fit it are the common ones.
#[cold]
#[inline(never)]
fn finish_begun<const PRE: bool>(
    vm: &mut Vm,
    b: &mut Option<PucBuffer>,
    out: &[u8],
) -> Result<crate::runtime::Gc<crate::runtime::LuaStr>, LuaError> {
    b.take().expect("a begun buffer").finish::<PRE>(vm, out)
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
    while fmt
        .get(p)
        .is_some_and(|&c| matches!(c, b'-' | b'+' | b' ' | b'#' | b'0'))
    {
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
