-- Arithmetic, bitwise and comparison operators with a numeral on one side:
-- each expression runs over integers, floats, numeric strings, a value with
-- every metamethod (argument order is printed) and values that raise.
local load = loadstring or load
local v53 = _VERSION >= "Lua 5.3"
local mtype = math.type or type

local function tag(v)
  if type(v) == "table" then return "obj" end
  if type(v) == "number" then return mtype(v) .. ":" .. tostring(v) end
  return type(v) .. ":" .. tostring(v)
end

local mt = {}
for _, ev in ipairs { "add", "sub", "mul", "div", "mod", "pow", "idiv", "band", "bor", "bxor", "shl", "shr" } do
  mt["__" .. ev] = function(a, b) return ev .. "(" .. tag(a) .. "," .. tag(b) .. ")" end
end
local log = {}
mt.__lt = function(a, b) log[#log + 1] = "lt(" .. tag(a) .. "," .. tag(b) .. ")" return true end
mt.__le = function(a, b) log[#log + 1] = "le(" .. tag(a) .. "," .. tag(b) .. ")" return false end
mt.__eq = function(a, b) log[#log + 1] = "eq" return true end
local obj = setmetatable({}, mt)

local vals = {
  0, 1, -1, 2, 5, 7, -7, 126, 127, 128, 129, -127, -128, -129, 1000, 65536,
  0.5, -0.5, 2.5, 1e15, -12345.75, 5.0, 1.0,
  "10", "0x10", " 3 ", "2.5", "abc", obj, {}, true,
}
if v53 then
  vals[#vals + 1] = math.maxinteger
  vals[#vals + 1] = math.mininteger
  vals[#vals + 1] = 2^53
  vals[#vals + 1] = 3.0
end
-- nil last: `#vals` must not depend on a hole
local nvals = #vals + 1

local exprs = {
  "x + 1", "1 + x", "x + 127", "x + 128", "x + 129", "x + -127", "x + -128", "x + 1000", "1000 + x",
  "x + 0.5", "0.5 + x", "x + 2.0",
  "x - 1", "x - 128", "x - 129", "x - -128", "x - 1000", "x - 0.5", "x - 0", "1 - x",
  "x * 3", "3 * x", "x * 0.5", "0.5 * x", "x * -1", "x * 1000000",
  "x % 7", "x % -7", "x % 2.5", "x % 1000", "7 % x",
  "x / 4", "x / 0.5", "x / -3", "4 / x",
  "x ^ 2", "x ^ 0.5", "x ^ -1", "2 ^ x",
  "x == 1", "x ~= 1", "1 == x", "x == 1.0", "x == 0.5", "x == 1000", "x == -128", "x == 128", "x == 129",
  "x == '10'", "'10' == x", "x ~= 'abc'",
  "x < 5", "x <= 5", "x > 5", "x >= 5", "5 < x", "5 <= x", "5 > x", "5 >= x",
  "x < 5.0", "x <= 5.0", "5.0 < x", "x < 0.5", "x < 1000", "x < -128", "x > 128", "x >= 129", "x < '5'",
  "not (x < 5)", "x < 5 and 1 or 2", "(x >= 1 and x <= 7) or x == 127",
}
if v53 then
  for _, e in ipairs {
    "x // 3", "x // -3", "x // 0.5", "x // 1000", "3 // x",
    "x & 12", "12 & x", "x & 1000", "x | 1", "1 | x", "x ~ 255", "255 ~ x", "x & 1.0",
    "x << 3", "x >> 3", "x << -3", "x >> -3", "x << 63", "x << 64", "x >> 70", "x << 0", "3 << x", "x << 127", "x >> 128",
  } do
    exprs[#exprs + 1] = e
  end
end

local function fmt(ok, v)
  if ok then return tag(v) end
  -- the position prefix names the chunk, which differs between hosts
  return "error: " .. tostring(v):gsub("^[^:]*:%d+: ", "")
end

for _, e in ipairs(exprs) do
  local f = assert(load("local x = ... return " .. e))
  local out = {}
  for i = 1, nvals do
    log = {}
    local ok, v = pcall(f, vals[i])
    out[#out + 1] = fmt(ok, v) .. (#log > 0 and " [" .. table.concat(log, ";") .. "]" or "")
  end
  print(e .. " => " .. table.concat(out, " | "))
end

-- the same forms inside loops and conditions
local function loops(n)
  local s, t, u, c = 0, 1, 0.5, 0
  for i = 1, n do
    s = s + i % 7 - 1
    t = t * 3 % 1000 + 1
    u = u * 0.5 + 1
    if i < 10 then c = c + 1 end
    if i >= 190 then c = c + 100 end
    if 50 <= i then c = c + 1000 end
    if i == 100 then c = c + 1000000 end
    if i ~= 100 then c = c - 1 end
    if u > 1 then c = c + 2 end
  end
  local w, k = 0, 0
  while k < 50 do k = k + 1 w = w + k / 4 end
  repeat k = k - 2 w = w - 0.5 until k <= 3
  return s, t, u, c, w, k
end
for _ = 1, 3 do print(loops(200)) end

-- a function and recursion on the constant forms
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
print(fib(20))
local function collatz(n) local c = 0 while n ~= 1 do if n % 2 == 0 then n = n / 2 else n = 3 * n + 1 end c = c + 1 end return c end
print(collatz(27), collatz(97))
