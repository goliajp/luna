-- v2.13 CORPUS-IV: error levels. NOTE: absolute line numbers are
-- NOT comparable under the diff harness (the luna side runs with
-- a print-capture preamble that shifts line numbers), so assert
-- structure: level 1 carries a position prefix, level 2 points at
-- the caller (different line than level 1's), level 0 is bare.
local function thrower_l1() error("m1") end
local function thrower_l2() error("m2", 2) end
local ok1, e1 = pcall(thrower_l1)
local ok2, e2 = pcall(thrower_l2)
print(ok1, e1:match(":%d+: m1$") ~= nil)
print(ok2, e2)
local ok3, e3 = pcall(function() error("m0", 0) end)
print(ok3, e3)
local l1 = tonumber(e1:match(":(%d+): m1$"))
local def_line = tonumber(e1:match(":(%d+)")) and true
print(type(l1) == "number", def_line)
