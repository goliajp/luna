-- v2.13 CORPUS-IV: select('#') counts nil holes exactly.
local function count(...) return select("#", ...) end
print(count())
print(count(nil))
print(count(nil, nil))
print(count(1, nil, 3))
print(count(nil, nil, nil, 4))
local function tail(...) return select(2, ...) end
print(tail("a", "b", "c"))
