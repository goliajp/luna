-- v2.14 CV.3: yield/resume value plumbing across arities.
local co = coroutine.create(function(a, b)
  local x, y, z = coroutine.yield(a + b)
  local w = coroutine.yield(x, y, z)
  return "done", w
end)
print(coroutine.resume(co, 1, 2))
print(coroutine.resume(co, 10, 20, 30))
print(coroutine.resume(co, "final"))
print(coroutine.resume(co))
