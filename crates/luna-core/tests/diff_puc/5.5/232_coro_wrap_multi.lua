-- v2.13 CORPUS-IV: coroutine.wrap multivalue plumbing both ways.
local w = coroutine.wrap(function(a, b)
  local c, d = coroutine.yield(a + b, a - b)
  return c * d, "done"
end)
print(w(10, 3))
print(w(4, 5))
local gen = coroutine.wrap(function()
  for i = 1, 3 do coroutine.yield(i, i * i) end
end)
print(gen())
print(gen())
print(gen())
