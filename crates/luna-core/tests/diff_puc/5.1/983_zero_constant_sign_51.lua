-- 5.1 keys a function's constant table by number value, and 0 == -0: the
-- first zero a function loads decides the sign of every later one. A folded
-- `x * 0` is -0 when x is negative.
print(tostring(0) .. tostring(-812646400 * 0))
local function f() return tostring(-5 * 0) .. tostring(0) .. tostring(0.0) end
print(f())
local function g() local z = -0 return z, 0, 1 / z end
print(g())
print(-0, 0)
-- parentheses and a negated right operand still fold
local function h()
  return tostring((0)) .. tostring((-812646400) * (0)) .. tostring(3 * -(0)) .. tostring(-(2) * ((0)))
end
print(h())
local function k() return tostring((-7) * (0)) .. tostring(0) end
print(k())
-- `%` and `^` fold too; a division or modulo by zero does not
local function m()
  return tostring((-1159611904 * 0) ^ (0 % 4)) .. tostring((0 % 4) * -1) .. tostring(0) .. tostring(1 / 0)
end
print(m())
local function n() return tostring(-2 * (3 % 3)) .. tostring(0 % 5) .. tostring(5 % 0 ~= 5 % 0) end
print(n())
-- a logical operation with a fixed outcome takes part in folding, and the
-- constants its test compared still enter the constant table first
local function o() return tostring((-65536 * 0) % ((0) ~= 0 and (0) or 1)) .. tostring(0) end
print(o())
local function q() return tostring(-3 * ((false) ~= 0 and (false) or 1) * 0) .. tostring(0) end
print(q())
local function r() return tostring((-9 * 0) % ((nil) ~= -0 and (nil) or 1)) .. tostring(0) end
print(r())
local function s() return tostring((true and 0) * -1) .. tostring(0) end
print(s())
