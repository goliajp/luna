-- v2.10 CORPUS: table.pack + select.
local t = table.pack(1, 2, 3, 4)
print(t.n, t[1], t[4])

print(select("#", "a", "b", "c"))
print(select(2, "a", "b", "c"))

-- vararg passthrough
local function fwd(...) return select("#", ...), ... end
print(fwd(10, 20, 30))
