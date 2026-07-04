-- v2.13 CORPUS-IV: `local function` binds the name BEFORE the
-- body (self-recursion works); `local f = function` does not.
local function fact(n)
  if n <= 1 then return 1 end
  return n * fact(n - 1)
end
print(fact(6))
local ok = pcall(load([[
  local g = function(n) if n <= 0 then return 0 end return g(n - 1) end
  return g(3)
]]))
print(ok)
local function even(n) if n == 0 then return true else return not even(n - 1) end end
print(even(10), even(7))
