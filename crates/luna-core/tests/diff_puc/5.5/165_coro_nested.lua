-- v2.13 CORPUS-IV: nested coroutines — resume/yield value
-- plumbing through two levels.
local inner = coroutine.create(function(a)
  local b = coroutine.yield(a + 1)
  return b * 10
end)
local outer = coroutine.create(function(x)
  local ok, v = coroutine.resume(inner, x)
  local y = coroutine.yield(v)
  local ok2, w = coroutine.resume(inner, y)
  return w
end)
print(coroutine.resume(outer, 5))   -- true 6
print(coroutine.resume(outer, 7))   -- true 70
print(coroutine.status(outer), coroutine.status(inner))
