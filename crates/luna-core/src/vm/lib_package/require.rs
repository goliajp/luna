//! `require` and the loader search behind it.

use super::*;
use crate::runtime::puc_fmt::FmtArg;
use crate::vm::puc_buffer::PucBuffer;

/// `findloader`: ask each searcher in turn; the first one to return a
/// function supplies the loader (and its extra value). The misses are
/// collected into the "not found" message.
pub(super) fn find_loader(
    vm: &mut Vm,
    pkg: Gc<Table>,
    name: Value,
) -> Result<(Value, Value), LuaError> {
    let v = vm.version();
    let field = if v == LuaVersion::Lua51 {
        "loaders"
    } else {
        "searchers"
    };
    // the searchers, the message (a string in 5.1, a buffer in 5.4+) and
    // the searcher being tried are pushed; each searcher is called where
    // it is pushed
    let Value::Table(searchers) = vm.native_getfield(Value::Table(pkg), None, field.as_bytes())?
    else {
        // a literal: the name is part of the format
        return Err(crate::vm::builtins::raise_fmt(
            vm,
            0,
            &format!("'package.{field}' must be a table"),
        ));
    };
    // 5.1's message is a string on the stack, which each miss replaces
    // with the two concatenated (`lua_concat`)
    let mut acc = Value::Nil;
    if v == LuaVersion::Lua51 {
        acc = vm.str_value(b"")?;
        vm.native_push_vals(&[acc])?;
    } else {
        vm.native_buffinit(0)?;
    }
    let mut msg = Vec::new();
    // 5.2 on: the message in a `luaL_Buffer`
    let mut buf = (v >= LuaVersion::Lua52).then(|| PucBuffer::new(vm));
    // 5.4+'s message, in the buffer's place while the error is made
    let mut made = None;
    for i in 1.. {
        // 5.4 on adds the separator first, taken off again when nothing
        // follows it
        if v >= LuaVersion::Lua54
            && let Some(b) = buf.as_mut()
        {
            msg.extend_from_slice(b"\n\t");
            b.add(vm, &msg, 2)?;
        }
        let s = searchers.get(Value::Int(i));
        if s.is_nil() {
            // 5.1 keeps the nil; later the message made from the buffer
            // is pushed (5.4+ in the buffer's place)
            match buf.take() {
                None => vm.native_push_vals(&[Value::Nil])?,
                Some(mut b) => {
                    if v >= LuaVersion::Lua54 {
                        msg.truncate(msg.len() - 2);
                        b.sub(2);
                    }
                    let m = b.finish(vm, &msg)?;
                    if v <= LuaVersion::Lua53 {
                        vm.native_push_vals(&[Value::Str(m)])?;
                    } else {
                        made = Some(m);
                    }
                }
            }
            break;
        }
        // `lua_pushstring(L, name)` for the call: a long name is the string
        // `luaS_new`'s cache has for the name's characters
        let Value::Str(n) = name else {
            unreachable!("require passes the name as a string");
        };
        let arg = vm.api_push_string_at(name_at(n), n.as_bytes())?;
        let r = vm.call_value(s, &[arg])?;
        let first = r.first().copied().unwrap_or(Value::Nil);
        let second = r.get(1).copied().unwrap_or(Value::Nil);
        if matches!(first, Value::Closure(_) | Value::Native(_)) {
            return Ok((first, second));
        }
        // the searcher's result is on PUC's stack while the message grows
        let held = vm.lib_roots.len();
        vm.lib_roots.push(first);
        let r = add_message(vm, first, &mut msg, buf.as_mut(), &mut acc);
        vm.lib_roots.truncate(held);
        r?;
    }
    let Value::Str(n) = name else {
        unreachable!("require passes the name as a string");
    };
    // `luaL_error(L, "module '%s' not found:%s", name, msg)`
    let at = vm.lib_roots.len();
    vm.lib_roots.extend(made.map(Value::Str));
    let args = [FmtArg::S(c_str(n.as_bytes())), FmtArg::S(&msg)];
    let e = crate::vm::builtins::raise_lib(vm, b"module '%s' not found:%s", &args, 2);
    vm.lib_roots.truncate(at);
    Err(e)
}

