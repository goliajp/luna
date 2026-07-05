-- v2.14 CV.2: __metatable-protected table rejects setmetatable.
local t = setmetatable({}, { __metatable = "locked" })
setmetatable(t, {})
