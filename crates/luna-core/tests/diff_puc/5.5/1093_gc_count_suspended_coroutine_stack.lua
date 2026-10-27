-- a suspended coroutine's stack is marked up to its top only: a dead
-- string left in a register above the call that yielded is freed. The
-- body is loaded with the collector stopped: 5.5's parser anchors its
-- constants in a table whose key may land on a node a removed key left
-- (an address coincidence of PUC's), and a collection step taken while
-- it parses would differ from luna's by it
collectgarbage("stop")
load([[
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return collectgarbage("count") * 1024
end
local before = count()
local co = coroutine.wrap(function()
  local s = "a" .. "b" .. "c" .. ("x"):rep(2000) .. "d"
  coroutine.yield(#s)
  return #s
end)
print("yielded", co(), string.format("%.0f", count() - before))
print("returned", co(), string.format("%.0f", count() - before))
]])()
