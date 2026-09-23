-- Up to 5.3 a call follows one __call hop, and the handler must be a
-- function; otherwise the error names the original object. 5.4 retries
-- the call with whatever __call holds.
local ld = loadstring or load
local function e(src) print(pcall(ld(src, "=c"))) end
local pre = "local function mt(m) return setmetatable({}, m) end "
e(pre .. "local o = mt({__call = 1}) return o()")
e(pre .. "local inner = mt({__call = function(self, a) return 'inner ' .. type(a) end}) local outer = mt({__call = inner}) return outer(5)")
e(pre .. "local f = function() return 'deep' end for i = 1, 5 do f = mt({__call = f}) end return f()")
e(pre .. "local o = mt({__call = function(...) return select('#', ...) end}) return o(1, 2)")
