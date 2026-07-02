-- v2.11 CORPUS-II: table.sort with custom comparator.
local t = {5, 1, 4, 2, 3}
table.sort(t, function(a, b) return a > b end)
print(table.concat(t, ","))

-- sort strings
local s = {"banana", "apple", "cherry"}
table.sort(s)
print(table.concat(s, ","))

-- stable ordering not guaranteed; use distinct comparator
local pairs_t = {{1,"a"}, {2,"b"}, {3,"c"}}
table.sort(pairs_t, function(x, y) return x[1] > y[1] end)
for _, p in ipairs(pairs_t) do io.write(p[1], "=", p[2], " ") end
print()
