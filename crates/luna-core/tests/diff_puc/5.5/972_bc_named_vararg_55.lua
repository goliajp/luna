-- v3.1 bytecode: named varargs, read in place and materialised as a table.
local function peek(...t) return t.n, t[1], t[3] end
local function keep(...t) t[1] = "changed"; return t, ... end
print(peek("a", "b", "c"))
local t, a, b = keep("x", "y")
print(t.n, t[1], a, b)
local function count(...) return select("#", ...) end
print(count(), count(nil, nil))
