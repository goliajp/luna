//! `type`, `print`, `tostring`, the raw accessors, metatables, `select`
//! and `warn`.

use super::*;

pub(super) fn nat_type(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    let s = Value::Str(vm.heap.intern(v.type_name().as_bytes()));
    Ok(vm.nat_return(fs, &[s]))
}

pub(super) fn nat_print(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    // PUC ≤5.3 `luaB_print` looks the `tostring` global up at call time
    // and calls *that* for each argument — so reassigning `tostring`
    // changes what `print` does (calls.lua 5.3 :29 sets `_ENV.tostring = nil`
    // and expects `print` to fail with "attempt to call a nil value").
    // 5.4 (`luaL_tolstring`) converts in place without consulting the global.
    let global_tostring = if vm.version() <= LuaVersion::Lua53 {
        let g = Value::Table(vm.globals());
        let key = Value::Str(vm.heap.intern(b"tostring"));
        Some(vm.index_value(g, key)?)
    } else {
        None
    };
    let mut out = Vec::new();
    // PUC writes each piece to stdout as soon as it is converted; with C
    // stdio buffering a conversion that runs Lua code can write to stderr
    // or change stdout's buffering in between, so the pieces before it go
    // out first
    let c_stdio = crate::stdio::c_mode();
    for i in 0..nargs {
        let v = vm.nat_arg(fs, nargs, i);
        if c_stdio && !out.is_empty() && may_run_code(vm, v, global_tostring) {
            write_stdout(&out);
            out.clear();
        }
        let piece = match global_tostring {
            // `lua_call` from C: not yieldable.
            Some(ts) => match vm.call_noyield(ts, &[v]) {
                // `lua_tostring` on the result: a number is accepted and
                // rendered, anything else is refused.
                Ok(r) => match r.first().and_then(|&s| argcheck::to_str_bytes(vm, s)) {
                    Some(b) => Ok(b),
                    None => Err(raise_str(vm, "'tostring' must return a string to 'print'")),
                },
                Err(e) => Err(e),
            },
            None => vm.tostring_value(v),
        };
        // PUC writes each argument as soon as it is converted, so the ones
        // before a failing conversion still reach stdout.
        let piece = match piece {
            Ok(b) => b,
            Err(e) => {
                write_stdout(&out);
                return Err(e);
            }
        };
        if i > 0 {
            out.push(b'\t');
        }
        // 5.1 writes each piece with `fputs`, which stops at an embedded NUL;
        // 5.2+ writes the full length.
        let piece = match piece.iter().position(|&c| c == 0) {
            Some(nul) if vm.version() == LuaVersion::Lua51 => &piece[..nul],
            _ => &piece[..],
        };
        out.extend_from_slice(piece);
    }
    out.push(b'\n');
    // 5.2 on end with `lua_writeline`, which flushes stdout
    if c_stdio && vm.version() >= LuaVersion::Lua52 {
        crate::stdio::write_line_flushed(&out);
    } else {
        write_stdout(&out);
    }
    Ok(0)
}

/// Whether converting `v` for `print` can run Lua code: a `__tostring`, or
/// a `tostring` global that is not the library's.
fn may_run_code(vm: &Vm, v: Value, global_tostring: Option<Value>) -> bool {
    let library_tostring = match global_tostring {
        None => true,
        Some(Value::Native(nc)) => {
            std::ptr::fn_addr_eq(nc.f, nat_tostring as crate::runtime::value::NativeFn)
        }
        Some(_) => false,
    };
    !library_tostring || !vm.get_mm(v, crate::vm::exec::Mm::ToString).is_nil()
}

fn write_stdout(bytes: &[u8]) {
    // PUC's `lua_writestring` is an unchecked `fwrite`: a closed or full
    // stdout does not make `print` fail.
    crate::stdio::write_stdout(bytes);
}

pub(super) fn nat_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    // Fast-path Int: avoid the `i.to_string()` String allocation that
    // `tostring_value` would do — stack-buffer it then intern in place.
    // A number metatable (debug.setmetatable) could carry `__tostring`.
    if let Value::Int(i) = v
        && vm.metatable_of(v).is_none()
    {
        let mut buf = [0u8; 20];
        let bytes = crate::numeric::write_i64_dec(i, &mut buf);
        let s = Value::Str(vm.heap.intern(bytes));
        return Ok(vm.nat_return(fs, &[s]));
    }
    // PUC ≤5.2: `tostring(x)` returns whatever `__tostring` returns — even
    // non-string values like nil. 5.1 hands it back untouched; 5.2 goes
    // through `lua_tolstring`, which renders a number as a string. 5.3+
    // raises "must return a string" (in `tostring_value`).
    if vm.version() <= LuaVersion::Lua52 {
        use crate::vm::exec::Mm;
        let mm = vm.get_mm(v, Mm::ToString);
        if !mm.is_nil() {
            // `luaL_callmeta` is a plain `lua_call`: not yieldable.
            let r = vm.call_noyield(mm, &[v])?;
            let mut out = r.into_iter().next().unwrap_or(Value::Nil);
            if vm.version() == LuaVersion::Lua52
                && let Some(b) = match out {
                    Value::Int(_) | Value::Float(_) => argcheck::to_str_bytes(vm, out),
                    _ => None,
                }
            {
                out = Value::Str(vm.heap.intern(&b));
            }
            return Ok(vm.nat_return(fs, &[out]));
        }
    }
    let bytes = vm.tostring_value(v)?;
    let s = Value::Str(vm.heap.intern(&bytes));
    Ok(vm.nat_return(fs, &[s]))
}

