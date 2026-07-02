-- v2.12 CORPUS-III: table.insert positional variants.
local t = {}
table.insert(t, "a")
table.insert(t, "b")
table.insert(t, 1, "z")
print(table.concat(t, ","))   -- z,a,b

table.insert(t, 3, "y")
print(table.concat(t, ","))   -- z,a,y,b

-- remove
print(table.remove(t))         -- b (last)
print(table.concat(t, ","))    -- z,a,y

print(table.remove(t, 1))      -- z (first)
print(table.concat(t, ","))    -- a,y
