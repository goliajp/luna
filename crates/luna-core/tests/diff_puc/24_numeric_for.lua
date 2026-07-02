-- v2.10 CORPUS: numeric for-loop edge cases.
-- descending
for i = 10, 1, -2 do io.write(i, " ") end
print()

-- float step
for i = 0.0, 1.0, 0.25 do io.write(string.format("%.2f ", i)) end
print()

-- backward with positive step (empty)
for i = 5, 1 do io.write(i, " ") end
print("(none)")

-- large range integer
local sum = 0
for i = 1, 1000 do sum = sum + i end
print(sum)  -- 500500