pub(super) fn nat_rawget(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let k = argcheck::check_any(vm, a, 1)?;
    let v = t.get(k);
    Ok(vm.nat_return(fs, &[v]))
}

pub(super) fn nat_rawset(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let k = argcheck::check_any(vm, a, 1)?;
    let v = argcheck::check_any(vm, a, 2)?;
    // a bad key or a read-only table is the VM's error (`luaH_set` /
    // `lua_rawset` → `luaG_runerror`), raised while rawset runs, so it
    // carries no position
    vm.raw_set(t, k, v)?;
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

pub(super) fn nat_rawequal(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let x = argcheck::check_any(vm, a, 0)?;
    let y = argcheck::check_any(vm, a, 1)?;
    Ok(vm.nat_return(fs, &[Value::Bool(x.raw_eq(y))]))
}

pub(super) fn nat_rawlen(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let n = match a.get(vm, 0) {
        Value::Table(t) => t.len(),
        Value::Str(s) => s.len() as i64,
        _ => return Err(argcheck::arg_expected(vm, a, 0, "table or string")),
    };
    Ok(vm.nat_return(fs, &[Value::Int(n)]))
}

pub(super) fn nat_setmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::exec::Mm;
    let a = Args::new(fs, nargs);
    let t = argcheck::check_table(vm, a, 0)?;
    let mt = match a.get(vm, 1) {
        _ if a.is_none(1) => return Err(argcheck::arg_expected(vm, a, 1, "nil or table")),
        Value::Nil => None,
        Value::Table(m) => Some(m),
        _ => return Err(argcheck::arg_expected(vm, a, 1, "nil or table")),
    };
    if !vm.get_mm(Value::Table(t), Mm::Metatable).is_nil() {
        return Err(raise_str(vm, "cannot change a protected metatable"));
    }
    // Redis's `lua_setmetatable` refuses a read-only table
    vm.refuse_readonly(t)?;
    // SAFETY: `t` is the table argument, kept alive by its stack slot; the borrow covers one call, and `mt` is a separate handle
    unsafe { t.as_mut() }.set_metatable(mt);
    // setmetatable links a long-lived table to a long-lived mt; barrier_back
    // so the new mt gets traced even if t was already black.
    vm.barrier_back_table(t);
    // register for finalization if the new metatable carries `__gc` (PUC marks
    // the object finalizable at setmetatable time)
    vm.check_finalizer(t);
    Ok(vm.nat_return(fs, &[Value::Table(t)]))
}

pub(super) fn nat_getmetatable(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    use crate::vm::exec::Mm;
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    // __metatable protection: return that field instead
    let protected = vm.get_mm(v, Mm::Metatable);
    if !protected.is_nil() {
        return Ok(vm.nat_return(fs, &[protected]));
    }
    let mt = vm.metatable_of(v).map(Value::Table).unwrap_or(Value::Nil);
    Ok(vm.nat_return(fs, &[mt]))
}

pub(super) fn nat_select(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let n = match a.get(vm, 0) {
        Value::Int(i) if vm.version() >= LuaVersion::Lua53 => i,
        // PUC tests only the first character: `select("#x", ...)` counts too.
        Value::Str(s) if s.as_bytes().first() == Some(&b'#') => {
            return Ok(vm.nat_return(fs, &[Value::Int(nargs as i64 - 1)]));
        }
        // ≤5.2 reads the index with `luaL_checkint`, a C int.
        _ if vm.version() <= LuaVersion::Lua52 => argcheck::check_int(vm, a, 0)? as i64,
        _ => argcheck::check_integer(vm, a, 0)?,
    };
    let top = nargs as i64;
    let i = if n < 0 {
        top + n
    } else if n > top {
        top
    } else {
        n
    };
    if i < 1 {
        return Err(arg_error(vm, 1, "index out of range"));
    }
    let vals: Vec<Value> = (i..top).map(|k| vm.nat_arg(fs, nargs, k as u32)).collect();
    Ok(vm.nat_return(fs, &vals))
}

/// PUC 5.4+ `warn(msg1, ..., msgN)` — every argument must be a string (or a
/// number, which converts). The first N-1 pieces are emitted with
/// `to_cont = true`, the last with `to_cont = false`, so the default warnf
/// concatenates the parts and flushes the line at the tail call (mirrors
/// `lbaselib.c::luaB_warn`). All arguments are checked before any is
/// emitted, so a bad one leaves no half-composed warning.
pub(crate) fn nat_warn(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let a = Args::new(fs, nargs);
    let mut parts = Vec::with_capacity(nargs.max(1) as usize);
    for i in 0..nargs.max(1) {
        parts.push(argcheck::check_string(vm, a, i)?);
    }
    let n = parts.len();
    for (i, p) in parts.iter().enumerate() {
        vm.emit_warn(p.as_bytes(), i + 1 < n)?;
    }
    Ok(vm.nat_return(fs, &[]))
}
