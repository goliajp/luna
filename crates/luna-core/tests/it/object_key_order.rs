//! Tables, functions, coroutines and userdata as table keys hash by the
//! number each got when it was made, so a program walks such a table in
//! the same order on every run: ten states running one chunk in one
//! process (where the objects sit at different addresses) agree. They
//! share a string hashing seed: with a seed of its own each state walks
//! the tables of string keys the collector marks in its own order, and
//! 5.2 and 5.3 keep a cached closure or make another as that order says.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const CHUNK: &str = r#"
local keys, t = {}, {}
for i = 1, 40 do
  local k
  if i % 4 == 0 then k = {}
  elseif i % 4 == 1 then k = function() return i end
  elseif i % 4 == 2 then k = coroutine.create(function() end)
  else k = io.tmpfile and print or {} end
  if t[k] == nil then keys[#keys + 1] = k; t[k] = i end
end
-- dead keys: half removed, then the table grows past them
for i = 1, #keys, 2 do t[keys[i]] = nil end
for i = 1, 20 do t[{}] = -i end
local out = {}
for k, v in pairs(t) do out[#out + 1] = type(k) .. ":" .. v end
return table.concat(out, " ")
"#;

fn walk(v: LuaVersion) -> String {
    let mut vm = Vm::new_minimal_with_hash_seed(v, 1);
    vm.open_all_libs().expect("the libraries open");
    match vm.eval(CHUNK).expect("the chunk runs").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn object_keys_walk_in_the_same_order_on_every_run() {
    for v in [
        LuaVersion::Lua51,
        LuaVersion::Lua52,
        LuaVersion::Lua53,
        LuaVersion::Lua54,
        LuaVersion::Lua55,
    ] {
        let first = walk(v);
        // keep earlier states alive so later ones get other addresses
        let mut held = Vec::new();
        for _ in 0..9 {
            held.push(Vm::new(v));
            assert_eq!(walk(v), first, "{v:?}");
        }
    }
}
