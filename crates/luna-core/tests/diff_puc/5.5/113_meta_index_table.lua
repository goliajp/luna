-- v2.12 CORPUS-III: __index as table for prototype-style inheritance.
local Base = {name = "Base"}
function Base:hello() return "hi from " .. self.name end

local Derived = setmetatable({}, {__index = Base})
Derived.name = "Derived"

print(Derived:hello())     -- hi from Derived
print(Base:hello())        -- hi from Base

-- Instance of Derived
local instance = setmetatable({}, {__index = Derived})
instance.name = "instance"
print(instance:hello())    -- hi from instance
