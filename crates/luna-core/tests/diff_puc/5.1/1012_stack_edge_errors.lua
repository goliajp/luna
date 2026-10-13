-- rule: ldo.growCI.1 ldo.luaD_growstack.2 ldo.luaD_reallocCI.1 lgc.checkstacksizes.1 lstate.stack_init.1
-- errors raised at the very top of a full stack (in a coroutine, whose
-- stack starts where it does whatever host runs the script), by Lua code and by
-- natives: how much room a message handler then has (free: what
-- table.unpack's lua_checkstack grants it), how deep a recursing handler
-- gets, and the message. The collector is stopped (its runs shrink a
-- stack that is handling an overflow).
collectgarbage("stop")
coroutine.wrap(function()
local unpack = table.unpack or unpack
local E = {}
local free, seen
local function hfree(m)
  seen = m
  local lo, hi = 0, 1100000
  while lo < hi do
    local mid = math.floor((lo + hi + 1) / 2)
    if pcall(unpack, E, 1, mid) then lo = mid else hi = mid - 1 end
  end
  free = lo
  return m
end
local depth
local function hdeep(m) depth = depth + 1 return hdeep(m) .. "" end
local function norm(m)
  if type(m) ~= "string" then return "<" .. type(m) .. ">" end
  return (m:gsub("^[^\n]-:%d+: ", "@ "))
end
local function measure(label, f, ...)
  free, seen, depth = -1, nil, 0
  xpcall(f, hfree, ...)
  local f1, s1 = free, seen
  xpcall(f, hdeep, ...)
  print(label, f1, depth, norm(s1))
end
-- a recursion that raises at depth n; `kind` picks the error
local kinds = {
  arith = function(x) return x + nil end,
  index = function(x) return x.y.z end,
  call = function(x) return x() end,
  concat = function(x) return x .. {} end,
  native = function(x) return string.rep(x, -1, {}) end,
  err = function(x) error("e") end,
  errt = function(x) error({}) end,
}
local function rec(n, k)
  if n == 0 then return kinds[k](nil) end
  return 1 + rec(n - 1, k)
end
-- the deepest recursion that raises its own error rather than overflowing
local function deepest(k)
  local lo, hi = 1, 2000000
  while lo < hi do
    local mid = math.floor((lo + hi + 1) / 2)
    local ok, e = pcall(rec, mid, k)
    if not tostring(e):find("stack overflow") then lo = mid else hi = mid - 1 end
  end
  return lo
end
for _, k in ipairs({ "arith", "index", "call", "concat", "native", "err", "errt" }) do
  local d = deepest(k)
  print(k .. ":deepest", d)
  for n = d - 6, d + 1 do
    measure(k .. ":" .. (n - d), rec, n, k)
  end
end
end)()
