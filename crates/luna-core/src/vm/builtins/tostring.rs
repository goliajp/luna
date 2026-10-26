//! `tostring`.

use super::*;

pub(super) fn nat_tostring(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let v = argcheck::check_any(vm, Args::new(fs, nargs), 0)?;
    // A number metatable (debug.setmetatable) could carry `__tostring`.
    if let Value::Int(i) = v
        && vm.metatable_of(v).is_none()
    {
        let s = int_text(vm, fs, i)?;
        return vm.nat_ret(fs, &[s]);
    }
    // PUC ≤5.2: `tostring(x)` returns whatever `__tostring` returns — even
    // non-string values like nil. 5.1 hands it back untouched; 5.2 goes
    // through `lua_tolstring`, which renders a number as a string. 5.3+
    // raises "must return a string" (in `tostring_value`).
    if vm.version() <= LuaVersion::Lua52
        && let Some(out) = meta_text_52(vm, v)?
    {
        return vm.nat_ret(fs, &[out]);
    }
    let s = if vm.version() == LuaVersion::Lua51 {
        vm.tostring_51(v, fs, 0)?
    } else {
        vm.tolstring(v, 0)?
    };
    vm.nat_ret(fs, &[Value::Str(s)])
}

/// `tostring` of the integer `i` (the argument at `fs`), with no
/// metatable for numbers. The digits go in a stack buffer, not a `String`.
fn int_text(vm: &mut Vm, fs: u32, i: i64) -> Result<Value, LuaError> {
    let mut buf = [0u8; 20];
    let bytes = crate::numeric::write_i64_dec(i, &mut buf);
    let s = if vm.version() == LuaVersion::Lua53 {
        // `lua_pushfstring(L, "%I")`, made of pieces
        let f = [crate::runtime::puc::fmt::FmtArg::I(i)];
        Value::Str(vm.pushfstring(b"%I", &f)?)
    } else {
        vm.str_value(bytes)?
    };
    // 5.1 and 5.2 convert the number in place (`lua_tolstring`); 5.3
    // on push the string (`%I`, 5.5 `lua_pushstring`) and check then
    if vm.version() >= LuaVersion::Lua53 {
        vm.api_check_pushed(s)?;
    } else {
        vm.api_check_converted(s)?;
    }
    if vm.version() == LuaVersion::Lua51 {
        vm.nat_set_arg(fs, 0, s);
    }
    Ok(s)
}

/// 5.1 and 5.2: what `v`'s `__tostring` returns, when it has one
/// (`luaL_callmeta`, a plain `lua_call`, so not yieldable); 5.2 turns a
/// number into a string.
fn meta_text_52(vm: &mut Vm, v: Value) -> Result<Option<Value>, LuaError> {
    use crate::vm::exec::Mm;
    let mm = vm.get_mm(v, Mm::ToString);
    // `luaL_callmeta(L, 1, "__tostring")` (5.2 makes it in
    // `luaL_tolstring`, which takes the check itself)
    if vm.version() == LuaVersion::Lua51 || !mm.is_nil() {
        vm.metafield_check(v, b"__tostring")?;
    }
    if mm.is_nil() {
        return Ok(None);
    }
    let r = vm.call_value(mm, &[v])?;
    let mut out = r.into_iter().next().unwrap_or(Value::Nil);
    if vm.version() == LuaVersion::Lua52
        && let Some(b) = match out {
            Value::Int(_) | Value::Float(_) => argcheck::to_str_bytes(vm, out),
            _ => None,
        }
    {
        out = vm.str_value(&b)?;
        vm.api_check_converted(out)?;
    }
    Ok(Some(out))
}
