-- Subtracting a literal integer zero. 5.4 and 5.5 compile `x - K` for a
-- small integer constant K as `x + -K` (ADDI), which for K = 0 turns
-- `-0.0 - 0` into `-0.0 + 0`, i.e. 0.0; a variable zero, a float literal and
-- 5.3's plain subtraction keep -0.0. A zero-valued float result is never
-- constant folded, and a non-number still subtracts (`__sub`, strings).
local function s(x) return string.format("%s %s", x, 1/x) end
local x = -0.0
local y = 0
print("literal", s(x - 0))
print("variable", s(x - y))
print("folded", s(x - (1-1)))
print("negated", s(x - -0))
print("float literal", s(x - 0.0))
print("parens", s((x) - (0)))
print("constants", s(-0.0 - 0))
print("left zero", s(0 - x))
print("string", s("-0.0" - 0))
print("integer", x // 1 - 0, math.type(y - 0))
local mt = setmetatable({}, {
  __sub = function(a, b) return "sub " .. tostring(b) end,
  __add = function() return "add" end,
})
print("metamethod", mt - 0)
-- the message without its position (the harness names chunks differently)
local function err(f) return (select(2, pcall(f)):gsub("^[^:]*:%d+: ", "")) end
print("bad metamethod", err(function() return setmetatable({}, {__sub = 1}) - 0 end))
print("arith error", err(function() local t = {} return t - 0 end))
-- hot enough for the JIT
local pos = 0
for i = 1, 500 do
  local r = (i % 2 == 0 and x or 1.5) - 0
  if 1 / r > 0 then pos = pos + 1 end
end
print("loop", pos)
print("fuzz", ((-1734830080 % 1.0) - 0) % 1)
