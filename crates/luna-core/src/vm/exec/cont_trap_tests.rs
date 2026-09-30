//! A continuation frame on top of the stack is found by the dispatch loop
//! only through `Vm::trap`. These chunks leave one on top in every way the
//! interpreter can (pcall / xpcall of Lua and native callees, errors,
//! yieldable metamethods, `__close` handlers, `__pairs`, yields across
//! them); in a debug build the loop head asserts that `trap` was set each
//! time, so a push or pop that stops maintaining it fails here.

use crate::runtime::Value;
use crate::version::LuaVersion;
use crate::vm::exec::Vm;

fn run(version: LuaVersion, src: &str) -> String {
    let mut vm = Vm::new(version);
    let f = vm.load(src.as_bytes(), b"=cont").expect("load");
    let r = vm.call_value(Value::Closure(f), &[]).expect("run");
    match r.first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("not a string: {other:?}"),
    }
}

const PCALL: &str = r#"
local out = {}
local function add(a, b) return a + b end
local ok, v = pcall(add, 1, 2)
out[#out + 1] = tostring(ok) .. v
ok, v = pcall(math.max, 3, 9, 4)
out[#out + 1] = tostring(ok) .. v
ok, v = pcall(error, "e1")
out[#out + 1] = tostring(ok) .. v
ok, v = pcall(function() local t = nil; return t.x end)
out[#out + 1] = tostring(ok)
ok, v = xpcall(function() error("e2") end, function(m) return "h:" .. m end)
out[#out + 1] = tostring(ok) .. v
ok, v = pcall(pcall, add, 4, 5)
out[#out + 1] = tostring(ok) .. tostring(v)
local function tail() return pcall(add, 6, 7) end
ok, v = tail()
out[#out + 1] = tostring(ok) .. v
return table.concat(out, ",")
"#;

const META: &str = r#"
local log = {}
local mt = {}
mt.__index = function(t, k) return k .. "!" end
mt.__newindex = function(t, k, v) rawset(t, k, v * 2) end
mt.__add = function(a, b) return 100 end
mt.__lt = function(a, b) return true end
mt.__le = function(a, b) return false end
mt.__eq = function(a, b) return true end
mt.__concat = function(a, b) return "cat" end
mt.__len = function(a) return 42 end
mt.__call = function(self, x) return x + 1 end
local a = setmetatable({}, mt)
local b = setmetatable({}, mt)
a.y = 5
log[#log + 1] = a.x
log[#log + 1] = rawget(a, "y")
log[#log + 1] = a + 1
log[#log + 1] = tostring(a < b) .. tostring(a <= b) .. tostring(a == b)
log[#log + 1] = a .. "z"
log[#log + 1] = #a
log[#log + 1] = a(9)
local ok, v = pcall(function() return a.q .. (a + 2) end)
log[#log + 1] = tostring(ok) .. v
return table.concat(log, ",")
"#;

const CO: &str = r#"
local log = {}
local mt = {__index = function(t, k) coroutine.yield("in-index"); return k end}
local co = coroutine.wrap(function()
  local ok, v = pcall(function()
    coroutine.yield("in-pcall")
    return "p"
  end)
  log[#log + 1] = tostring(ok) .. v
  local t = setmetatable({}, mt)
  log[#log + 1] = t.w
  local ok2, e = pcall(function() coroutine.yield("before-error"); error("x", 0) end)
  log[#log + 1] = tostring(ok2) .. e
  return "done"
end)
for _ = 1, 4 do
  local r = co()
  log[#log + 1] = r
end
return table.concat(log, ",")
"#;

const CLOSE: &str = r#"
local log = {}
local function closer(name)
  return setmetatable({}, {__close = function() log[#log + 1] = "c" .. name end})
end
do
  local x <close> = closer("1")
  local y <close> = closer("2")
end
local function f()
  local z <close> = closer("3")
  return "r"
end
local r = f()
log[#log + 1] = r
local ok = pcall(function()
  local w <close> = closer("4")
  error("boom")
end)
log[#log + 1] = tostring(ok)
local co = coroutine.wrap(function()
  local v <close> = setmetatable({}, {__close = function() coroutine.yield("y"); log[#log + 1] = "c5" end})
  return "end"
end)
for _ = 1, 2 do
  local r = co()
  log[#log + 1] = r
end
return table.concat(log, ",")
"#;

const PAIRS: &str = r#"
local t = setmetatable({}, {__pairs = function(t)
  return function(_, k) if not k then return 1, "a" elseif k < 2 then return k + 1, "b" end end, t, nil
end})
local out = {}
for k, v in pairs(t) do out[#out + 1] = k .. v end
return table.concat(out, ",")
"#;

#[test]
fn pcall_and_xpcall() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert_eq!(
            run(v, PCALL),
            "true3,true9,falsee1,false,falseh:cont:12: e2,truetrue,true13",
            "{v:?}"
        );
    }
}

#[test]
fn metamethods() {
    for v in [LuaVersion::Lua53, LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(
            run(v, META),
            "x!,10,100,truefalsetrue,cat,42,10,trueq!100",
            "{v:?}"
        );
    }
}

#[test]
fn yields_across_pcall_and_metamethods() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        assert_eq!(
            run(v, CO),
            "in-pcall,truep,w,before-error,falsex,done",
            "{v:?}"
        );
    }
}

#[test]
fn close_handlers() {
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        assert_eq!(run(v, CLOSE), "c2,c1,c3,r,c4,false,y,c5,end", "{v:?}");
    }
}

#[test]
fn pairs_metamethod() {
    for v in [LuaVersion::Lua52, LuaVersion::Lua53, LuaVersion::Lua54] {
        assert_eq!(run(v, PAIRS), "1a,2b", "{v:?}");
    }
}
