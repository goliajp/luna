-- v2.14 HD 5.1 seed: implicit `arg` table in vararg functions
-- (removed in 5.2; official 5.1 binary ships with compat on).
local function f(...)
  return arg.n, arg[1], arg[2]
end
print(f("a", "b"))
local function g(...)
  local t = { n = select("#", ...), ... }
  return t.n, t[1]
end
print(g(10, 20, 30))
