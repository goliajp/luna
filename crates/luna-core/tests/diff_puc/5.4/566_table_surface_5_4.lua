-- v2.18 P3: the table library's surface is dialect-specific.
--
-- luna used to register the union of every dialect's functions, so 5.1 saw
-- table.create/move/pack/unpack, 5.2 saw create/move and 5.3/5.4 saw
-- create -- while 5.1 was missing setn and 5.2 was missing maxn. Code
-- written against an older dialect would silently pick up newer API.
--
-- Pins the exact name set against stock PUC 5.4.
local k = {}
for n in pairs(table) do k[#k + 1] = n end
table.sort(k)
print(table.concat(k, " "))
