-- v2.13 CORPUS-IV: yield across pcall boundary (legal 5.2+).
local co = coroutine.create(function()
  local ok, v = pcall(function()
    local got = coroutine.yield("from_inside_pcall")
    return got .. "_returned"
  end)
  return ok, v
end)
print(coroutine.resume(co))
print(coroutine.resume(co, "fed"))
print(coroutine.status(co))
