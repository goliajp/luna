-- v2.2 Phase 5 (DP) deterministic diff fixture: control flow.
-- if / while / for / break / continue (via goto).

for i = 1, 5 do print("for", i) end

local n = 10
while n > 5 do
  print("while", n)
  n = n - 1
end

local i = 0
repeat
  i = i + 1
  print("repeat", i)
until i >= 3

if 1 < 2 then print("if-true") else print("if-false") end
if 2 < 1 then print("if-true") else print("if-false") end

local sum = 0
for v in ipairs({10, 20, 30}) do sum = sum + v end
print("sum-of-iter-i", sum)

-- recursion
local function fib(n)
  if n < 2 then return n end
  return fib(n - 1) + fib(n - 2)
end
print("fib(10)", fib(10))
