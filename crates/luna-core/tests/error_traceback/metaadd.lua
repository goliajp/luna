local o = setmetatable({}, {__add = function() error("add") end})
local function h() return o + 1 end
h()
