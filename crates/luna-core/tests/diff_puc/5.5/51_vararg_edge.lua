-- v2.11 CORPUS-II: vararg edges.
local function f(...) return select("#", ...) end
print(f())              -- 0
print(f(nil))           -- 1
print(f(nil, nil))      -- 2
print(f(1, nil, 3))     -- 3

-- {...} shape
local function pack(...) return {...} end
local t = pack(10, 20, 30)
print(#t, t[1], t[3])

-- {...} truncates trailing nils in # calculation (impl-defined)
-- so use table.pack for reliable n
local u = table.pack(10, nil, 30)
print(u.n, u[1], u[2], u[3])
