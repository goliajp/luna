//! `require` and the loader search behind it.

use super::*;

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
    let Value::Table(searchers) = vm.native_getfield(Value::Table(pkg), field.as_bytes())? else {
        return Err(raise_str(vm, &format!("'package.{field}' must be a table")));
    };
    if v == LuaVersion::Lua51 {
        vm.native_push(1);
    } else {
        vm.native_buffinit(0);
    }
    let mut msg = Vec::new();
    for i in 1.. {
        let s = searchers.get(Value::Int(i));
        if s.is_nil() {
            // 5.1 keeps the nil; 5.2 and 5.3 push the message made from
            // the buffer, which 5.4+ put in the buffer's place
            vm.native_push(u32::from(v <= LuaVersion::Lua53));
            break;
        }
        let r = vm.call_value(s, &[name])?;
        let first = r.first().copied().unwrap_or(Value::Nil);
        let second = r.get(1).copied().unwrap_or(Value::Nil);
        if matches!(first, Value::Closure(_) | Value::Native(_)) {
            return Ok((first, second));
        }
        if let Some(text) = argcheck::to_str_bytes(vm, first) {
            // 5.4 puts the separator between messages itself
            if v >= LuaVersion::Lua54 {
                msg.extend_from_slice(b"\n\t");
            }
            msg.extend_from_slice(&text);
        }
    }
    let Value::Str(n) = name else {
        unreachable!("require passes the name as a string");
    };
    let text = format!(
        "module '{}' not found:{}",
        String::from_utf8_lossy(c_str(n.as_bytes())),
        String::from_utf8_lossy(&msg)
    );
    Err(raise_str(vm, &text))
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
    vm.native_push(1);
    let cur = vm.native_getfield(Value::Table(loaded), name_s.as_bytes())?;
    if cur.truthy() {
        if v == LuaVersion::Lua51 && same(cur, sentinel) {
            let text = format!(
                "loop or previous error loading module '{}'",
                String::from_utf8_lossy(c_str(name_s.as_bytes()))
            );
            return Err(raise_str(vm, &text));
        }
        return Ok(vm.nat_return(fs, &[cur]));
    }
    if v >= LuaVersion::Lua52 {
        vm.native_pop(1);
    }
    let (loader, data) = find_loader(vm, pkg, name)?;
    // the loader is called where it sits: 5.4+ keep the buffer and the
    // loader data below it
    if v >= LuaVersion::Lua54 {
        vm.native_push(1);
    }
    if v == LuaVersion::Lua51 {
        vm.newindex_value(Value::Table(loaded), name, sentinel)?;
    }
    // 5.1 hands the loader only the name
    let args: &[Value] = if v == LuaVersion::Lua51 {
        &[name]
    } else {
        &[name, data]
    };
    let r = vm.call_value(loader, args)?;
    let res = r.first().copied().unwrap_or(Value::Nil);
    // the result is pushed, and `lua_setfield` sets it from there
    vm.native_push(1);
    if !res.is_nil() {
        vm.native_setfield(Value::Table(loaded), name_s.as_bytes(), res)?;
        vm.native_push(1);
    }
    let mut value = vm.index_value(Value::Table(loaded), name)?;
    let unset = if v == LuaVersion::Lua51 {
        same(value, sentinel)
    } else {
        value.is_nil()
    };
    if unset {
        value = Value::Bool(true);
        vm.newindex_value(Value::Table(loaded), name, value)?;
    }
    // 5.4 also returns the loader data
    if v >= LuaVersion::Lua54 {
        return Ok(vm.nat_return(fs, &[value, data]));
    }
    Ok(vm.nat_return(fs, &[value]))
}
