-- A boolean read from a table by a string key held in a register, in a
-- loop the trace JIT takes: the trace reads either boolean.
local t = { a = false, b = true, c = false }
local keys = { "a", "b", "c" }
local n = 0
for i = 1, 300 do
  local k = keys[i % 3 + 1]
  if t[k] then n = n + 1 end
end
print(n)
