local ok = _VERSION >= "Lua 5.4"
if not ok then error("no close") end
local f = load([[
local function closer() return setmetatable({}, {__close = function() error("in close") end}) end
do local c <close> = closer() end
]])
f()
