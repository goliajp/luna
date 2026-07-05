-- v2.15 P2.5 (5.1): coroutine basic yield/resume.
local co = coroutine.create(function(x)
  local y = coroutine.yield(x + 1)
  return y * 2
end)
print(coroutine.resume(co, 10))
print(coroutine.resume(co, 5))
print(coroutine.status(co))
