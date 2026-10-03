-- error() called directly by a C function, and error() with a number.
-- luaB_error adds luaL_where(level) only to a message that passes its
-- string test: lua_isstring up to 5.2 (a number is positioned and comes
-- out as a string), lua_type == LUA_TSTRING from 5.3 (a number stays a
-- number). luaL_where gives nothing when the function at that level is a
-- C function, so error called by gsub, sort, pcall or a metamethod slot
-- has no position. Positions in this chunk print as <pos>.
local function show(...)
  local out = {}
  for i = 1, select('#', ...) do
    local v = select(i, ...)
    local s = type(v) == "table" and "<table>" or tostring(v)
    -- the chunk's name and lines differ between the two runners; whether a
    -- position is there is what counts
    if type(v) == "string" then s = s:gsub("^[%w_]+:%d+: ", "<pos> ") end
    out[#out + 1] = type(v) .. ":" .. s
  end
  print(table.concat(out, " "))
end
show(pcall(error, 42))
show(pcall(error, 42, 2))
show(pcall(error, 42, 0))
show(pcall(error, 4.5))
show(pcall(function() error(42) end))
show(pcall(function() error(42.5, 2) end))
show(pcall(function() error(-7, 1) end))
show(pcall(function() error("s") end))
show(pcall(function() error("lvl2", 2) end))
show(pcall(error))
show(pcall(error, "p"))
show(pcall(error, "p", 2))
show(pcall(function() return pcall(error, 4, 2) end))
show(pcall(string.gsub, "a", "a", error))
show(pcall(function() return string.gsub("a", "a", error) end))
show(pcall(function() return string.gsub("1", "%d", error) end))
show(pcall(function() return ("x"):rep(3):gsub(".", error) end))
show(pcall(function() return string.gsub("a", "(a)", function(x) error(x) end) end))
show(pcall(function() table.sort({ 3, 2, 1 }, error) end))
show(pcall(function() table.sort({ 3, 2, 1 }, function(a) error(a) end) end))
show(pcall(function() return xpcall(error, function(m) return m end, "z") end))
show(pcall(function() return xpcall(error, function(m) return m end, 7) end))
show(pcall(function() return coroutine.wrap(error)("c") end))
show(pcall(function() return coroutine.wrap(error)(5) end))
show(pcall(function() local t = setmetatable({}, { __index = error }) return t[1] end))
show(pcall(function() local t = setmetatable({}, { __newindex = error }) t[1] = 2 end))
show(pcall(function() local t = setmetatable({}, { __call = error }) t() end))
show(pcall(function() local t = setmetatable({}, { __add = error }) return t + 1 end))
show(pcall(function() for w in string.gmatch("ab", "%a") do error(w, 1) end end))
show(pcall(function() local f = (loadstring or load)("error('x')") return f() end))
show(pcall(function() local f = (loadstring or load)("error(3)") return f() end))
