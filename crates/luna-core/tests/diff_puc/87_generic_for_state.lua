-- v2.11 CORPUS-II: generic-for state semantics.
-- Stateless iterator with control var
local function fib(prev, cur)
  return cur, cur + prev
end
local a, b = 0, 1
for i = 1, 5 do
  io.write(b, " ")
  a, b = fib(a, b)
end
print()
