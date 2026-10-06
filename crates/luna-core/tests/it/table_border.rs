//! `#t` on tables with holes returns the border PUC returns, dialect by
//! dialect: the expected lines are what PUC 5.1.5, 5.2.4, 5.3.6, 5.4.9 and
//! 5.5.1 print for the same chunk (`diff_puc`'s `1007_table_border` fixtures
//! start with it). The loops run long enough for the JIT crates' tests of
//! the corpus to compile them.

use luna_core::runtime::Value;
use luna_core::version::LuaVersion;
use luna_core::vm::Vm;

const CHUNK: &str = r#"
local function r0() end
local function r1(x) return x end
local function r2(x) return x, x end
local function pk(...) return {...} end
local out = {}
local function show(name, t) out[#out + 1] = name .. " " .. #t end
show("calls", {r1(6), r1(7), r0(), r1(8)})
show("nil middle", {1, nil, 3})
show("nil lead", {nil, nil, 3})
show("nil tail", {1, 2, nil})
show("hole", {1, 2, nil, 4})
show("holes", {1, nil, nil, nil, 5, 6, nil, 8, nil})
show("vararg", pk(1, nil, 3, nil, 5))
show("call cut", {r2(1), x = 1})
show("call open", {r0(), r2(1)})
show("keyed", {[1] = 1, [3] = 3})
show("keyed after list", {1, 2, [4] = 4, [3] = nil})

local t = {}
for i = 1, 10 do t[i] = i end
t[5] = nil; show("fill 10, drop 5", t)
t[10] = nil; show("drop 10", t)
t[9] = nil; t[8] = nil; show("drop 9 8", t)
t[6] = nil; show("drop 6", t)
t[20] = 1; show("hash 20", t)

t = {}
for i = 1, 17 do t[i] = i end
for i = 2, 16, 2 do t[i] = nil end
show("odd of 17", t)
t[17] = nil; show("odd of 16", t)

t = {}
t[1] = 1; t[3] = 3; show("1 3", t)
t[4] = 4; show("1 3 4", t)
t[2] = 2; show("1 2 3 4", t)
t[6] = 6; t[8] = 8; show("1 2 3 4 6 8", t)
t[4] = nil; show("drop 4", t)

t = {1, 2, 3, 4, 5, 6, 7, 8}
table.remove(t); table.remove(t); show("remove twice", t)
t[3] = nil; show("hole 3", t)
local _ = t[6]; show("read 6", t)
table.insert(t, 9); show("insert", t)
table.insert(t, 1, 0); show("insert front", t)
t[8] = nil; t[9] = nil; show("drop 8 9", t)
for _ in ipairs(t) do end; show("ipairs", t)

t = {}
for i = 1, 33 do t[i] = i end
t[33] = nil; t[20] = nil; show("33 minus 33 20", t)
t[17] = nil; show("minus 17", t)
t[40] = 40; t[41] = 41; show("hash 40 41", t)

-- the same in loops, so that the trace and method JITs run them
local function churn(n)
  local t, acc = {}, {}
  for i = 1, n do
    t[i] = i
    if i % 3 == 0 then t[i - 1] = nil end
    if i % 5 == 0 then local _ = t[i + 2] end
    acc[#acc + 1] = #t
  end
  return table.concat(acc, " ")
end
out[#out + 1] = "churn " .. churn(70)

local function ctor(n)
  local acc = {}
  for i = 1, n do
    local u = {i, nil, i, r0()}
    local v = {r1(i), r0(), r1(i)}
    local w = pk(i, nil, i, nil)
    acc[#acc + 1] = #u .. "/" .. #v .. "/" .. #w
  end
  return table.concat(acc, " ")
end
out[#out + 1] = "ctor " .. ctor(12)

local function fill(n)
  local t = {}
  for i = 1, n do t[i] = i end
  local acc = {}
  for i = n, 1, -3 do
    t[i] = nil
    acc[#acc + 1] = #t
  end
  for i = 1, n, 4 do
    table.insert(t, i)
    acc[#acc + 1] = #t
    table.remove(t)
    acc[#acc + 1] = #t
  end
  return table.concat(acc, " ")
end
out[#out + 1] = "fill " .. fill(100)

local function sparse(n)
  local t, acc = {}, {}
  for i = 1, n do
    t[i * 2] = i
    if i % 4 ~= 2 then t[i] = i end
    for _ in ipairs(t) do end
    acc[#acc + 1] = #t
  end
  return table.concat(acc, " ")
end
out[#out + 1] = "sparse " .. sparse(60)
return table.concat(out, "\n")
"#;

fn borders(version: LuaVersion) -> String {
    let mut vm = Vm::new(version);
    match vm.eval(CHUNK).expect("chunk runs").first() {
        Some(Value::Str(s)) => String::from_utf8_lossy(s.as_bytes()).into_owned(),
        other => panic!("expected a string, got {other:?}"),
    }
}

#[test]
fn borders_match_puc_51() {
    let want = [
        "calls 4",
        "nil middle 3",
        "nil lead 3",
        "nil tail 2",
        "hole 4",
        "holes 1",
        "vararg 5",
        "call cut 1",
        "call open 3",
        "keyed 1",
        "keyed after list 2",
        "fill 10, drop 5 10",
        "drop 10 9",
        "drop 9 8 7",
        "drop 6 4",
        "hash 20 4",
        "odd of 17 1",
        "odd of 16 1",
        "1 3 1",
        "1 3 4 4",
        "1 2 3 4 4",
        "1 2 3 4 6 8 8",
        "drop 4 8",
        "remove twice 6",
        "hole 3 6",
        "read 6 6",
        "insert 7",
        "insert front 8",
        "drop 8 9 3",
        "ipairs 3",
        "33 minus 33 20 32",
        "minus 17 32",
        "hash 40 41 41",
        "churn 1 2 1 4 5 6 7 8 7 7 7 7 7 7 7 16 17 16 16 16 16 16 16 16 16 16 25 28 29 30 31 32 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 64 65 64 64 64 64 64",
        "ctor 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1",
        "fill 99 99 99 99 99 99 99 99 99 99 99 99 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98",
        "sparse 2 4 4 8 8 12 14 16 16 16 16 16 26 28 30 32 32 32 32 32 32 32 32 32 50 52 54 56 58 60 62 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 98 100 102 104 106 108 110 112 114 116 118 120",
    ]
    .join("\n");
    assert_eq!(borders(LuaVersion::Lua51), want);
}

#[test]
fn borders_match_puc_52() {
    let want = [
        "calls 4",
        "nil middle 3",
        "nil lead 3",
        "nil tail 2",
        "hole 4",
        "holes 1",
        "vararg 5",
        "call cut 1",
        "call open 3",
        "keyed 1",
        "keyed after list 2",
        "fill 10, drop 5 10",
        "drop 10 9",
        "drop 9 8 7",
        "drop 6 4",
        "hash 20 4",
        "odd of 17 1",
        "odd of 16 1",
        "1 3 1",
        "1 3 4 4",
        "1 2 3 4 4",
        "1 2 3 4 6 8 8",
        "drop 4 8",
        "remove twice 6",
        "hole 3 6",
        "read 6 6",
        "insert 7",
        "insert front 8",
        "drop 8 9 3",
        "ipairs 3",
        "33 minus 33 20 32",
        "minus 17 32",
        "hash 40 41 41",
        "churn 1 2 1 4 5 6 7 8 7 7 7 7 7 7 7 16 17 16 16 16 16 16 16 16 16 16 25 28 29 30 31 32 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 64 65 64 64 64 64 64",
        "ctor 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1",
        "fill 99 99 99 99 99 99 99 99 99 99 99 99 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98",
        "sparse 2 4 4 8 8 12 14 16 16 16 16 16 26 28 30 32 32 32 32 32 32 32 32 32 50 52 54 56 58 60 62 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 98 100 102 104 106 108 110 112 114 116 118 120",
    ]
    .join("\n");
    assert_eq!(borders(LuaVersion::Lua52), want);
}

#[test]
fn borders_match_puc_53() {
    let want = [
        "calls 4",
        "nil middle 3",
        "nil lead 3",
        "nil tail 2",
        "hole 4",
        "holes 1",
        "vararg 5",
        "call cut 1",
        "call open 3",
        "keyed 1",
        "keyed after list 2",
        "fill 10, drop 5 10",
        "drop 10 9",
        "drop 9 8 7",
        "drop 6 4",
        "hash 20 4",
        "odd of 17 1",
        "odd of 16 1",
        "1 3 1",
        "1 3 4 4",
        "1 2 3 4 4",
        "1 2 3 4 6 8 8",
        "drop 4 8",
        "remove twice 6",
        "hole 3 6",
        "read 6 6",
        "insert 7",
        "insert front 8",
        "drop 8 9 3",
        "ipairs 3",
        "33 minus 33 20 32",
        "minus 17 32",
        "hash 40 41 41",
        "churn 1 2 1 4 5 6 7 8 7 7 7 7 7 7 7 16 17 16 16 16 16 16 16 16 16 16 25 28 29 30 31 32 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 64 65 64 64 64 64 64",
        "ctor 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1 3/3/1",
        "fill 99 99 99 99 99 99 99 99 99 99 99 99 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 63 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98 99 98",
        "sparse 2 4 4 8 8 12 14 16 16 16 16 16 26 28 30 32 32 32 32 32 32 32 32 32 50 52 54 56 58 60 62 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 98 100 102 104 106 108 110 112 114 116 118 120",
    ]
    .join("\n");
    assert_eq!(borders(LuaVersion::Lua53), want);
}

#[test]
fn borders_match_puc_54() {
    let want = [
        "calls 4",
        "nil middle 3",
        "nil lead 3",
        "nil tail 2",
        "hole 4",
        "holes 8",
        "vararg 5",
        "call cut 1",
        "call open 3",
        "keyed 1",
        "keyed after list 2",
        "fill 10, drop 5 10",
        "drop 10 9",
        "drop 9 8 7",
        "drop 6 4",
        "hash 20 7",
        "odd of 17 1",
        "odd of 16 1",
        "1 3 1",
        "1 3 4 4",
        "1 2 3 4 4",
        "1 2 3 4 6 8 8",
        "drop 4 8",
        "remove twice 6",
        "hole 3 6",
        "read 6 6",
        "insert 7",
        "insert front 8",
        "drop 8 9 7",
        "ipairs 7",
        "33 minus 33 20 32",
        "minus 17 32",
        "hash 40 41 41",
        "churn 1 2 3 4 5 6 7 8 7 7 7 7 7 7 15 16 17 16 16 16 16 16 16 16 16 16 25 28 29 30 31 32 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 63 64 65 64 64 64 64 64",
        "ctor 3/3/3 3/3/3 3/3/3 3/3/3 3/3/3 3/3/3 3/3/3 3/3/3 3/3/3 3/3/3 3/3/3 3/3/3",
        "fill 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99",
        "sparse 2 4 4 8 8 12 14 16 16 16 16 16 26 28 30 32 32 32 32 32 32 32 32 32 50 52 54 56 58 60 62 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 98 100 102 104 106 108 110 112 114 116 118 120",
    ]
    .join("\n");
    assert_eq!(borders(LuaVersion::Lua54), want);
}

#[test]
fn borders_match_puc_55() {
    let want = [
        "calls 2",
        "nil middle 1",
        "nil lead 0",
        "nil tail 2",
        "hole 2",
        "holes 1",
        "vararg 1",
        "call cut 1",
        "call open 0",
        "keyed 1",
        "keyed after list 2",
        "fill 10, drop 5 10",
        "drop 10 9",
        "drop 9 8 7",
        "drop 6 7",
        "hash 20 4",
        "odd of 17 15",
        "odd of 16 15",
        "1 3 1",
        "1 3 4 1",
        "1 2 3 4 4",
        "1 2 3 4 6 8 4",
        "drop 4 3",
        "remove twice 6",
        "hole 3 6",
        "read 6 6",
        "insert 7",
        "insert front 8",
        "drop 8 9 7",
        "ipairs 7",
        "33 minus 33 20 32",
        "minus 17 32",
        "hash 40 41 32",
        "churn 1 2 1 1 5 4 4 4 7 7 7 7 7 7 7 7 17 16 16 16 16 16 16 16 16 16 16 16 16 16 16 16 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 31 65 64 64 64 64 64",
        "ctor 1/1/1 1/1/1 1/1/1 1/1/1 1/1/1 1/1/1 1/1/1 1/1/1 1/1/1 1/1/1 1/1/1 1/1/1",
        "fill 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99 100 99",
        "sparse 2 2 4 4 8 8 8 8 16 16 16 16 16 16 16 16 32 32 32 32 32 32 32 32 32 32 32 32 32 32 32 32 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64 64",
    ]
    .join("\n");
    assert_eq!(borders(LuaVersion::Lua55), want);
}
