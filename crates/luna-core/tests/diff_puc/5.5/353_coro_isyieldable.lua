-- v2.14 CV.3: isyieldable inside vs outside.
print(coroutine.isyieldable())
local co = coroutine.create(function()
  print(coroutine.isyieldable())
end)
coroutine.resume(co)
