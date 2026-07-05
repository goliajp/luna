-- v2.13 CORPUS-IV: float-step numeric for — iteration count is
-- computed once (no drift accumulation surprises across impls
-- when the values are exactly representable).
local n = 0
for i = 0, 1, 0.25 do n = n + 1 end
print(n)
local xs = {}
for i = 1, 2, 0.5 do xs[#xs + 1] = i end
print(table.concat(xs, ","))
local m = 0
for i = 10, 1, -2.5 do m = m + 1 end
print(m)
for i = 1, 0 do error("never") end
print("empty_ok")
