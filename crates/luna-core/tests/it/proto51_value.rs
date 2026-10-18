//! The prototype 5.1's parser holds on the stack while `load` calls its
//! reader, as `debug.getlocal` hands it over: a value of type "proto".
//! PUC 5.1 crashes on most operations on it (it is no value the language
//! knows); luna raises the error 5.1 raises for a value of a type the
//! operation does not take, and `tostring` gives "proto: 0x...".

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SCRIPT: &str = r#"
local k, p = 0
load(function()
  k = k + 1
  if k == 2 then p = select(2, debug.getlocal(2, 5)) end
  return ({"local a = 1 ", "return a"})[k]
end)
local out = {type(p), (tostring(p):gsub("0x%x+", "ADDR"))}
for _, f in ipairs {
  function() return p.x end, function() p.x = 1 end, function() return p + 1 end,
  function() return p < p end, function() return p .. "" end, function() return p() end,
  function() return #p end, function() return -p end, function() return getmetatable(p) end,
  function() error(p) end, function() for _ in p do end end, function() return p <= 1 end,
} do
  local ok, e = pcall(f)
  out[#out + 1] = tostring(ok) .. " " .. (type(e) == "string" and e:gsub("^[^:]*:%d+: ", "") or type(e))
end
return table.concat(out, "\n")
"#;

const EXPECTED: &str = "proto
proto: ADDR
false attempt to index upvalue 'p' (a proto value)
false attempt to index upvalue 'p' (a proto value)
false attempt to perform arithmetic on upvalue 'p' (a proto value)
false attempt to compare two proto values
false attempt to concatenate upvalue 'p' (a proto value)
false attempt to call upvalue 'p' (a proto value)
false attempt to get length of upvalue 'p' (a proto value)
false attempt to perform arithmetic on upvalue 'p' (a proto value)
true nil
false proto
false attempt to call a proto value
false attempt to compare proto with number";

#[test]
fn a_parser_prototype_fails_as_a_value_of_its_own_type() {
    let mut vm = Vm::new(LuaVersion::Lua51);
    let got = match vm.eval(SCRIPT).expect("script").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("a listing, got {other:?}"),
    };
    assert_eq!(got, EXPECTED);
}
