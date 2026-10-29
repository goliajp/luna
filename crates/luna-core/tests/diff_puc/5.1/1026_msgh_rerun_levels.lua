-- a message handler that fails at once, with no Lua frame to unwind, runs
-- again on its own error one C level deeper, as PUC's nested luaG_errormsg
-- does, until the C levels run out; the call hook counts the runs, and
-- stops counting once its own call is refused at the limit (the hooks stay
-- off until the protected call catches the error), here and ten protected
-- calls deeper
local function runs()
  local n = 0
  debug.sethook(function () n = n + 1 end, "c")
  local ok, msg = xpcall(error, error)
  debug.sethook()
  return ok, msg, n
end
local ok, msg, top = runs()
local deeper
local function nest(k)
  if k == 0 then
    local _, _, n = runs()
    deeper = n
  else
    pcall(nest, k - 1)
  end
end
nest(10)
print(ok, msg, top, deeper)
