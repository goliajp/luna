-- open upvalues whose closures die while incremental cycles run: closed by
-- a return, by an error unwinding, inside a coroutine, taken back by a new
-- closure, and left open in a coroutine the collector frees
pcall(collectgarbage, "setpause", 1)
pcall(collectgarbage, "setstepmul", 1000)
local sum = 0
local function steps(n) for _ = 1, n do collectgarbage("step", 1) end end
for round = 1, 150 do
  -- closed by a return after its closure died mid-cycle
  local function ret()
    local a = round
    local f = function() a = a + 1 end
    f(); f = nil
    steps(2)
    local g = function() return a end
    steps(1)
    return g()
  end
  sum = sum + ret()
  -- closed by an error unwinding
  local ok, v = pcall(function()
    local b = round
    local f = function() return b end
    f = nil
    steps(2)
    error({b})
  end)
  sum = sum + v[1]
  -- in a coroutine, yielding between the closure's death and the close
  local co = coroutine.wrap(function()
    local c = round
    local f = function() c = c + 2 end
    f(); f = nil
    coroutine.yield(c)
    steps(2)
    local h = function() return c end
    coroutine.yield(h())
    return c
  end)
  sum = sum + co(); steps(1); sum = sum + co(); sum = sum + co()
  -- a suspended coroutine dropped with an upvalue still open, one closure
  -- over it kept, another not
  local keep
  local co2 = coroutine.create(function()
    local d = round
    keep = function() return d end
    local lost = function() d = d + 1 end
    lost = nil
    coroutine.yield()
  end)
  coroutine.resume(co2)
  co2 = nil
  steps(4)
  sum = sum + keep()
  -- a coroutine killed by an error with its upvalues open
  local co3 = coroutine.create(function()
    local e = round
    keep = function() return e end
    error("x")
  end)
  coroutine.resume(co3)
  co3 = nil
  steps(4)
  sum = sum + keep()
end
collectgarbage()
print(sum)
