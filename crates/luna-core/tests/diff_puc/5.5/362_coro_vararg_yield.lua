-- v2.14 CV.3: vararg body + multi-value yield round trips.
local co = coroutine.create(function(...)
  local n = select("#", ...)
  coroutine.yield(n, ...)
  return select(2, ...)
end)
print(coroutine.resume(co, "a", nil, "c"))
print(coroutine.resume(co))
