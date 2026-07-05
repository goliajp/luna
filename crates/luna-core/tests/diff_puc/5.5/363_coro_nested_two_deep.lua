-- v2.14 CV.3: two-deep nesting — inner yield only pauses inner.
local inner = coroutine.create(function()
  coroutine.yield("inner-1")
  return "inner-done"
end)
local outer = coroutine.create(function()
  print("first:", coroutine.resume(inner))
  coroutine.yield("outer-pause")
  print("second:", coroutine.resume(inner))
  return "outer-done"
end)
print(coroutine.resume(outer))
print(coroutine.resume(outer))
print(coroutine.status(inner), coroutine.status(outer))
