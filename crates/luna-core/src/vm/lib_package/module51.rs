//! 5.1/5.2 `module` and `package.seeall`.

use super::*;

/// `luaL_findtable` on the globals: walk (raw) the dotted `name`, creating
/// missing tables; `None` when a part is a non-table value.
pub(super) fn find_table(vm: &mut Vm, name: &[u8]) -> Result<Option<Gc<Table>>, LuaError> {
    let mut t = vm.globals();
    for part in name.split(|&b| b == b'.') {
        let k = Value::Str(vm.heap.intern(part));
        t = match t.get(k) {
            Value::Table(next) => next,
            Value::Nil => {
                let next = vm.heap.new_table();
                raw_set_checked(vm, t, k, Value::Table(next))?;
                next
            }
            _ => return Ok(None),
        };
    }
    Ok(Some(t))
}

pub(super) fn ll_module(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = vm.version();
    let a = Args::new(fs, nargs);
    let name_s = argcheck::check_string(vm, a, 0)?;
    let name = c_str(name_s.as_bytes()).to_vec();
    let loaded = upval_table(vm, fs, 0);
    let module = match loaded.get(Value::Str(name_s)) {
        Value::Table(t) => t,
        _ => {
            let Some(t) = find_table(vm, &name)? else {
                let text = format!(
                    "name conflict for module '{}'",
                    String::from_utf8_lossy(&name)
                );
                return Err(raise_str(vm, &text));
            };
            raw_set_checked(vm, loaded, Value::Str(name_s), Value::Table(t))?;
            t
        }
    };
    let mv = Value::Table(module);
    let nk = str_value(vm, b"_NAME");
    if vm.index_value(mv, nk)?.is_nil() {
        // modinit: _M, _NAME, and _PACKAGE (the name up to its last dot)
        let mk = str_value(vm, b"_M");
        vm.newindex_value(mv, mk, mv)?;
        let nv = str_value(vm, &name);
        vm.newindex_value(mv, nk, nv)?;
        let cut = name.iter().rposition(|&b| b == b'.').map_or(0, |i| i + 1);
        let pk = str_value(vm, b"_PACKAGE");
        let pv = str_value(vm, &name[..cut]);
        vm.newindex_value(mv, pk, pv)?;
    }
    set_caller_env(vm, module)?;
    // options: 5.1 calls every extra argument, 5.2 only the functions
    for i in 1..nargs {
        let opt = a.get(vm, i);
        if v == LuaVersion::Lua52 && !matches!(opt, Value::Closure(_) | Value::Native(_)) {
            continue;
        }
        vm.call_value(opt, &[mv])?;
    }
    if v == LuaVersion::Lua51 {
        return Ok(vm.nat_return(fs, &[]));
    }
    Ok(vm.nat_return(fs, &[mv]))
}

/// Make `env` (the module table) the environment of the Lua function that
/// called `module`: 5.1's `setfenv`, which gives the caller a new `_ENV`
/// cell (the old one may be shared), 5.2's `lua_setupvalue(f, 1)` on its
/// first upvalue.
pub(super) fn set_caller_env(vm: &mut Vm, env: Gc<Table>) -> Result<(), LuaError> {
    let Some(cl) = lua_caller(vm) else {
        return Err(raise_str(vm, "'module' not called from a Lua function"));
    };
    if vm.version() == LuaVersion::Lua51 {
        if let Some(i) = cl.proto.upvals.iter().position(|d| &*d.name == "_ENV") {
            vm.set_closure_env(cl, i, env);
        }
    } else if !cl.upvals().is_empty() {
        vm.upvalue_set_value(cl, 0, Value::Table(env));
    }
    Ok(())
}

/// The Lua function that called the running native (PUC: level 1 of the
/// stack is a Lua activation), if it was one.
pub(super) fn lua_caller(vm: &Vm) -> Option<Gc<crate::runtime::LuaClosure>> {
    vm.lua_closure_at_level(1)
}

pub(super) fn ll_seeall(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let t = argcheck::check_table(vm, Args::new(fs, nargs), 0)?;
    let mt = match t.metatable() {
        Some(mt) => mt,
        None => {
            let mt = vm.heap.new_table();
            // SAFETY: `t` is the table argument, kept alive by its stack slot; `mt` is a separate new table, and the borrow covers one call
            unsafe { t.as_mut() }.set_metatable(Some(mt));
            mt
        }
    };
    let g = Value::Table(vm.globals());
    let k = Value::Str(vm.heap.intern(b"__index"));
    raw_set_checked(vm, mt, k, g)?;
    Ok(vm.nat_return(fs, &[]))
}
