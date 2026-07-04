-- v2.14 CV.3: tostring honors __tostring, then __name.
local named = setmetatable({}, { __name = "MyType" })
local s = tostring(named)
print(s:match("^MyType: ") ~= nil)
local shown = setmetatable({}, { __tostring = function() return "SHOWN" end })
print(tostring(shown))
print(tostring(nil), tostring(true))
