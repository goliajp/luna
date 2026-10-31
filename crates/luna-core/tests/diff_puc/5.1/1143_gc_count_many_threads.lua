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
for _ = 1, 50 do collectgarbage("step", 0) end
print(math.floor(collectgarbage("count")))
collectgarbage()
print(math.floor(collectgarbage("count")))
for i = 1, 10000, 2499 do
  print(i, coroutine.status(t[i]), select(2, coroutine.resume(t[i])))
end
