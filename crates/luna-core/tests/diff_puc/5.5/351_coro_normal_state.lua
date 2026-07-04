-- v2.14 CV.3: outer coroutine shows "normal" while inner runs.
local outer
local inner = coroutine.create(function()
  print("outer from inner:", coroutine.status(outer))
end)
outer = coroutine.create(function()
  coroutine.resume(inner)
end)
coroutine.resume(outer)
