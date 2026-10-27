-- How many values a function returns where it returns on one path and
-- falls off its end, or returns nothing, on another; called often enough
-- for the JIT to take it.
local function pack(...) return {n = select("#", ...), ...} end
local function none() end
local function two() return 1, 2 end
local down
down = function(a, b) if a > 0 then return down(a - 1, b) end end
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
local function f(a, b)
    if a == b then return a end
end
print("count:", select("#", f(1, 99)))
local result = f(1, 99)
print("result:", result, type(result))
local fs = {
  function(a, b) if a == b then return a end end,
  function(a, b) if a < b then return a else local x = a + b end end,
  function(a, b) if a < b then return end return a end,
  function(a, b) if a < b then return a end return end,
  function(a, b) local x = a + b end,
  function(a, b) return end,
  function(a, b) if a < b then return a + b end end,
  function(a, b) if a < b then return none() end return a end,
  function(a, b) return none() end,
  function(a, b) if a < b then return two() end return a end,
  function(a, b) return two() end,
  function(a, b) if a > b then return a end end,
  function(a, b) while a < b do a = a + 1 if a == 5 then return a end end end,
  function(a, b) for i = a, b do if i == 1000 then return i end end end,
  down,
  function(a, b) if a < b then return fib(a) end end,
}
for k, g in ipairs(fs) do
  local s = {}
  for i = 1, 300 do
    local t = pack(g(i % 7, i % 5))
    if i % 50 == 0 then s[#s + 1] = t.n .. ":" .. tostring(t[1]) .. "," .. tostring(t[2]) end
  end
  print(k, table.concat(s, " "))
end
