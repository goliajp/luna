//! `string.rep` builds its result by doubling the copies made so far: the
//! bytes must be those of the copies and separators laid end to end, for
//! every length the doubling can stop at.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const CHECK: &str = r#"
local seps = {false, "", ",", "<sep>"}
local strs = {"", "a", "ab", "hello world", string.rep("z", 70)}
local bad = 0
for _, s in ipairs(strs) do
  for _, sep in ipairs(seps) do
    for n = 0, 70 do
      local want = {}
      for i = 1, n do want[i] = s end
      local w = table.concat(want, sep or "")
      local got = sep and string.rep(s, n, sep) or string.rep(s, n)
      if got ~= w then bad = bad + 1 end
    end
  end
end
return bad
"#;

#[test]
fn rep_lays_copies_and_separators_end_to_end() {
    for v in [
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let mut vm = Vm::new(v);
        let cl = vm.load(CHECK.as_bytes(), b"=rep").expect("load");
        let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
        let ok = matches!(r.first(), Some(Value::Int(0)))
            || matches!(r.first(), Some(Value::Float(x)) if *x == 0.0);
        assert!(ok, "{v:?}: {:?}", r.first());
    }
}

#[test]
fn rep_51_ignores_a_separator() {
    let mut vm = Vm::new(LuaVersion::Lua51);
    let src = b"return string.rep('ab', 37, ','), string.rep('x', 1)";
    let cl = vm.load(src, b"=rep51").expect("load");
    let r = vm.call_value(Value::Closure(cl), &[]).expect("run");
    let Some(Value::Str(s)) = r.first() else {
        panic!("{:?}", r.first())
    };
    assert_eq!(s.as_bytes(), "ab".repeat(37).as_bytes());
}
