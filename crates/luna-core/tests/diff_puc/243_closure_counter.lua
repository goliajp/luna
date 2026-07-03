-- v2.13 CORPUS-IV: integration — closure counters + memoized fib.
local function counter()
  local n = 0
  return function() n = n + 1 return n end
end
local c1, c2 = counter(), counter()
print(c1(), c1(), c1(), c2())
local memo = {}
local function fib(n)
  if n < 2 then return n end
  if memo[n] then return memo[n] end
  local r = fib(n - 1) + fib(n - 2)
  memo[n] = r
  return r
end
print(fib(30), fib(50))
print(#(function() local t = {} for k in pairs(memo) do t[#t + 1] = k end return t end)())
