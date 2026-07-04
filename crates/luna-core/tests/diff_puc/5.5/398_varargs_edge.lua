-- v2.14 CV.3: vararg edge shapes — holes, select negatives, table.pack.
local function f(...)
  return select("#", ...), table.pack(...).n
end
print(f())
print(f(nil))
print(f(nil, nil, nil))
local function g(...) return ... end
print(g(1, nil, 3))
print((g(1, nil, 3)))
