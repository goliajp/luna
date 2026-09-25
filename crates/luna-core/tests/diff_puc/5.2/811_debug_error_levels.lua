-- v3.1 debug slice: error(msg, level) positions (luaL_where) count every
-- C function as a level of its own.
local function lvl(n) error("msg", n) end
local function mask(s) return (tostring(s):gsub("^[%w_%.%-]+:%d+:", "POS:")) end
for n = 1, 4 do print(n, mask(select(2, pcall(lvl, n)))) end
print(mask(select(2, pcall(pcall, error, "m", 2))))
local function outer(n) local function inner() error("msg", n) end inner() end
for n = 1, 4 do print(n, mask(select(2, pcall(outer, n)))) end
