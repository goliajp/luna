-- v2.11 CORPUS-II: pairs on empty + single element.
local empty = {}
local n = 0
for _ in pairs(empty) do n = n + 1 end
print(n)  -- 0

-- single element with pairs
local one = {only = "here"}
for k, v in pairs(one) do print(k, v) end
