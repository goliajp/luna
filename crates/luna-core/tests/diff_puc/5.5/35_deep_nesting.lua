-- v2.10 CORPUS: deep call stack + local reuse.
local function f(n)
  if n == 0 then return 0 end
  local x = n * 2
  return x + f(n - 1)
end
print(f(20))  -- sum(1..20)*2 = 420

-- deeper (still <MAX_LUA_STACK)
local function g(n, acc)
  if n == 0 then return acc end
  return g(n - 1, acc + n)
end
print(g(100, 0))  -- 5050

-- tail call in expression position
local function h(n) return n <= 1 and n or h(n - 1) end
print(h(50))  -- 1
