-- string.unpack filling the stack to its limit and then failing: how much
-- room the message handler has (lua_checkstack in it), and the message
-- it got. In a coroutine, whose stack starts where it does in any host.
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
local function measure(label, f, ...)
  free, seen = -1, nil
  local ok, r = xpcall(f, hfree, ...)
  print(label, free, tostring(seen))
end
local max = _VERSION == "Lua 5.3" and 999976 or 999980
local Z = string.rep('\0', 1000100)
for _, k in ipairs({ 100, 40, 30, 25, 22, 21, 20, 19, 18, 10, 5, 2, 1, 0 }) do
  measure("short at max-" .. k, string.unpack, string.rep('B', max - k) .. 'i16', string.rep('\0', max - k))
end
measure("overflow", string.unpack, string.rep('B', 1000000), Z)
end)()
