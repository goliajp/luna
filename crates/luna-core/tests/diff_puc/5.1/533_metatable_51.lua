-- v2.15 P2.5 (5.1): basic __index.
local Base = {val = 100}
local o = setmetatable({}, {__index = Base})
print(o.val)
print(o.other)
