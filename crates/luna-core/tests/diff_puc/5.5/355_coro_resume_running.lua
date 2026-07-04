-- v2.14 CV.3: resuming a non-suspended coroutine from inside.
local co
co = coroutine.create(function()
  print(coroutine.resume(co))
end)
coroutine.resume(co)
