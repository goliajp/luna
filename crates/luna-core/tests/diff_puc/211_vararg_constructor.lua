-- v2.13 CORPUS-IV: {...} expansion positions.
local function pack_all(...) return { ... } end
local function pack_mid(...) return { ..., "tail" } end
local function pack_n(...) return select("#", ...), { n = select("#", ...), ... } end
print(#pack_all(1, 2, 3))
print(#pack_all())
local m = pack_mid("a", "b", "c")
print(#m, m[1], m[2])
local n, t = pack_n("x", nil, "z")
print(n, t.n, t[1], t[3])
