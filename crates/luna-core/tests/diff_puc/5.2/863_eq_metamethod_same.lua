-- __eq in 5.1/5.2 fires only when both operands' metatables yield the
-- same metamethod; 5.3+ take it from either operand.
local ld = loadstring or load
local function e(src) print(pcall(ld(src, "=c"))) end
local pre = "local function mt(m) return setmetatable({}, m) end "
e(pre .. "return mt({__eq = function() return true end}) == mt({__eq = function() return true end})")
e(pre .. "return mt({__eq = function() return true end}) == {}")
e(pre .. "local f = function() return true end return mt({__eq = f}) == mt({__eq = f})")
