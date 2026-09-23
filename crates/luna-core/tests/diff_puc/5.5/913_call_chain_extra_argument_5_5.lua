-- v3.1: 5.5's luaL_argerror counts the objects a __call chain puts in
-- front of the arguments separately ("extraargs"): an error in one of them
-- is a "bad extra argument", and the real arguments are numbered without
-- them. Positions are stripped (chunk names differ between harness sides).
local function msg(f) local _, e = pcall(f) return (tostring(e):gsub("^[^:]+:%d+: ", "")) end
local t = setmetatable({}, {__call = string.rep})
print(msg(function() return t() end))
print(msg(function() return t(3) end))
local u = setmetatable({}, {__call = function(self, s, n) return string.rep(s, n) end})
print(msg(function() return u("x", "y") end))
local two = setmetatable({}, {__call = t})
print(msg(function() return two() end))
local s = setmetatable({}, {__call = string.sub})
print(msg(function() return s("abc", "x") end))
print(select("#", pcall(t, 1)), (select(2, pcall(t, 1)):gsub("^[^:]+:%d+: ", "")))
