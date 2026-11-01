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
-- fifty steps into a cycle, then a full collection ends it: a count taken
-- mid-cycle depends on where the steps stopped, which moves with the string
-- hash seed, and stepping to the cycle's end takes thousands of steps.
-- No count taken by stepping is the same on every PUC run: with 200 threads
-- whose stacks grew after the last collection, the step at which traversal
-- reaches them moves by a step or two with the seed, and the bytes a whole
-- stepped cycle frees differ by 384 or 768 between runs, so stack shrinking
-- during a step's traversal is not compared here
for _ = 1, 50 do collectgarbage("step", 0) end
collectgarbage()
print(math.floor(collectgarbage("count")))
collectgarbage()
print(math.floor(collectgarbage("count")))
for i = 1, 10000, 2499 do
  print(i, coroutine.status(t[i]), select(2, coroutine.resume(t[i])))
end
