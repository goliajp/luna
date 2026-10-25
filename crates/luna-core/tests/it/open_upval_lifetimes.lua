-- open upvalues across thread deaths, error unwinding and collections at
-- every phase: each closure must keep reading and writing its variable
local function churn(n)
  local t = {}
  for i = 1, n do t[i] = {i} end
  return #t
end
local steps = 0
local function step()
  steps = steps + 1
  collectgarbage("step", 0)
end

-- a thread dies by an error with a variable still captured
local fs = {}
local co = coroutine.create(function()
  local a = {1}
  fs[1] = function() a[1] = a[1] + 1; return a[1] end
  coroutine.yield()
  error("boom")
end)
coroutine.resume(co)
emit("suspended", fs[1]())
emit("dies", (coroutine.resume(co)))
for _ = 1, 30 do step(); churn(20) end
emit("dead", fs[1]())
co = nil
collectgarbage(); collectgarbage()
emit("dropped", fs[1]())

-- error unwinding closes what the frames captured
local keep = {}
for i = 1, 40 do
  pcall(function()
    local x = {i}
    keep[i] = function() x[1] = x[1] + 1; return x[1] end
    churn(10)
    step()
    error("e")
  end)
  step()
end
local sum = 0
for i = 1, 40 do sum = sum + keep[i]() end
collectgarbage()
for i = 1, 40 do sum = sum + keep[i]() end
emit("unwound", sum)

-- a suspended thread nothing holds but the closures of its variables
local get, set
do
  local w = coroutine.wrap(function()
    local v = "a"
    get = function() return v end
    set = function(n) v = n end
    coroutine.yield()
  end)
  w()
end
for _ = 1, 30 do step(); churn(20) end
set("b")
collectgarbage(); collectgarbage()
emit("orphaned", get())
set("c")
for _ = 1, 30 do step(); churn(20) end
emit("orphaned", get())

-- closures made and dropped at every step, their variables closed in
-- every phase
local total = 0
for i = 1, 300 do
  local v = i
  local f = function() return v end
  if i % 3 == 0 then f = nil end
  if f then total = total + f() end
  if i % 7 == 0 then step() end
  churn(3)
end
emit("loop", total)

-- a variable captured again after its closures died
local function again()
  local x = 0
  local g = function() x = x + 1 end
  g(); g = nil
  for _ = 1, 20 do step(); churn(10) end
  local h = function() x = x + 10; return x end
  return h()
end
emit("again", again(), again())
-- closures over two new variables: the first is gathered before the
-- second is made
local acc = 0
for i = 1, 100 do
  local a, b = i, 2 * i
  local f = function() return a + b end
  acc = acc + f()
end
emit("pairs", acc)
emit("steps", steps > 0)
