-- a suspended coroutine's stack is marked up to its top only: a dead
-- string left in a register above the call that yielded is freed
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
