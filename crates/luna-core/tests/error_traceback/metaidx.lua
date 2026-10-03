local o = setmetatable({}, {__index = function(t, k) error("idx " .. k) end, __add = function() error("add") end})
local function h() return o + 1 end
local _ = o.zz
