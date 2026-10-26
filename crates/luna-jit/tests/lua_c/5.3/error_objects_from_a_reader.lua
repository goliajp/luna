-- a reader raising values that are no strings, under lua.c's message
-- handler: the description of each is a string PUC's lua_pushfstring
-- makes, then the traceback; the count after each load
local function run(err)
  collectgarbage()
  collectgarbage("stop")
  local before = collectgarbage("count") * 1024
  local i = 0
  local f, msg = load(function()
    i = i + 1
    if i == 1 then return "local x = " end
    error(err)
  end)
  local after = collectgarbage("count") * 1024
  collectgarbage("restart")
  print(f, #tostring(msg), after - before)
end
run({})
run(nil)
run(true)
run(setmetatable({}, {__tostring = function() return "described" end}))
