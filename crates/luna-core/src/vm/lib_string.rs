//! string library: byte-string functions, the pattern-based family
//! (find/match/gmatch/gsub) on top of src/pattern.rs, and the shared string
//! metatable — method syntax on every dialect, arithmetic from 5.4.

use crate::runtime::{Gc, LuaStr, Value};
use crate::version::LuaVersion;
use crate::vm::argcheck::{self, Args};
use crate::vm::builtins::{arg_error, raise_str};
use crate::vm::error::LuaError;
use crate::vm::exec::Vm;

mod arith_mm;
mod patterns;
mod sub;
use arith_mm::{mm_add, mm_div, mm_idiv, mm_mod, mm_mul, mm_pow, mm_sub, mm_unm};
use patterns::{s_find, s_gmatch, s_gsub, s_match};
pub(crate) use sub::*;
mod puc_fns;
pub(crate) use puc_fns::PUC_FNS;

/// PUC ≤5.3 `posrelat`: a negative position counts from the end, and one
/// before the start becomes 0. Callers clamp; 5.4's `posrelatI` /
/// `getendpos` come to the same after clamping.
fn posrelat(pos: i64, len: usize) -> i64 {
    if pos >= 0 {
        pos
    } else if pos.unsigned_abs() > len as u64 {
        0
    } else {
        len as i64 + pos + 1
    }
}

/// 5.4's `posrelatI` makes a positive start position a `size_t`, which
/// wraps on a 32-bit target.
#[inline(always)]
fn start_pos(v: LuaVersion, pos: i64) -> i64 {
    #[cfg(target_pointer_width = "32")]
    if v >= LuaVersion::Lua54 && pos > 0 {
        return i64::from(pos as u32);
    }
    let _ = v;
    pos
}

fn s_len(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    vm.nat_ret(fs, &[Value::Int(s.len() as i64)])
}

fn s_upper(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let mut out = vm.new_buf();
    vm.buf_push(&mut out, s.as_bytes())?;
    out.make_ascii_uppercase();
    let r = vm.buffer_result(&out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[r])
}

fn s_lower(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let mut out = vm.new_buf();
    vm.buf_push(&mut out, s.as_bytes())?;
    out.make_ascii_lowercase();
    let r = vm.buffer_result(&out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[r])
}

fn s_reverse(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let mut out = vm.new_buf();
    vm.buf_push(&mut out, s.as_bytes())?;
    out.reverse();
    let r = vm.buffer_result(&out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[r])
}

/// The longest string the library builds (1 GiB). Past each dialect's own
/// size check PUC goes on to ask the allocator; luna stops here and reports
/// what a failing allocator would.
pub(crate) const MAX_STR: u64 = 1 << 30;

fn s_rep(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let s = argcheck::check_string(vm, a, 0)?;
    let n = if v <= LuaVersion::Lua52 {
        i64::from(argcheck::check_int(vm, a, 1)?)
    } else {
        argcheck::check_integer(vm, a, 1)?
    };
    // 5.1 has no separator argument
    let sep = if v == LuaVersion::Lua51 {
        None
    } else {
        argcheck::opt_string(vm, a, 2)?
    };
    let (l, lsep) = (s.len() as u128, sep.map_or(0, |x| x.len()) as u128);
    // the result is empty for any count when both pieces are; PUC before
    // 5.5 spins `n` times producing it
    if n <= 0 || l + lsep == 0 {
        // 5.1 builds it in a buffer, whose empty result checks for no
        // collection (`lua_concat` of nothing); later ones push a literal
        let r = if v == LuaVersion::Lua51 {
            vm.buffer_result(&[])?
        } else {
            vm.api_push_string(b"")?
        };
        return vm.nat_ret(fs, &[r]);
    }
    let n = n as u128;
    let total = rep_size(vm, n, l, lsep)?;
    let mut out = vm.new_buf();
    // PUC asks for the whole size at once (`luaL_buffinitsize`); from 5.3
    // on a box is made first, which a refused block leaves behind
    if v >= LuaVersion::Lua53 && out.reserve(total as usize).is_err() {
        if total as usize > Vm::lual_buffersize(v) {
            // 5.4 on, the protected call that catches the error closes it
            let slot = vm.native_top();
            vm.new_box(slot)?;
        }
        return Err(vm.buf_error());
    }
    vm.buf_reserve(&mut out, total as usize)?;
    for k in 0..n {
        vm.buf_push(&mut out, s.as_bytes())?;
        if let Some(sep) = sep
            && k + 1 < n
        {
            vm.buf_push(&mut out, sep.as_bytes())?;
        }
    }
    let r = vm.buffer_result(&out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[r])
}

