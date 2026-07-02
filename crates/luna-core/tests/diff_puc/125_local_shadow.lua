-- v2.12 CORPUS-III: shadow across for-loop body.
local x = "outer"
for x = 1, 3 do io.write(x, " ") end
print()
print(x)   -- outer (loop var doesn't leak)

-- shadow in do-end
local y = "outer_y"
do local y = "inner_y"; print(y) end
print(y)
