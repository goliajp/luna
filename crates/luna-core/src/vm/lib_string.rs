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
use arith_mm::{mm_add, mm_div, mm_idiv, mm_mod, mm_mul, mm_pow, mm_sub, mm_unm};
use patterns::{s_find, s_gmatch, s_gsub, s_match};

type NativeFn = fn(&mut Vm, u32, u32) -> Result<u32, LuaError>;

pub(crate) fn open_string(vm: &mut Vm) {
    let t = vm.heap.new_table();
    let v = vm.version();
    let set = |vm: &mut Vm, t: Gc<crate::runtime::Table>, name: &str, fv: Value| {
        let k = Value::Str(vm.heap.intern(name.as_bytes()));
        // SAFETY: `t` is the table allocated above, so it is alive; no reference into it is held across this call, and `set` does not collect
        unsafe { t.as_mut() }
            .set(&mut vm.heap, k, fv)
            .expect("valid key");
    };
    let mut fns: Vec<(&str, NativeFn)> = vec![
        ("len", s_len),
        ("sub", s_sub),
        ("upper", s_upper),
        ("lower", s_lower),
        ("rep", s_rep),
        ("reverse", s_reverse),
        ("byte", s_byte),
        ("char", s_char),
        ("find", s_find),
        ("match", s_match),
        ("gsub", s_gsub),
        ("format", crate::vm::lib_strformat::s_format),
        ("dump", s_dump),
    ];
    if v >= LuaVersion::Lua53 {
        fns.push(("pack", crate::vm::lib_strpack::s_pack));
        fns.push(("unpack", crate::vm::lib_strpack::s_unpack));
        fns.push(("packsize", crate::vm::lib_strpack::s_packsize));
    }
    for (name, f) in fns {
        let b = inlinable_builtin(name.as_bytes()).unwrap_or(crate::runtime::Builtin::None);
        let fv = vm.builtin(f, &[], b);
        set(vm, t, name, fv);
    }
    // 5.1's LUA_COMPAT_GFIND keeps `gfind` as the very same function as
    // `gmatch`; the suite identity-tests them.
    let gmatch_v = vm.native(s_gmatch);
    set(vm, t, "gmatch", gmatch_v);
    if v == LuaVersion::Lua51 {
        set(vm, t, "gfind", gmatch_v);
    }
    vm.set_global("string", Value::Table(t))
        .expect("stdlib registration");
    vm.barrier_back_table(t);
    let mt = vm.heap.new_table();
    set(vm, mt, "__index", Value::Table(t));
    if v >= LuaVersion::Lua54 {
        let arith: [(&str, NativeFn); 8] = [
            ("__add", mm_add),
            ("__sub", mm_sub),
            ("__mul", mm_mul),
            ("__mod", mm_mod),
            ("__pow", mm_pow),
            ("__div", mm_div),
            ("__idiv", mm_idiv),
            ("__unm", mm_unm),
        ];
        for (name, f) in arith {
            let fv = vm.native(f);
            set(vm, mt, name, fv);
        }
    }
    vm.barrier_back_table(mt);
    vm.set_string_metatable(Some(mt));
}

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

fn s_len(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    Ok(vm.nat_return(fs, &[Value::Int(s.len() as i64)]))
}

fn s_sub(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let s = argcheck::check_string(vm, a, 0)?;
    let i = argcheck::check_integer(vm, a, 1)?;
    let j = argcheck::opt_integer(vm, a, 2, -1)?;
    let r = Value::Str(str_sub(vm, s, i, j));
    Ok(vm.nat_return(fs, &[r]))
}

/// `string.sub(s, i, j)` past its argument checks.
#[doc(hidden)]
pub fn str_sub(vm: &mut Vm, s: Gc<LuaStr>, i: i64, j: i64) -> Gc<LuaStr> {
    let l = s.len();
    let start = posrelat(i, l).max(1);
    let end = posrelat(j, l).min(l as i64);
    let bytes: &[u8] = if start <= end {
        &s.as_bytes()[(start - 1) as usize..end as usize]
    } else {
        b""
    };
    vm.heap.intern(bytes)
}

/// The library function `string.<name>` is, if it is one the trace JIT
/// may run as a direct call on arguments of known types.
#[doc(hidden)]
pub fn inlinable_builtin(name: &[u8]) -> Option<crate::runtime::Builtin> {
    match name {
        b"sub" => Some(crate::runtime::Builtin::StringSub),
        _ => None,
    }
}

fn s_upper(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let out = s.as_bytes().to_ascii_uppercase();
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

fn s_lower(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let out = s.as_bytes().to_ascii_lowercase();
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

fn s_reverse(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let mut out = s.as_bytes().to_vec();
    out.reverse();
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
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
        let r = Value::Str(vm.heap.intern(b""));
        return Ok(vm.nat_return(fs, &[r]));
    }
    let n = n as u128;
    // each dialect's own "too large" test; 5.1 has none
    let too_large = if v == LuaVersion::Lua51 {
        false
    } else if v == LuaVersion::Lua52 {
        l + lsep >= (usize::MAX >> 1) as u128 / n
    } else if v < LuaVersion::Lua55 {
        l + lsep > i32::MAX as u128 / n
    } else {
        l + lsep > i64::MAX as u128 / n
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
    let mut out = Vec::with_capacity(total as usize);
    for k in 0..n {
        out.extend_from_slice(s.as_bytes());
        if let Some(sep) = sep
            && k + 1 < n
        {
            out.extend_from_slice(sep.as_bytes());
        }
    }
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
}

fn s_byte(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let s = argcheck::check_string(vm, a, 0)?;
    let l = s.len();
    let pi = posrelat(argcheck::opt_integer(vm, a, 1, 1)?, l);
    let pose = posrelat(argcheck::opt_integer(vm, a, 2, pi)?, l).min(l as i64);
    let posi = pi.max(1);
    if posi > pose {
        return Ok(0);
    }
    let bytes = &s.as_bytes()[(posi - 1) as usize..pose as usize];
    if let [b] = bytes {
        return Ok(vm.nat_return(fs, &[Value::Int(i64::from(*b))]));
    }
    argcheck::check_stack(vm, a, bytes.len() as i64, "string slice too long")?;
    let vals: Vec<Value> = bytes.iter().map(|&b| Value::Int(i64::from(b))).collect();
    Ok(vm.nat_return(fs, &vals))
}

fn s_char(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let v = vm.version();
    let mut out = Vec::with_capacity(nargs as usize);
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
        out.push(c as u8);
    }
    let r = Value::Str(vm.heap.intern(&out));
    Ok(vm.nat_return(fs, &[r]))
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
        crate::vm::dump::dump_puc(&cl.proto, strip, v)
            .map_err(|_| raise_str(vm, "unable to dump given function"))?
    };
    let r = Value::Str(vm.heap.intern(&bytes));
    Ok(vm.nat_return(fs, &[r]))
}
