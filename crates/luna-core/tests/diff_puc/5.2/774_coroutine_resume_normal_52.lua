-- v3.1 iopkg: 5.2 auxresume calls a thread whose current frame is empty
-- dead (lua_gettop(co) == 0). A coroutine waiting in a wrapped call has
-- such a frame, its arguments having moved to the coroutine it resumed;
-- one waiting in coroutine.resume still holds the thread argument and is
-- only non-suspended.
local A
A = coroutine.create(function()
  local w = coroutine.wrap(function() return coroutine.resume(A) end)
  local r1 = {w()}
  local B = coroutine.create(function() return coroutine.resume(A) end)
  local r2 = {coroutine.resume(B)}
  local w2 = coroutine.wrap(function(...) return coroutine.resume(A) end)
  local r3 = {w2(1, 2)}
  return r1[1], r1[2], r2[2], r2[3], r3[1], r3[2]
end)
print(coroutine.resume(A))
local main = coroutine.running()
if type(main) == "thread" then
  print(coroutine.wrap(function() return coroutine.resume(main) end)())
  print(coroutine.resume(coroutine.create(function() return coroutine.resume(main) end)))
end
