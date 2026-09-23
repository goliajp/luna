-- v3.1 bytecode: small functions and loops hot enough for luna's JIT to
-- compile translated code.
local function sq(x) return x * x end
local function add3(a, b, c) return a + b + c end
local function pick(t, i) return t[i] end
local function fib(n) if n < 2 then return n end return fib(n - 1) + fib(n - 2) end
local s = 0
local t = {1, 2, 3}
for i = 1, 3000 do s = s + sq(i) + add3(i, 1, 2) + pick(t, 1 + i % 3) end
local acc = 0
for _, v in ipairs(t) do acc = acc + v end
local w, i = 0, 0
while i < 20000 do
  i = i + 1
  if i % 3 == 0 then w = w + 1 end
end
print(s, fib(20), acc, w)
