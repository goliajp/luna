//! PUC's string table counts a place for each short string: one the
//! collector's string sweep has freed and a load then finds again before
//! luna frees the object is PUC's to make anew. Under `gc-verify` the
//! count is checked at every collection; without it a count that went
//! astray grows the table without end.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const SRC: &str = "
    local n = 0
    for i = 1, 300 do
      local names = {}
      for j = 1, 40 do names[j] = 'v' .. (i % 7) .. '_' .. j end
      names = nil
      local src = {}
      for j = 1, 40 do src[j] = 'local v' .. (i % 7) .. '_' .. j .. ' = ' .. j end
      local f = (loadstring or load)(table.concat(src, '\\n'))
      if f then n = n + 1 end
    end
    return n";

/// How many of the loads compiled, with `mode` set first when given.
fn run(v: LuaVersion, mode: Option<&str>) -> Option<i64> {
    let mut vm = Vm::new(v);
    let mode = mode.map_or(String::new(), |m| format!("collectgarbage('{m}')"));
    let src =
        format!("{mode} collectgarbage('setpause', 100) collectgarbage('setstepmul', 400) {SRC}");
    match vm.eval(&src).expect("runs").first() {
        Some(&Value::Int(n)) => Some(n),
        Some(&Value::Float(n)) => Some(n as i64),
        _ => None,
    }
}

#[test]
fn a_load_finds_a_string_the_string_sweep_freed() {
    for (v, mode) in [
        (LuaVersion::Lua51, None),
        (LuaVersion::Lua52, None),
        (LuaVersion::Lua52, Some("generational")),
    ] {
        assert_eq!(run(v, mode), Some(300), "{v:?} {mode:?}");
    }
}
