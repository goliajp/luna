-- `x ^ 2`: 5.4 and 5.5 square by multiplying (`luai_numpow`), 5.3 calls
-- the C library's pow, and the two differ in the last bit for some x.
local b = 2
local function f(x) return string.format("%.17g", x) end
for _, x in ipairs({117440513, 117440511, -994132440, 127496801, -1843293392, 3, 0.1, -2.5}) do
  print(f(x ^ 2), f(x ^ b), f(x ^ (b % 4)), f(x ^ 2.0), f(x ^ 3))
end
print(f(117440513 ^ "2"), f("117440513" ^ 2))
-- hot enough for the JIT
local acc = 0
for i = 1, 400 do
  local x = (i % 2 == 0) and 117440513 or 3
  acc = acc + x ^ b % 7
end
print(f(acc))
