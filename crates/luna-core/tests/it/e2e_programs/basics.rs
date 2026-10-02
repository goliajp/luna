//! Programs for core language features: recursion, loops, closures, coroutines and pcall.

use super::Program;
use luna_core::version::LuaVersion;

pub(super) const PROGRAMS: &[Program] = &[
    Program {
        name: "fib_recursive",
        src: r#"
local function f(n)
    if n < 2 then return n end
    return f(n-1) + f(n-2)
end
print(f(15))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "factorial_iter",
        src: r#"
local function fact(n)
    local p = 1
    for i = 1, n do p = p * i end
    return p
end
print(fact(10))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "string_concat_loop",
        src: r#"
local parts = {}
for i = 1, 50 do parts[i] = tostring(i) end
print(table.concat(parts, ","))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "table_index_sort",
        src: r#"
local t = {5, 3, 1, 4, 2, 9, 7, 8, 6, 10}
table.sort(t)
print(table.concat(t, " "))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "string_pattern_capture",
        src: r#"
local s = "key1=42, key2=99, key3=137"
for k, v in s:gmatch("(%w+)=(%d+)") do
    print(k, v)
end
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "coroutine_generator",
        src: r#"
local function gen(n)
    return coroutine.wrap(function()
        for i = 1, n do coroutine.yield(i * i) end
    end)
end
local out = {}
for v in gen(5) do out[#out+1] = tostring(v) end
print(table.concat(out, ","))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "pcall_error_chain",
        src: r#"
local function inner() error("inner-boom") end
local function middle() inner() end
local ok, err = pcall(middle)
print(ok, type(err))
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "linked_list_traverse",
        src: r#"
-- build a 100-node linked list, sum the values
local head = nil
for i = 100, 1, -1 do head = {val = i, next = head} end
local sum = 0
local n = head
while n do sum = sum + n.val; n = n.next end
print(sum)
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "metatable_inheritance",
        src: r#"
local Animal = {sound = "?"}
Animal.__index = Animal
function Animal.speak(a) return a.name .. " says " .. a.sound end
local function new(name, sound)
    return setmetatable({name = name, sound = sound}, Animal)
end
local cat = new("Cat", "meow")
local dog = new("Dog", "woof")
print(cat:speak())
print(dog:speak())
"#,
        min_version: LuaVersion::Lua51,
    },
    Program {
        name: "prime_sieve_int",
        src: r#"
local N = 100
local is = {}
for i = 2, N do is[i] = true end
for i = 2, N do
    if is[i] then
        for j = i*i, N, i do is[j] = false end
    end
end
local primes = {}
for i = 2, N do if is[i] then primes[#primes+1] = tostring(i) end end
print(table.concat(primes, ","))
"#,
        min_version: LuaVersion::Lua51,
    },
];
