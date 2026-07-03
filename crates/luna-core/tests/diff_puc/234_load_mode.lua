-- v2.13 CORPUS-IV: load mode gate — "t" rejects binary chunks,
-- "b" rejects text.
local bin = string.dump(load("return 1"))
local f1, e1 = load(bin, "c", "t")
print(f1 == nil, e1 ~= nil)
local f2 = load(bin, "c", "b")
print(f2 ~= nil and f2())
local f3, e3 = load("return 2", "c", "b")
print(f3 == nil, e3 ~= nil)
local f4 = load("return 3", "c", "bt")
print(f4())
