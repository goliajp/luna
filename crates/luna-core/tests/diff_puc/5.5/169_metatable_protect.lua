-- v2.13 CORPUS-IV: __metatable field protects the metatable —
-- getmetatable returns it, setmetatable errors.
local real_mt = { __metatable = "locked" }
local o = setmetatable({}, real_mt)
print(getmetatable(o))
local ok, err = pcall(setmetatable, o, {})
print(ok, err:match("protected metatable") ~= nil)
-- plain table: getmetatable returns the mt itself
local mt2 = {}
local p = setmetatable({}, mt2)
print(getmetatable(p) == mt2)
print(getmetatable(42), getmetatable(true))
