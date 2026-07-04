-- v2.2 Phase 5 (DP) deterministic diff fixture: table library.
local t = {10, 20, 30, 40, 50}
print(#t)
print(t[1], t[3], t[5])

table.insert(t, 60)
print(#t, t[6])

table.insert(t, 1, 0)
print(t[1], t[2])

local r = table.remove(t)
print(r, #t)

local s = table.concat({"a","b","c","d"}, "-")
print(s)

-- table.sort stability isn't guaranteed across impls; use distinct keys
local s2 = {5, 1, 4, 2, 3}
table.sort(s2)
for i, v in ipairs(s2) do io.write(v, " ") end
print()

-- pairs iteration order is impl-defined, so only test ipairs (ordered)
local p = {x=1, y=2, z=3}
print(p.x, p.y, p.z)
