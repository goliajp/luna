-- v2.13 CORPUS-IV: __tostring wins over __name; __name shapes
-- the default prefix (address suffix stripped for determinism).
local named = setmetatable({}, { __name = "MyType" })
print(tostring(named):match("^MyType: ") ~= nil)
local both = setmetatable({}, {
  __name = "Ignored",
  __tostring = function() return "custom_str" end,
})
print(tostring(both))
local plain = setmetatable({}, {})
print(tostring(plain):match("^table: ") ~= nil)
