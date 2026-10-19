-- from 5.3, a C string handed to `luaS_new` (`lua_pushstring`,
-- `lua_getfield`, the parser's chunk name) is looked up first in a cache
-- by its address: a long string found there with the same characters is
-- the one used, and no new string is made. The same address comes back
-- for the characters of the same Lua string, for a buffer in the same C
-- function's frame on the same C stack, for the environment. A collection
-- forgets the strings that died
local ld = loadstring or load
local keep = {""}
for i = 1, 3000 do keep[#keep + 1] = "k" .. i end
local function bytes(f)
  collectgarbage(); collectgarbage(); collectgarbage("stop")
  local before = collectgarbage("count")
  f()
  return string.format("%.0f", (collectgarbage("count") - before) * 1024)
end
local function twice(title, f)
  local a, b = bytes(f), nil
  local before = collectgarbage("count")
  f()
  b = string.format("%.0f", (collectgarbage("count") - before) * 1024)
  print(title, a, b)
end
local src = "return 1 -- a chunk whose source is its own long chunk name"
local name = "=a chunk name that is longer than forty characters"
twice("load", function() ld(src) end)
twice("load named", function() ld("return 1", name) end)
twice("getinfo S", function() debug.getinfo(1, "S") end)
twice("getinfo S in pcall", function() pcall(debug.getinfo, 1, "S") end)
local function a_function_with_a_name_longer_than_forty_chars()
  return debug.getinfo(1, "n")
end
twice("getinfo n", function() a_function_with_a_name_longer_than_forty_chars() end)
local function f(a_parameter_with_a_name_longer_than_forty_chars) end
if _VERSION ~= "Lua 5.1" then
  twice("getlocal", function() debug.getlocal(f, 1) end)
end
local an_upvalue_with_a_name_longer_than_forty_chars = 1
local function g() return an_upvalue_with_a_name_longer_than_forty_chars end
twice("getupvalue", function() debug.getupvalue(g, 1) end)
local mod = "a.module.name.that.is.longer.than.forty.characters"
package.preload[mod] = function() return {} end
twice("require", function() require(mod) end)
twice("getenv", function() os.getenv("PATH") end)
