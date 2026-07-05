-- v2.15 P2.5 (5.1): table library basics.
local t = {1, 2, 3, 4, 5}
print(#t)
table.insert(t, 6)
print(#t, t[6])
print(table.remove(t))
print(table.concat({"a", "b", "c"}, "-"))
