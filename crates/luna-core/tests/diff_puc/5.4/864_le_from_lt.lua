-- Without __le, `a <= b` becomes `not (b < a)` up to 5.4 (5.4 through
-- its default build's LUA_COMPAT_LT_LE); __lt is looked up as for
-- `b < a`, on b first.
local ld = loadstring or load
local function e(src) print(pcall(ld(src, "=c"))) end
local pre = "local function mt(m) return setmetatable({}, m) end "
e(pre .. "local m = {__lt = function(a, b) return false end} return mt(m) <= mt(m)")
e(pre .. "local a = mt({__lt = function() return 'from a' end}) local b = mt({__lt = function() return false end}) return a <= b, b >= a")
e(pre .. "return mt({}) <= mt({})")
