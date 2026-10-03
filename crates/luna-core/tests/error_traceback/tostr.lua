local o = setmetatable({}, {__tostring = function(s) error("ts") end})
print(tostring(o))
