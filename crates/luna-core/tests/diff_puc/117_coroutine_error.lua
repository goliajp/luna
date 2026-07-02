-- v2.12 CORPUS-III: coroutine errors propagate via resume.
local co = coroutine.create(function()
  error("in-coro")
end)
local ok, err = coroutine.resume(co)
print(ok, err:match(": (.+)$") or err)
print(coroutine.status(co))    -- dead