/// A searcher's message `first` (rooted by the caller) added to the
/// message: to the buffer (5.2 on), or concatenated to `acc` on the
/// stack (5.1).
fn add_message(
    vm: &mut Vm,
    first: Value,
    msg: &mut Vec<u8>,
    buf: Option<&mut PucBuffer>,
    acc: &mut Value,
) -> Result<(), LuaError> {
    let v = vm.version();
    if let Some(text) = argcheck::to_str_bytes(vm, first) {
        msg.extend_from_slice(&text);
        if let Some(b) = buf {
            // `luaL_addvalue`, which turns a number into a string first
            let s = match first {
                Value::Str(s) => s,
                _ => {
                    let s = vm.alloc_str(&text)?;
                    // converted in its stack slot: the caller drops it
                    vm.lib_roots.push(Value::Str(s));
                    vm.api_check_converted(Value::Str(s))?;
                    s
                }
            };
            b.add_value(vm, msg, s)?;
        }
        if v == LuaVersion::Lua51 {
            // `lua_concat(L, 2)` of the message so far and this one
            vm.api_check_before()?;
            let m = crate::vm::builtins::concat2(vm, *acc, first)?;
            *acc = m;
            let top = vm.running_natives.last().map_or(0, |a| a.top_off);
            vm.native_hold(top - 1, &[m])?;
        }
    } else if v >= LuaVersion::Lua54
        && let Some(b) = buf
    {
        msg.truncate(msg.len() - 2);
        b.sub(2);
    }
    Ok(())
}

pub(super) fn ll_require(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let name_s = argcheck::check_string(vm, Args::new(fs, nargs), 0)?;
    let name = Value::Str(name_s);
    let pkg = upval_table(vm, fs, 0);
    let loaded = upval_table(vm, fs, 1);
    let sentinel = vm.nat_upval(fs, 2);
    // `lua_settop(L, 1)`, the loaded table, then `LOADED[name]`, which 5.2+
    // pop again before searching
    vm.native_settop(1);
    vm.native_push_vals(&[Value::Table(loaded)])?;
    let cur = vm.native_getfield(
        Value::Table(loaded),
        Some(name_at(name_s)),
        name_s.as_bytes(),
    )?;
    if cur.truthy() {
        if v == LuaVersion::Lua51 && same(cur, sentinel) {
            let args = [FmtArg::S(c_str(name_s.as_bytes()))];
            let fmt = b"loop or previous error loading module '%s'";
            return Err(crate::vm::builtins::raise_lib(vm, fmt, &args, 1));
        }
        return vm.nat_ret(fs, &[cur]);
    }
    if v >= LuaVersion::Lua52 {
        vm.native_pop(1);
    }
    let (loader, data) = find_loader(vm, pkg, name)?;
    // the loader is called where it sits: 5.4+ keep the buffer and the
    // loader data below it
    if v >= LuaVersion::Lua54 {
        vm.native_push_vals(&[data])?;
    }
    if v == LuaVersion::Lua51 {
        vm.newindex_value(Value::Table(loaded), name, sentinel)?;
    }
    // ≤5.3 push the name again for the loader (`lua_pushstring`), 5.4 on
    // pass the argument itself; 5.1 hands the loader only the name
    let arg = if v <= LuaVersion::Lua53 {
        vm.lib_roots.extend([loader, data]);
        let pushed = vm.api_push_string_at(name_at(name_s), name_s.as_bytes());
        vm.lib_roots.truncate(vm.lib_roots.len() - 2);
        pushed?
    } else {
        name
    };
    let args: &[Value] = if v == LuaVersion::Lua51 {
        &[arg]
    } else {
        &[arg, data]
    };
    let r = vm.call_value(loader, args)?;
    let res = r.first().copied().unwrap_or(Value::Nil);
    // the result is pushed, and `lua_setfield` sets it from there
    vm.native_push(1);
    if !res.is_nil() {
        vm.native_setfield(
            Value::Table(loaded),
            Some(name_at(name_s)),
            name_s.as_bytes(),
            res,
        )?;
        vm.native_push(1);
    }
    // `lua_getfield(L, 2, name)` and `lua_setfield(L, 2, name)`: keys
    // `luaS_new` makes of the name
    let key = vm.field_key(Some(name_at(name_s)), name_s.as_bytes())?;
    vm.lib_roots.push(key);
    let value = vm.index_value(Value::Table(loaded), key);
    vm.lib_roots.pop();
    let mut value = value?;
    let unset = if v == LuaVersion::Lua51 {
        same(value, sentinel)
    } else {
        value.is_nil()
    };
    if unset {
        value = Value::Bool(true);
        let key = vm.field_key(Some(name_at(name_s)), name_s.as_bytes())?;
        vm.lib_roots.push(key);
        let set = vm.newindex_value(Value::Table(loaded), key, value);
        vm.lib_roots.pop();
        set?;
    }
    // 5.4 also returns the loader data
    if v >= LuaVersion::Lua54 {
        return vm.nat_ret(fs, &[value, data]);
    }
    vm.nat_ret(fs, &[value])
}

/// Where `luaS_new` finds the characters of `name`, a string argument
/// (`luaL_checkstring`): in the string.
pub(super) fn name_at(
    name: crate::runtime::Gc<crate::runtime::LuaStr>,
) -> crate::runtime::heap::strcache::CStr {
    crate::runtime::heap::strcache::CStr::In(name.as_ptr() as *const _, 0)
}
