local function f() error(setmetatable({}, {__tostring = function() return "custom" end})) end
f()
