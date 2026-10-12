//! A native that calls Lua through `Vm::call_value`, which has no
//! continuation, run in a coroutine that yields below it: the yield is
//! refused where it starts, with PUC's message, whether the native passes
//! the error on, catches it, or calls the library `pcall` with it. The
//! expected lines are what a C host doing the same with `lua_call` /
//! `lua_pcall` prints under each PUC release.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::LuaError;
use luna_core::vm::exec::Vm;

/// `h(f)`: call `f` and return its results, passing an error on
fn h(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    let r = vm.call_value(f, &[])?;
    Ok(vm.nat_return(fs, &r))
}

/// `hs(f)`: call `f` and return whether it succeeded and its first result
/// or error, as a native that catches every error does
fn hs(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    let out = match vm.call_value(f, &[]) {
        Ok(r) => [Value::Bool(true), r.first().copied().unwrap_or(Value::Nil)],
        Err(e) => [Value::Bool(false), e.0],
    };
    Ok(vm.nat_return(fs, &out))
}

/// `hp(f)`: the library `pcall` called with `f` through `call_value`
fn hp(vm: &mut Vm, fs: u32, nargs: u32) -> Result<u32, LuaError> {
    let f = vm.nat_arg(fs, nargs, 0);
    let key = Value::Str(vm.heap.intern(b"pcall"));
    let pcall = vm.globals().get(key);
    let r = vm.call_value(pcall, &[f])?;
    Ok(vm.nat_return(fs, &r))
}

const SCRIPT: &str = r#"
local out = {}
for _, name in ipairs{'h', 'hs', 'hp'} do
  local f = _G[name]
  local co = coroutine.create(function()
    local a, b = f(function() coroutine.yield('y'); return 'r' end)
    return 'after ' .. tostring(a) .. ' ' .. tostring(b)
  end)
  local ok, v = coroutine.resume(co)
  out[#out + 1] = name .. ': ' .. tostring(ok) .. ' ' .. tostring(v)
end
return table.concat(out, '\n')
"#;

fn run(version: LuaVersion) -> String {
    let mut vm = Vm::new(version);
    for (name, f) in [
        ("h", h as luna_core::runtime::value::NativeFn),
        ("hs", hs),
        ("hp", hp),
    ] {
        let v = vm.native(f);
        vm.set_global(name, v).unwrap();
    }
    let chunk = vm.load(SCRIPT.as_bytes(), b"=host").expect("load");
    match vm
        .call_value(Value::Closure(chunk), &[])
        .expect("run")
        .first()
    {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("not a string: {other:?}"),
    }
}

#[test]
fn yield_below_a_call_without_continuation_is_refused() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let msg = if v == LuaVersion::Lua51 {
            "attempt to yield across metamethod/C-call boundary"
        } else {
            "attempt to yield across a C-call boundary"
        };
        let want =
            format!("h: false {msg}\nhs: true after false {msg}\nhp: true after false {msg}");
        assert_eq!(run(v), want, "{v:?}");
    }
}
