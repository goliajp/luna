-- v2.12 CORPUS-III: numeric-for negative step.
local vs = {}
for i = 10, 1, -3 do vs[#vs+1] = i end
print(table.concat(vs, ","))

for i = 5, 5 do io.write(i, " ") end
print()

for i = 5, 5, -1 do io.write(i, " ") end
print()

-- empty range with wrong-direction step
for i = 1, 5, -1 do io.write(i, " ") end
print("(empty)")
