-- ten thousand suspended threads through full and step collections: each
-- thread's stack is shrunk once a cycle, as PUC's traversal does
local t = {}
for i = 1, 10000 do
  local co = coroutine.create(function(a)
    local x = {a}
    coroutine.yield(x)
    return a
  end)
  coroutine.resume(co, i)
  t[i] = co
end
collectgarbage()
print(math.floor(collectgarbage("count")))
-- a whole cycle: a count taken mid-cycle depends on where the steps
-- stopped, and that moves with the string hash seed
repeat until collectgarbage("step", 0)
print(math.floor(collectgarbage("count")))
collectgarbage()
print(math.floor(collectgarbage("count")))
for i = 1, 10000, 2499 do
  print(i, coroutine.status(t[i]), select(2, coroutine.resume(t[i])))
end
