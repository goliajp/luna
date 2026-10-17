-- suspended coroutines keep the stacks and call records they grew; the
-- count after full collections has them, and loses them when the
-- coroutines go
collectgarbage("stop")
if _VERSION >= "Lua 5.4" then collectgarbage("incremental") end
-- a full collection first: the run starts with no cycle in progress
collectgarbage(); collectgarbage("stop")
local function count()
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  return string.format("%.0f", collectgarbage("count") * 1024)
end
local base = count()
local cos = {}
for i = 1, 6 do
  cos[i] = coroutine.create(function(n)
    local function down(k) if k == 0 then coroutine.yield(k) else down(k - 1) end end
    down(n)
  end)
  coroutine.resume(cos[i], i * 40)
end
print(count() - base)
cos[2], cos[4] = nil, nil
print(count() - base)
cos = nil
print(count() - base)
