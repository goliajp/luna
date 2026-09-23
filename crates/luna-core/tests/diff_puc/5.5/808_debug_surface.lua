-- v3.1 debug slice: the debug library's functions in each version —
-- debug.debug everywhere, the uservalue / upvalueid pair from 5.2,
-- setcstacklimit only in 5.4 (a stub returning LUAI_MAXCCALLS).
local ks = {}
for k in pairs(debug) do ks[#ks + 1] = k end
table.sort(ks)
print(table.concat(ks, ","))
if debug.setcstacklimit then
  print(debug.setcstacklimit(100), pcall(debug.setcstacklimit, "x"))
end
