-- v3.1: '__tostring' must return a string is a luaL_error raised inside the
-- library function, so it carries luaL_where(1): the position of whatever
-- called that function. 5.3's print reaches tostring through lua_call (a C
-- caller: no position); 5.4+ print calls luaL_tolstring itself (its caller
-- is Lua: positioned). Chunk names differ between harness sides, so only the
-- presence of a position is printed. `print` itself is pinned by
-- tests/tostring_error_position.rs: this harness replaces print with a Lua
-- function on luna's side, which changes who calls tostring.
local bad = setmetatable({}, {__tostring = function() return nil end})
for _, case in ipairs{
  {"tostring", function() return tostring(bad) end},
  {"format", function() return string.format("%s", bad) end},
} do
  local ok, msg = pcall(case[2])
  print(case[1], ok, type(msg) == "string" and msg:gsub("^[^:]+:%d+: ", "") or type(msg),
    type(msg) == "string" and msg:find("^[^:]+:%d+:") ~= nil)
end