/// The size of `n` copies of `l` bytes with `lsep` between them, under
/// each dialect's "too large" test (5.1 has none) and luna's limit.
fn rep_size(vm: &mut Vm, n: u128, l: u128, lsep: u128) -> Result<u128, LuaError> {
    let v = vm.version();
    let too_large = if v == LuaVersion::Lua51 {
        false
    } else if v == LuaVersion::Lua52 {
        l + lsep >= (usize::MAX >> 1) as u128 / n
    } else if v < LuaVersion::Lua55 {
        l + lsep > i32::MAX as u128 / n
    } else {
        l + lsep > u128::from(crate::vm::c_abi::MAX_SIZE_55) / n
    };
    if too_large {
        return Err(raise_str(vm, "resulting string too large"));
    }
    let total = n * (l + lsep) - lsep;
    if total > u128::from(MAX_STR) {
        // 5.3's buffer reports a refused allocation as an ordinary error
        if v == LuaVersion::Lua53 {
            return Err(raise_str(vm, "not enough memory for buffer allocation"));
        }
        return Err(vm.mem_err());
    }
    Ok(total)
}

fn s_byte(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let s = argcheck::check_string(vm, a, 0)?;
    let l = s.len();
    let pi = posrelat(
        start_pos(vm.version(), argcheck::opt_integer(vm, a, 1, 1)?),
        l,
    );
    let pose = posrelat(argcheck::opt_integer(vm, a, 2, pi)?, l).min(l as i64);
    let posi = pi.max(1);
    if posi > pose {
        return Ok(0);
    }
    let bytes = &s.as_bytes()[(posi - 1) as usize..pose as usize];
    if let [b] = bytes {
        return vm.nat_ret(fs, &[Value::Int(i64::from(*b))]);
    }
    argcheck::check_stack(vm, a, bytes.len() as i64, "string slice too long")?;
    let n = bytes.len();
    vm.nat_ret_iter(fs, n, bytes.iter().map(|&b| Value::Int(i64::from(b))))
}

fn s_char(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let mut out = vm.new_buf();
    vm.buf_reserve(&mut out, nargs as usize)?;
    for i in 0..nargs {
        let c = match if v <= LuaVersion::Lua52 {
            argcheck::check_int(vm, a, i).map(i64::from)
        } else {
            argcheck::check_integer(vm, a, i)
        } {
            Ok(c) => c,
            Err(e) => {
                // `luaL_buffinitsize` sized the buffer to the arguments
                let buf = vm.buffer_slot(nargs as usize);
                vm.native_push(buf);
                return Err(e);
            }
        };
        if !(0..=255).contains(&c) {
            let msg = if v == LuaVersion::Lua51 {
                "invalid value"
            } else {
                "value out of range"
            };
            let buf = vm.buffer_slot(nargs as usize);
            vm.native_push(buf);
            return Err(arg_error(vm, i + 1, msg));
        }
        vm.buf_push(&mut out, &[c as u8])?;
    }
    let r = vm.buffer_result(&out)?;
    vm.recycle_buf(out);
    vm.nat_ret(fs, &[r])
}

fn s_dump(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    // the strip flag arrived in 5.3
    let strip = v >= LuaVersion::Lua53 && a.get(vm, 1).truthy();
    let cl = if v >= LuaVersion::Lua55 {
        match a.get(vm, 0) {
            Value::Closure(cl) => cl,
            _ => return Err(arg_error(vm, 1, "Lua function expected")),
        }
    } else {
        match argcheck::check_function(vm, a, 0)? {
            Value::Closure(cl) => cl,
            _ => return Err(raise_str(vm, "unable to dump given function")),
        }
    };
    // PUC bytecode of the running dialect; MacroLua, which has none, keeps
    // luna's own format
    let bytes = if v.is_macro_lua() {
        crate::vm::dump::dump(&cl.proto, strip, v)
    } else {
        let (bytes, _, saved) = crate::vm::dump::dump_puc(&cl.proto, strip, v)
            .map_err(|_| raise_str(vm, "unable to dump given function"))?;
        // 5.5's `luaU_dump` anchors its table of saved strings on the stack
        if v >= LuaVersion::Lua55 {
            let t = vm.dump_strings_table(saved)?;
            vm.native_push_vals(&[Value::Table(t)])?;
        }
        bytes
    };
    let r = vm.api_push_string(&bytes)?;
    vm.nat_ret(fs, &[r])
}
