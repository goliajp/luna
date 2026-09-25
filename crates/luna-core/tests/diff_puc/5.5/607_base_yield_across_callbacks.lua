-- Base functions that call Lua back with a plain lua_call are not
-- yieldable: __tostring (tostring, and print's call to tostring on <=5.3),
-- __pairs on 5.2/5.3 (5.4+ use lua_callk), __ipairs, and load's reader
-- (the parser runs non-yieldable). A yield there fails; the message is
-- reduced to whether it names the boundary, since its wording and position
-- belong to coroutine.yield.
local function try(label, f)
  local co = coroutine.create(f)
  local ok, v = coroutine.resume(co)
  if not ok then v = string.find(v, "yield across", 1, true) and "<yield across boundary>" or v end
  print(label, ok, v, coroutine.status(co))
end
local ts = setmetatable({}, {__tostring = function() coroutine.yield("ts") return "s" end})
try("tostring", function() return tostring(ts) end)
try("print", function() print(ts) end)
try("pairs", function()
  for _ in pairs(setmetatable({}, {__pairs = function() coroutine.yield("p") return next, {} end})) do end
  return "done"
end)
try("ipairs", function()
  for _ in ipairs(setmetatable({}, {__ipairs = function() coroutine.yield("i") return next, {} end})) do end
  return "done"
end)
try("load reader", function()
  local n = 0
  return load(function() n = n + 1 coroutine.yield("r") if n == 1 then return "return 1" end end)
end)
