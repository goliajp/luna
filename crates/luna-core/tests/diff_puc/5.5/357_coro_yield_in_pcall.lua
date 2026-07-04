-- v2.14 CV.3: yield crosses a pcall inside the coroutine.
local co = coroutine.create(function()
  local ok, v = pcall(function()
    return coroutine.yield("from pcall") .. "!"
  end)
  return ok, v
end)
print(coroutine.resume(co))
print(coroutine.resume(co, "resumed"))
