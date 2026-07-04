-- v2.14 HD 5.4 seed: integer numeric-for termination guard near
-- maxinteger (5.4 rework).
local n = 0
for i = math.maxinteger - 1, math.maxinteger do n = n + 1 end
print(n)
for i = 1, 0 do error("never") end
print("empty_ok")
