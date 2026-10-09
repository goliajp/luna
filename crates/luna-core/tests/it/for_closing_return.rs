//! A `return` from inside a generic `for` closes the loop's closing value
//! (5.4+), also in a function that captures no local: the function's
//! returns look for something to close.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SRC: &str = r#"
local log = {}
local function iter()
  local closer = setmetatable({}, {__close = function() log[#log + 1] = "closed" end})
  local i = 0
  return function() i = i + 1; if i <= 3 then return i end end, nil, nil, closer
end
local function f()
  for i in iter() do
    if i == 2 then return i end
  end
end
local function g()
  for i in iter() do
    if i == 1 then return end
  end
end
return f(), g(), #log
"#;

#[test]
fn a_return_from_a_generic_for_closes_its_closing_value() {
    for v in [LuaVersion::Lua54, LuaVersion::Lua55] {
        let r = Vm::new(v).eval(SRC).expect("runs");
        assert!(
            matches!(r[..], [Value::Int(2), Value::Nil, Value::Int(2)]),
            "{v:?}: {r:?}"
        );
    }
}
