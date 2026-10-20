-- an upvalue still open whose closures are all dead: 5.1 and 5.2 free it
-- when the collection sweeps its thread, though its variable is still in
-- scope; a later closure over the variable makes a new one. Measured with
-- the collector stopped between the explicit collections
collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return collectgarbage("count") * 1024
end
local out = {}
local function case(name, f)
  local before = count()
  f()
  out[#out + 1] = string.format("%s %.0f", name, count() - before)
end
case("one dead closure", function()
  local x = 0
  local f = function() x = x + 1 end
  f()
  f = nil
  local mid = count()
  local g = function() return x end
  out[#out + 1] = string.format("  kept %s", tostring(g() == 1))
end)
case("many, one kept", function()
  local a, b, c = 1, 2, 3
  local keep = function() return b end
  for i = 1, 3 do
    local h = function() return a + c + i end
    h()
  end
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  out[#out + 1] = string.format("  b %d", keep())
end)
local co = coroutine.create(function()
  local y = 10
  local f = function() y = y + 1 end
  f()
  f = nil
  coroutine.yield()
  local g = function() return y end
  return g()
end)
case("in a coroutine", function()
  coroutine.resume(co)
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  out[#out + 1] = string.format("  y %s", tostring(select(2, coroutine.resume(co))))
end)
for _, l in ipairs(out) do print(l) end
