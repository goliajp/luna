-- an error inside lua_load (a chunk of a kind the mode does not allow, a
-- reader that returns no string, a reader that yields) is caught by
-- luaD_protectedparser's protected call, which shrinks the stack: the
-- count after it has the smaller stack
collectgarbage("stop")
local function deep(n) if n > 0 then return deep(n - 1) + 1 end return 0 end
local function count() return string.format("%.0f", collectgarbage("count") * 1024) end
local function after(what, ...)
  deep(300)
  local base = count()
  local f, e = load(...)
  local c = count() - base
  print(what, f, (e:gsub("^.-:%d+: ", "")), c)
end
-- under pcall, which runs no message handler on the reader's error (the
-- one lua.c runs the script under would)
pcall(after, "mode", "return 1", "=x", "b")
pcall(after, "reader", function() return {} end)
print(coroutine.wrap(function()
  pcall(after, "yield", function() coroutine.yield() end)
  return "done"
end)())
